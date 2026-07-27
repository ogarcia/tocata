// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Password hashing, and the secrets this server issues itself.

use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use sha2::{Digest, Sha256};

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
}
