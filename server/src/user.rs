// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Users and how a request proves it is one of them.
//!
//! Verification lives here rather than beside the request handling so the
//! stored hash never leaves this module.

use crate::auth;
use crate::db::InTurn;
use crate::db::now;
use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::sync::LazyLock;
use tracing::{info, warn};

/// Name of the account created on an empty database.
const INITIAL_USERNAME: &str = "admin";

/// Verified against when the username does not exist, so a request for a
/// missing user costs the same as one for a real user. Without this, response
/// time tells an attacker which accounts exist.
static ABSENT_USER_HASH: LazyLock<String> = LazyLock::new(|| {
    auth::hash_password("no such user").expect("hashing must work for the server to be usable")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
}

/// Checks a username and password. `None` means no such user or a wrong
/// password, deliberately indistinguishable to the caller.
/// Checks a password against an account named by whatever somebody typed into the
/// panel's login form: the account's name, or the address on it.
///
/// **The panel only.** `/rest` authenticates by username and nothing else, because
/// that is what the protocol says a client sends, and a server that quietly accepted
/// something else there would be a server whose behaviour no client can predict.
///
/// The name is tried first. If an account is called `a@b.com` and another one carries
/// that as its address, the name wins — a username is what an account *is*, and an
/// address is a second way of pointing at one.
///
/// It is a way in and not a name to be greeted by, so nothing about it is shown or
/// echoed: a wrong address and a wrong name come back the same way, which is what
/// keeps this from answering whether an address has an account behind it.
pub async fn authenticate_panel(
    pool: &SqlitePool,
    who: &str,
    password: &str,
) -> Result<Option<User>> {
    if let Some(user) = authenticate_password(pool, who, password).await? {
        return Ok(Some(user));
    }

    // Only if the name matched nothing at all. A name that exists and a password that
    // does not go with it is a failure, and going on to try the address would let
    // somebody's wrong password be checked against a second account.
    if named(pool, who).await?.is_some() {
        return Ok(None);
    }

    let Some(username) = with_address(pool, who).await? else {
        // Nothing to check against, and the same time spent as if there had been.
        auth::verify_password(password, &ABSENT_USER_HASH);
        return Ok(None);
    };

    authenticate_password(pool, &username, password).await
}

/// Whether an account goes by this name, without checking anything about it.
async fn named(pool: &SqlitePool, username: &str) -> Result<Option<i64>> {
    sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
        .context("looking up an account by name")
}

/// The account carrying this address, if one does.
///
/// Folded to compare, the way the index that keeps them unique is folded: nobody
/// types their own address the same way twice.
async fn with_address(pool: &SqlitePool, email: &str) -> Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT username FROM users WHERE email IS NOT NULL AND lower(email) = lower(?)",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
    .context("looking up an account by address")
}

pub async fn authenticate_password(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<Option<User>> {
    let row: Option<(i64, String, bool, String)> = sqlx::query_as(
        "SELECT id, username, is_admin, password_hash FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .context("looking up user by name")?;

    match row {
        Some((id, username, is_admin, hash)) => {
            // Remembering rather than plain verifying, because this runs on every
            // OpenSubsonic request. The account is still read from the database
            // every time, so what it may do is never remembered — only that this
            // password goes with that stored hash.
            if auth::verify_password_remembering(password, &hash) {
                seen(pool, id).await;

                Ok(Some(User {
                    id,
                    username,
                    is_admin,
                }))
            } else {
                Ok(None)
            }
        }
        None => {
            // Spend the same time as a real verification would.
            auth::verify_password(password, &ABSENT_USER_HASH);
            Ok(None)
        }
    }
}

/// Resolves an API key to its owner, without claiming it was used.
///
/// Split from the use it is put to because a key can be recognised and still
/// not let the request through, and a key that opened nothing was not used.
async fn lookup_api_key(pool: &SqlitePool, key: &str) -> Result<Option<(i64, User)>> {
    let key_hash = auth::hash_secret(key);

    // An expired key stays in the table so its date can be pushed out later, and
    // a revoked one stays so it can still be read in the panel, so both have to
    // be turned away here rather than by not being there. The comparison is
    // against a bound timestamp and not SQLite's own `datetime('now')`, which
    // writes a space where the schema writes a T.
    let row: Option<(i64, i64, String, bool)> = sqlx::query_as(
        "SELECT k.id, u.id, u.username, u.is_admin
           FROM api_keys k
           JOIN users u ON u.id = k.user_id
          WHERE k.key_hash = ? AND k.revoked_at IS NULL
            AND (k.expires_at IS NULL OR k.expires_at > ?)",
    )
    .bind(&key_hash)
    .bind(now())
    .fetch_optional(pool)
    .await
    .context("looking up API key")?;

    Ok(row.map(|(key_id, id, username, is_admin)| {
        (
            key_id,
            User {
                id,
                username,
                is_admin,
            },
        )
    }))
}

/// How stale an account's `last_seen_at` may get before it is written again. The
/// same five minutes a session's own is written at, for the same reason.
const SEEN_RESOLUTION_MINUTES: i64 = 5;

/// Notes that an account was just used, by whichever door the request came in.
///
/// What it answers is the question an administrator has about an account they are
/// thinking of removing — is anybody still using this? Neither the sessions nor the
/// keys can answer it: a session expires and is swept, and a key that was never
/// used says nothing about the password beside it.
///
/// **It asks before it writes**, and that is the whole shape of this function. The
/// guard used to be inside the `UPDATE` — the row only changes when the note has
/// gone stale, so an update matching nothing writes no page — and the reasoning was
/// half right: it writes no page, and it takes the write lock all the same. SQLite
/// has one writer, so every authenticated request was queueing for it behind
/// whatever else was writing, for a column that is deliberately kept to the nearest
/// five minutes.
///
/// It showed up on a server scanning eleven thousand files: this statement, three
/// seconds, on a request that wanted nothing written. Reading first costs a query
/// that never waits — readers do not block in WAL — and leaves one write per account
/// per five minutes where there had been one per request.
///
/// **And it cannot fail the thing it is noting.** This is a courtesy write on the end
/// of an authentication that has already succeeded, and it used to be waited on with
/// a `?`: on that same server a password that was right came back as a failure, and
/// the panel called it a wrong password. Somebody was told to doubt their own
/// password because a date could not be written. So a failure here is logged and
/// nothing else.
pub async fn seen(pool: &SqlitePool, user_id: i64) {
    let stale = crate::db::from_now(-chrono::Duration::minutes(SEEN_RESOLUTION_MINUTES));

    // A reader, which in WAL never waits for whoever is writing.
    let fresh: Result<Option<i64>, _> =
        sqlx::query_scalar("SELECT 1 FROM users WHERE id = ? AND last_seen_at >= ?")
            .bind(user_id)
            .bind(&stale)
            .fetch_optional(pool)
            .await;

    match fresh {
        // Recent enough, which is the answer almost every time this is called.
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(e) => return warn!("could not read when an account was last seen: {e}"),
    }

    let written = sqlx::query(
        "UPDATE users SET last_seen_at = ?
          WHERE id = ? AND (last_seen_at IS NULL OR last_seen_at < ?)",
    )
    .bind(now())
    .bind(user_id)
    // Kept in the statement as well as in the question above. Two requests can read
    // "stale" at once, and this is what makes the second one's write a no-op rather
    // than a second write of the same instant.
    .bind(&stale)
    .in_turn(pool)
    .await;

    if let Err(e) = written {
        warn!("could not record that an account was seen: {e}");
    }
}

/// Notes that a key just let a request through, for the panel to show.
///
/// A courtesy write like [`seen`], and it does not fail the request either: the key
/// authenticated, and whether we managed to write down that it did is our problem.
async fn record_api_key_use(pool: &SqlitePool, key_id: i64) {
    let noted = sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
        .bind(now())
        .bind(key_id)
        .in_turn(pool)
        .await;

    if let Err(e) = noted {
        warn!("could not record that an API key was used: {e}");
    }
}

/// Resolves an API key to its owner and records the use.
pub async fn authenticate_api_key(pool: &SqlitePool, key: &str) -> Result<Option<User>> {
    let Some((key_id, user)) = lookup_api_key(pool, key).await? else {
        return Ok(None);
    };

    record_api_key_use(pool, key_id).await;
    seen(pool, user.id).await;

    Ok(Some(user))
}

/// Checks a username against either its password or one of its API keys.
///
/// Almost no client can send `apiKey`: of the eight surveyed only Symfonium
/// has a field for one, while every one of them can be told to send the
/// password. A key nobody can paste anywhere is a key nobody uses, so it is
/// accepted where the password goes — the same accommodation LMS makes. That
/// is what gives the other seven a credential which can be revoked on its own,
/// instead of a password whose change logs every client out at once.
pub async fn authenticate_password_or_api_key(
    pool: &SqlitePool,
    username: &str,
    secret: &str,
) -> Result<Option<User>> {
    if let Some(user) = authenticate_password(pool, username, secret).await? {
        return Ok(Some(user));
    }

    // A key already says whose it is, so one belonging to somebody else is a
    // mistake to reject rather than an invitation to log in as them.
    let Some((key_id, user)) = lookup_api_key(pool, secret).await? else {
        return Ok(None);
    };

    if user.username != username {
        return Ok(None);
    }

    record_api_key_use(pool, key_id).await;
    seen(pool, user.id).await;

    Ok(Some(user))
}

/// Creates the first account when the database has no users, returning the
/// generated password so the caller can show it once.
///
/// The password is random rather than a well known default: a published
/// default is a published credential, and most people never change it.
pub async fn ensure_initial_user(pool: &SqlitePool) -> Result<Option<String>> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .context("counting users")?;

    if count > 0 {
        return Ok(None);
    }

    let password = auth::generate_initial_password()?;
    let hash = auth::hash_password(&password)?;
    let timestamp = now();

    sqlx::query(
        "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
         VALUES (?, ?, 1, ?, ?)",
    )
    .bind(INITIAL_USERNAME)
    .bind(&hash)
    .bind(&timestamp)
    .bind(&timestamp)
    .in_turn(pool)
    .await
    .context("creating the initial user")?;

    info!("created the initial user '{INITIAL_USERNAME}'");
    warn!("initial password for '{INITIAL_USERNAME}': {password}");
    warn!("this is shown once and only once, so write it down now");

    Ok(Some(password))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sets up two accounts, each holding a password and a key, since what the
    /// checks here have to get right is which of the four opens which door.
    async fn two_users_with_keys() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let timestamp = now();

        for (name, password, key) in [
            ("ana", "ana's password", "ana's key"),
            ("bob", "bob's password", "bob's key"),
        ] {
            let hash = auth::hash_password(password).unwrap();
            let user_id: i64 = sqlx::query_scalar(
                "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
                 VALUES (?, ?, 0, ?, ?) RETURNING id",
            )
            .bind(name)
            .bind(&hash)
            .bind(&timestamp)
            .bind(&timestamp)
            .fetch_one(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO api_keys (user_id, key_hash, label, created_at)
                 VALUES (?, ?, 'a client', ?)",
            )
            .bind(user_id)
            .bind(auth::hash_secret(key))
            .bind(&timestamp)
            .execute(&pool)
            .await
            .unwrap();
        }

        pool
    }

    /// Moves a key's expiry, or clears it with `None`.
    async fn expire_at(pool: &SqlitePool, key: &str, when: Option<&str>) {
        sqlx::query("UPDATE api_keys SET expires_at = ? WHERE key_hash = ?")
            .bind(when)
            .bind(auth::hash_secret(key))
            .execute(pool)
            .await
            .unwrap();
    }

    /// Midnight of today, rather than a date years ago.
    ///
    /// Against SQLite's own `datetime('now')` — `2026-07-27 09:00:00` where the
    /// schema writes `2026-07-27T09:00:00Z` — the two agree until the separator
    /// and `'T' > ' '`, so a key that ran out this morning would still let a
    /// request through. A distant date differs early enough to compare correctly
    /// either way, which is why it would prove nothing.
    fn earlier_today() -> String {
        format!("{}T00:00:00Z", &now()[..10])
    }

    #[tokio::test]
    async fn a_key_with_no_expiry_keeps_working() {
        let pool = two_users_with_keys().await;

        assert!(
            authenticate_api_key(&pool, "ana's key")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_key_that_ran_out_today_is_turned_away() {
        let pool = two_users_with_keys().await;
        expire_at(&pool, "ana's key", Some(&earlier_today())).await;

        assert!(
            authenticate_api_key(&pool, "ana's key")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate_password_or_api_key(&pool, "ana", "ana's key")
                .await
                .unwrap()
                .is_none(),
            "and by the other door as well"
        );
    }

    #[tokio::test]
    async fn a_key_with_an_expiry_still_ahead_works() {
        let pool = two_users_with_keys().await;
        expire_at(&pool, "ana's key", Some("2999-01-01T00:00:00Z")).await;

        assert!(
            authenticate_api_key(&pool, "ana's key")
                .await
                .unwrap()
                .is_some()
        );
    }

    /// The point of keeping an expired key rather than sweeping it: the same
    /// secret works again once the date moves, so nothing has to be set up anew.
    #[tokio::test]
    async fn pushing_the_date_out_brings_the_same_key_back() {
        let pool = two_users_with_keys().await;
        expire_at(&pool, "ana's key", Some(&earlier_today())).await;

        expire_at(&pool, "ana's key", Some("2999-01-01T00:00:00Z")).await;

        assert!(
            authenticate_api_key(&pool, "ana's key")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn an_expired_key_is_not_gone_from_the_table() {
        let pool = two_users_with_keys().await;
        expire_at(&pool, "ana's key", Some(&earlier_today())).await;

        let _ = authenticate_api_key(&pool, "ana's key").await.unwrap();

        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE key_hash = ?")
            .bind(auth::hash_secret("ana's key"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    /// A revoked key stays in the table until it is removed, so nothing about it
    /// being gone turns it away: the column has to.
    #[tokio::test]
    async fn a_revoked_key_is_turned_away() {
        let pool = two_users_with_keys().await;
        revoke(&pool, "ana's key").await;

        assert!(
            authenticate_api_key(&pool, "ana's key")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate_password_or_api_key(&pool, "ana", "ana's key")
                .await
                .unwrap()
                .is_none(),
            "and by the other door as well"
        );
    }

    /// Revoked beats an expiry that is still ahead. The two conditions are
    /// separate, and a key with years left on it is exactly the one somebody
    /// revokes.
    #[tokio::test]
    async fn a_revoked_key_with_time_left_is_turned_away_too() {
        let pool = two_users_with_keys().await;
        expire_at(&pool, "ana's key", Some("2999-01-01T00:00:00Z")).await;
        revoke(&pool, "ana's key").await;

        assert!(
            authenticate_api_key(&pool, "ana's key")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_revoked_key_records_no_use() {
        let pool = two_users_with_keys().await;
        revoke(&pool, "ana's key").await;

        let _ = authenticate_api_key(&pool, "ana's key").await.unwrap();

        assert!(last_used(&pool, "ana's key").await.is_none());
    }

    /// Logging into the panel by address, which is a second way of pointing at an
    /// account rather than a second name for one.
    mod by_address {
        use super::*;

        /// Two accounts, one of them with an address on it.
        async fn with_addresses() -> SqlitePool {
            let pool = two_users_with_keys().await;

            sqlx::query("UPDATE users SET email = 'Ana@Example.ORG' WHERE username = 'ana'")
                .execute(&pool)
                .await
                .unwrap();

            pool
        }

        #[tokio::test]
        async fn the_address_lets_the_owner_in() {
            let pool = with_addresses().await;

            let who = authenticate_panel(&pool, "Ana@Example.ORG", "ana's password")
                .await
                .unwrap();

            assert_eq!(who.map(|user| user.username).as_deref(), Some("ana"));
        }

        /// Nobody types their own address the same way twice, and no mail system in
        /// use has cared for decades.
        #[tokio::test]
        async fn it_does_not_matter_how_it_is_capitalised() {
            let pool = with_addresses().await;

            for typed in ["ana@example.org", "ANA@EXAMPLE.ORG", "aNa@ExAmPlE.oRg"] {
                let who = authenticate_panel(&pool, typed, "ana's password")
                    .await
                    .unwrap();

                assert!(who.is_some(), "{typed} should have let ana in");
            }
        }

        /// The address is a way in and not a password: the wrong one is still refused.
        #[tokio::test]
        async fn the_address_does_not_excuse_the_password() {
            let pool = with_addresses().await;

            assert!(
                authenticate_panel(&pool, "ana@example.org", "bob's password")
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        #[tokio::test]
        async fn an_address_nobody_carries_lets_nobody_in() {
            let pool = with_addresses().await;

            assert!(
                authenticate_panel(&pool, "nobody@example.org", "ana's password")
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        /// The name wins, and this is the case that says why it has to.
        ///
        /// An account called `ana@example.org` and another one carrying that as its
        /// address are two accounts one string points at. Trying the name first
        /// settles it the same way every time — and trying the address as well when
        /// the name matched would mean checking somebody's wrong password against a
        /// second account, which is how one account's password ends up opening
        /// another.
        #[tokio::test]
        async fn a_name_that_looks_like_an_address_is_still_a_name() {
            let pool = with_addresses().await;

            let at = now();
            sqlx::query(
                "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
                 VALUES ('ana@example.org', ?, 0, ?, ?)",
            )
            .bind(auth::hash_password("the other one").unwrap())
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            let by_name = authenticate_panel(&pool, "ana@example.org", "the other one")
                .await
                .unwrap();
            assert_eq!(
                by_name.map(|user| user.username).as_deref(),
                Some("ana@example.org"),
                "the account called that is the one that answers"
            );

            // And ana's own password does not open it, even though her address is
            // that string: the name matched, so the address was never tried.
            assert!(
                authenticate_panel(&pool, "ana@example.org", "ana's password")
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        /// `/rest` takes a username and nothing else, because that is what the
        /// protocol says a client sends.
        #[tokio::test]
        async fn opensubsonic_does_not_take_an_address() {
            let pool = with_addresses().await;

            assert!(
                authenticate_password(&pool, "ana@example.org", "ana's password")
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                authenticate_password_or_api_key(&pool, "ana@example.org", "ana's password")
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    /// What an administrator is looking at when they wonder whether an account is
    /// still in use. It has to survive the session being swept and the key never
    /// having been used, so it is written on the account itself.
    #[tokio::test]
    async fn a_key_marks_the_account_as_seen() {
        let pool = two_users_with_keys().await;

        assert!(last_seen(&pool, "ana").await.is_none(), "created, unused");

        authenticate_api_key(&pool, "ana's key").await.unwrap();

        assert!(last_seen(&pool, "ana").await.is_some());
        assert!(
            last_seen(&pool, "bob").await.is_none(),
            "and says nothing about anybody else"
        );
    }

    /// Guarded twice — asked before writing, and again inside the statement — and it
    /// is what keeps a column nobody reads twice a day from costing a write per
    /// request. The write it saves is a write that queues for the lock.
    #[tokio::test]
    async fn an_account_seen_a_moment_ago_is_not_written_again() {
        let pool = two_users_with_keys().await;
        let id = who(&pool, "ana").await;

        // A moment inside the resolution, distinctive enough to recognise.
        sqlx::query("UPDATE users SET last_seen_at = ? WHERE id = ?")
            .bind(crate::db::from_now(-chrono::Duration::minutes(1)))
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        let before = last_seen(&pool, "ana").await;
        seen(&pool, id).await;
        assert_eq!(last_seen(&pool, "ana").await, before, "left alone");

        // And once it has gone stale it moves.
        sqlx::query("UPDATE users SET last_seen_at = '2020-01-01T00:00:00Z' WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        seen(&pool, id).await;
        assert_ne!(
            last_seen(&pool, "ana").await.as_deref(),
            Some("2020-01-01T00:00:00Z")
        );
    }

    /// The failure a real server ran into, as small as it can be made.
    ///
    /// A database that will not take a write is what a scan of eleven thousand files
    /// looked like from the outside: it held the write lock, and everything else got
    /// its five seconds and a refusal. The password was right, and the panel said the
    /// username and password did not go together.
    ///
    /// Both notes on the way through are made unwritable here — the account's and the
    /// session's, each aged past its resolution so that it will be attempted — and
    /// neither may cost anybody their way in.
    #[tokio::test]
    async fn a_database_that_takes_no_writes_still_lets_the_right_password_in() {
        let root = crate::fixtures::temp_root("auth-under-a-locked-database");
        let path = root.join("tocata.db");

        let pool = crate::db::connect(&path).await.unwrap();

        let hash = auth::hash_password("ana's password").unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, last_seen_at, created_at, updated_at)
             VALUES ('ana', ?, '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z',
                     '2020-01-01T00:00:00Z') RETURNING id",
        )
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();

        let (token, _) = crate::session::create(&pool, id, crate::session::A_MONTH)
            .await
            .unwrap();

        sqlx::query("UPDATE sessions SET last_seen_at = '2020-01-01T00:00:00Z'")
            .execute(&pool)
            .await
            .unwrap();

        // The same file, opened by a connection that refuses to write anything. What
        // a locked database does to a statement, without the timing.
        let shut = SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .pragma("query_only", "ON"),
        )
        .await
        .unwrap();

        // Proof that it is really refusing, so that a test that stopped refusing
        // could not quietly go on passing.
        assert!(
            sqlx::query("UPDATE users SET last_seen_at = 'now' WHERE id = ?")
                .bind(id)
                .execute(&shut)
                .await
                .is_err(),
            "this database is supposed to refuse writes"
        );

        let who = authenticate_password(&shut, "ana", "ana's password")
            .await
            .expect("a courtesy note must not fail an authentication");
        assert_eq!(
            who.map(|user| user.username).as_deref(),
            Some("ana"),
            "the password was right"
        );

        let session = crate::session::resolve(&shut, &token)
            .await
            .expect("nor may it fail a session lookup");
        assert!(
            session.is_some(),
            "the session was good; losing it here logs somebody out of the panel"
        );

        // And a wrong password is still wrong, which is the half that must not be
        // loosened by any of this.
        assert!(
            authenticate_password(&shut, "ana", "not it")
                .await
                .unwrap()
                .is_none()
        );
    }

    async fn last_seen(pool: &SqlitePool, username: &str) -> Option<String> {
        sqlx::query_scalar("SELECT last_seen_at FROM users WHERE username = ?")
            .bind(username)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn who(pool: &SqlitePool, username: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
            .bind(username)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Withdraws a key the way the API does, by writing the moment on it.
    async fn revoke(pool: &SqlitePool, key: &str) {
        sqlx::query("UPDATE api_keys SET revoked_at = ? WHERE key_hash = ?")
            .bind(now())
            .bind(auth::hash_secret(key))
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_expired_key_records_no_use() {
        let pool = two_users_with_keys().await;
        expire_at(&pool, "ana's key", Some(&earlier_today())).await;

        let _ = authenticate_api_key(&pool, "ana's key").await.unwrap();

        assert!(last_used(&pool, "ana's key").await.is_none());
    }

    async fn last_used(pool: &SqlitePool, key: &str) -> Option<String> {
        sqlx::query_scalar("SELECT last_used_at FROM api_keys WHERE key_hash = ?")
            .bind(auth::hash_secret(key))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_password_still_works() {
        let pool = two_users_with_keys().await;

        let user = authenticate_password_or_api_key(&pool, "ana", "ana's password")
            .await
            .unwrap()
            .expect("the password is the right one");

        assert_eq!(user.username, "ana");
    }

    #[tokio::test]
    async fn an_api_key_works_where_the_password_goes() {
        let pool = two_users_with_keys().await;

        let user = authenticate_password_or_api_key(&pool, "ana", "ana's key")
            .await
            .unwrap()
            .expect("a key is accepted in place of the password");

        assert_eq!(user.username, "ana");
        assert!(
            last_used(&pool, "ana's key").await.is_some(),
            "using a key this way is still a use of it"
        );
    }

    /// The one case worth being strict about: the key is valid, so a lookup on
    /// its own would hand back an account nobody asked for.
    #[tokio::test]
    async fn somebody_elses_key_is_not_a_way_in() {
        let pool = two_users_with_keys().await;

        assert!(
            authenticate_password_or_api_key(&pool, "ana", "bob's key")
                .await
                .unwrap()
                .is_none(),
            "bob's key must not log anybody in as ana"
        );
        assert!(
            last_used(&pool, "bob's key").await.is_none(),
            "a key that opened nothing was not used"
        );
    }

    #[tokio::test]
    async fn a_wrong_secret_is_still_wrong() {
        let pool = two_users_with_keys().await;

        assert!(
            authenticate_password_or_api_key(&pool, "ana", "neither of the two")
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The panel is deliberately left out of this accommodation, so the check it
    /// uses must keep refusing a key.
    #[tokio::test]
    async fn a_key_does_not_open_the_panel() {
        let pool = two_users_with_keys().await;

        assert!(
            authenticate_password(&pool, "ana", "ana's key")
                .await
                .unwrap()
                .is_none()
        );
    }
}
