// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Users and how a request proves it is one of them.
//!
//! Verification lives here rather than beside the request handling so the
//! stored hash never leaves this module.

use crate::auth;
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
            if auth::verify_password(password, &hash) {
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

/// Notes that a key just let a request through, for the panel to show.
async fn record_api_key_use(pool: &SqlitePool, key_id: i64) -> Result<()> {
    sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
        .bind(now())
        .bind(key_id)
        .execute(pool)
        .await
        .context("recording API key use")?;

    Ok(())
}

/// Resolves an API key to its owner and records the use.
pub async fn authenticate_api_key(pool: &SqlitePool, key: &str) -> Result<Option<User>> {
    let Some((key_id, user)) = lookup_api_key(pool, key).await? else {
        return Ok(None);
    };

    record_api_key_use(pool, key_id).await?;

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

    record_api_key_use(pool, key_id).await?;

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
    .execute(pool)
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
