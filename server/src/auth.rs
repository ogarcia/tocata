// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Password hashing, and the secrets this server issues itself.

use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Salt length recommended by the Argon2 authors.
const SALT_BYTES: usize = 16;

/// Every random value here comes from the operating system, through one door.
fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("reading from the system RNG: {e}"))?;
    Ok(bytes)
}

/// Length of a generated initial password, in characters of the alphabet
/// below.
const INITIAL_PASSWORD_CHARS: usize = 20;

/// Unambiguous alphabet: no O/0, no l/1/I. These passwords get read off a
/// terminal and typed into a phone.
const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Hashes a password for storage. Argon2id with the crate defaults.
pub fn hash_password(password: &str) -> Result<String> {
    let salt_bytes = random_bytes::<SALT_BYTES>()?;
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|e| anyhow!("encoding password salt: {e}"))?;

    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow!("hashing password: {e}"))
}

/// Checks a password against a stored hash. Comparison happens inside argon2
/// and is constant time.
pub fn verify_password(password: &str, stored: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// How long an answer already worked out is trusted to still be the answer.
const REMEMBERED_FOR: Duration = Duration::from_secs(15 * 60);

/// How many are held at once. A few hundred covers every client of every account
/// on a server this size; the cap is here so a stream of wrong names cannot make
/// this grow without end.
const REMEMBERED_MOST: usize = 512;

/// Verifications already paid for.
///
/// Keyed by a digest and holding nothing but a moment, so what is in memory says
/// neither which password was offered nor whose it was.
static REMEMBERED: Mutex<BTreeMap<[u8; 32], Instant>> = Mutex::new(BTreeMap::new());

/// A value this process picks once, so the keys above are meaningless outside it.
///
/// Without it the map would be a table of "this password goes with this stored
/// hash", computed with a digest anybody can compute — worth nothing on its own,
/// and still not something to leave lying in memory for a core dump to carry off.
fn pepper() -> &'static [u8; 32] {
    static PEPPER: OnceLock<[u8; 32]> = OnceLock::new();

    // A failing system RNG at this point would be a machine in no state to serve
    // anything, and a fixed value here weakens nothing: the keys are digests of
    // values that are already secret.
    PEPPER.get_or_init(|| random_bytes().unwrap_or([0; 32]))
}

fn remembrance(password: &str, stored: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(pepper());
    // The lengths go in as well, so no two different pairs can be run together
    // into the same bytes.
    hasher.update((stored.len() as u64).to_le_bytes());
    hasher.update(stored.as_bytes());
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

/// The same check, for the path that runs on every single request.
///
/// OpenSubsonic has no sessions: a client sends its credentials again with every
/// call, so a screen of fifty covers asks fifty times whether the same password
/// goes with the same stored hash. Argon2 is built to be slow — deliberately,
/// and rightly, for the one thing it is for — and paying it per request measured
/// at 358 milliseconds on every call of every client on an old machine, against
/// 32 for the one endpoint that answers without credentials.
///
/// So an answer worked out once is trusted for [`REMEMBERED_FOR`], and nothing
/// about how passwords are stored changes: this is the same argon2, asked fewer
/// times.
///
/// **Keyed by the stored hash, which is what makes this safe to forget about.**
/// Change a password and it is hashed with a fresh salt, so the stored value is
/// different and no entry here can ever match it again. Delete an account and the
/// row is gone before this is reached. There is no invalidation to call and so
/// none to forget, which is the only kind that cannot be got wrong.
///
/// Only successes are kept. Remembering a failure would make guessing cheap,
/// which is the exact opposite of what argon2 is here for.
pub fn verify_password_remembering(password: &str, stored: &str) -> bool {
    let key = remembrance(password, stored);
    let now = Instant::now();

    if let Ok(mut held) = REMEMBERED.lock()
        && let Some(until) = held.get(&key).copied()
    {
        if until > now {
            return true;
        }
        held.remove(&key);
    }

    if !verify_password(password, stored) {
        return false;
    }

    if let Ok(mut held) = REMEMBERED.lock() {
        if held.len() >= REMEMBERED_MOST {
            held.retain(|_, until| *until > now);
        }
        // Still full of answers that have not run out yet: let them go rather
        // than grow. Emptying costs the next request an argon2, which is what it
        // would have paid anyway.
        if held.len() >= REMEMBERED_MOST {
            held.clear();
        }
        held.insert(key, now + REMEMBERED_FOR);
    }

    true
}

/// Length of a generated token, in bytes before hex encoding.
const TOKEN_BYTES: usize = 32;

/// Hashes a secret this server issued, for storage and lookup: an API key, a
/// session token.
///
/// SHA-256 rather than Argon2 on purpose. Stretching defends a secret somebody
/// chose, which is guessable; these are 256 bits from the system RNG, so there is
/// nothing to slow an attacker down against. And this runs on every
/// authenticated request.
pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    to_hex(&hasher.finalize())
}

/// Builds an opaque secret to hand to a client.
pub fn generate_token() -> Result<String> {
    Ok(to_hex(&random_bytes::<TOKEN_BYTES>()?))
}

/// Builds a password for the account created on first start.
pub fn generate_initial_password() -> Result<String> {
    let bytes = random_bytes::<INITIAL_PASSWORD_CHARS>()?;

    // The alphabet length does not divide 256, so this is very slightly
    // biased. Irrelevant next to 20 characters of entropy, and the
    // alternative is rejection sampling for no gain.
    Ok(bytes
        .iter()
        .map(|b| PASSWORD_ALPHABET[*b as usize % PASSWORD_ALPHABET.len()] as char)
        .collect())
}

/// Decodes the `enc:` form the API allows for the `p` parameter, where the
/// password travels as hex. Anything else is already plain text.
pub fn decode_password(value: &str) -> Option<String> {
    match value.strip_prefix("enc:") {
        Some(encoded) => from_hex(encoded).and_then(|bytes| String::from_utf8(bytes).ok()),
        None => Some(value.to_string()),
    }
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }

    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_against_its_hash() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("Correct horse battery staple", &hash));
        assert!(!verify_password("", &hash));
    }

    #[test]
    fn the_same_password_hashes_differently_every_time() {
        let one = hash_password("hunter2").unwrap();
        let two = hash_password("hunter2").unwrap();
        assert_ne!(one, two, "each hash must carry its own salt");
        assert!(verify_password("hunter2", &one));
        assert!(verify_password("hunter2", &two));
    }

    #[test]
    fn a_hash_is_argon2id() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"), "got {hash}");
    }

    #[test]
    fn garbage_in_the_hash_column_fails_closed() {
        assert!(!verify_password("hunter2", "not a hash"));
        assert!(!verify_password("hunter2", ""));
    }

    #[test]
    fn hashing_an_issued_secret_is_stable() {
        let key = "24a2f3c1b0e9d8a7";
        assert_eq!(hash_secret(key), hash_secret(key));
        assert_ne!(hash_secret(key), hash_secret("other"));
        assert_eq!(hash_secret(key).len(), 64);
        assert!(hash_secret(key).chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn initial_passwords_avoid_ambiguous_characters() {
        for _ in 0..64 {
            let password = generate_initial_password().unwrap();
            assert_eq!(password.len(), INITIAL_PASSWORD_CHARS);
            assert!(
                !password.contains(['0', 'O', 'l', '1', 'I']),
                "got {password}"
            );
        }
    }

    #[test]
    fn plain_passwords_pass_through() {
        assert_eq!(decode_password("hunter2").as_deref(), Some("hunter2"));
    }

    #[test]
    fn hex_encoded_passwords_are_decoded() {
        assert_eq!(
            decode_password("enc:68756e74657232").as_deref(),
            Some("hunter2")
        );
        // Uppercase hex is just as valid.
        assert_eq!(
            decode_password("enc:68756E74657232").as_deref(),
            Some("hunter2")
        );
    }

    #[test]
    fn malformed_hex_is_rejected_rather_than_guessed() {
        assert_eq!(decode_password("enc:abc"), None, "odd length");
        assert_eq!(decode_password("enc:zzzz"), None, "not hex");
        assert_eq!(
            decode_password("enc:"),
            Some(String::new()),
            "empty is empty"
        );
    }

    /// A remembered verification is the same answer, not a looser one.
    ///
    /// The wrong password stays wrong however many times it is offered, and it is
    /// never remembered: an answer kept for a failure would turn argon2 into a
    /// one-time toll on guessing instead of a toll on every guess.
    #[test]
    fn remembering_a_verification_does_not_change_the_answer() {
        let stored = hash_password("the right one").unwrap();

        assert!(verify_password_remembering("the right one", &stored));
        assert!(
            verify_password_remembering("the right one", &stored),
            "the second time comes from memory and must agree with the first"
        );

        for _ in 0..3 {
            assert!(
                !verify_password_remembering("the wrong one", &stored),
                "a wrong password is wrong every time it is offered"
            );
        }
    }

    /// What makes this safe without anything to invalidate: a new password is
    /// hashed with a fresh salt, so the stored value it is remembered against no
    /// longer exists and the old answer cannot be reached by any key.
    #[test]
    fn a_changed_password_leaves_nothing_behind_that_still_opens() {
        let old = hash_password("the old one").unwrap();
        assert!(verify_password_remembering("the old one", &old));

        let new = hash_password("the new one").unwrap();
        assert_ne!(old, new, "a fresh salt every time is what this rests on");

        assert!(
            !verify_password_remembering("the old one", &new),
            "the password that was replaced opens nothing"
        );
        assert!(verify_password_remembering("the new one", &new));
    }

    /// Two accounts that chose the same password are remembered apart, because
    /// their stored hashes differ and the stored hash is half of the key.
    #[test]
    fn the_same_password_under_two_accounts_is_two_answers() {
        let hers = hash_password("hunter2").unwrap();
        let his = hash_password("hunter2").unwrap();

        assert!(verify_password_remembering("hunter2", &hers));
        assert!(verify_password_remembering("hunter2", &his));
        assert!(!verify_password_remembering("hunter3", &hers));
    }

    /// It is meant to be faster, and a test that did not check would not notice
    /// the day the memory stopped being consulted. Generous on purpose: the
    /// figure that matters is thousands of times, and asserting ten keeps this
    /// from failing on a machine that happened to be busy.
    #[test]
    fn an_answer_from_memory_is_the_cheap_one() {
        let stored = hash_password("something to remember").unwrap();

        let first = std::time::Instant::now();
        assert!(verify_password_remembering(
            "something to remember",
            &stored
        ));
        let paid = first.elapsed();

        let again = std::time::Instant::now();
        for _ in 0..10 {
            assert!(verify_password_remembering(
                "something to remember",
                &stored
            ));
        }
        let remembered = again.elapsed() / 10;

        assert!(
            remembered * 10 < paid,
            "argon2 took {paid:?} and the memory {remembered:?}, which is not an answer being saved"
        );
    }
}
