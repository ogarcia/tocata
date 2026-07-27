// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Panel logins.
//!
//! The panel runs in a browser, which rules out the mechanisms `/rest` offers. A
//! password in a query string ends up in access logs, shell history and referrer
//! headers, and a bearer token cannot be used at all with server sent events,
//! because `EventSource` has no way to set a header. A cookie is sent by the
//! browser without being handed to the page's own scripts, which is both what
//! makes the stream work and what keeps the token out of reach of anything
//! injected into the page.

use crate::auth;
use crate::db;
use crate::user::User;
use anyhow::{Context, Result};
use chrono::{Duration, SecondsFormat, Utc};
use sqlx::SqlitePool;

/// How long a login lasts. Absolute: see the schema for why it does not slide.
const LIFETIME_DAYS: i64 = 30;

/// How stale `last_seen_at` is allowed to get before it is written again.
///
/// Recording every request would mean a write per request for a column nobody
/// reads more than once a minute.
const LAST_SEEN_RESOLUTION_MINUTES: i64 = 5;

/// A resolved session: which one it is, who it belongs to, and until when.
pub struct Session {
    /// Its row, which is how a session can be pointed at without the token that
    /// opens it: to say "this one is the one you are using", or to spare it when
    /// the rest are being closed.
    pub id: i64,
    pub user: User,
    pub expires_at: String,
}

/// A moment relative to now, in the shape the schema stores.
fn from_now(offset: Duration) -> String {
    (Utc::now() + offset).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Logs somebody in, returning the token for their cookie and when it runs out.
///
/// Expired rows are cleared here rather than on a timer: the table only grows
/// when somebody logs in, so that is the moment worth tidying, and a server
/// nobody touches needs no upkeep at all.
pub async fn create(pool: &SqlitePool, user_id: i64) -> Result<(String, String)> {
    let token = auth::generate_token()?;
    let timestamp = db::now();
    let expires_at = from_now(Duration::days(LIFETIME_DAYS));

    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(&timestamp)
        .execute(pool)
        .await
        .context("clearing expired sessions")?;

    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, created_at, last_seen_at, expires_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(auth::hash_secret(&token))
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(&expires_at)
    .execute(pool)
    .await
    .context("creating a session")?;

    Ok((token, expires_at))
}

/// Resolves a token to whoever holds it, or `None` if it is unknown or expired.
pub async fn resolve(pool: &SqlitePool, token: &str) -> Result<Option<Session>> {
    let row: Option<(i64, String, String, i64, String, bool)> = sqlx::query_as(
        "SELECT s.id, s.expires_at, s.last_seen_at, u.id, u.username, u.is_admin
           FROM sessions s
           JOIN users u ON u.id = s.user_id
          WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(auth::hash_secret(token))
    .bind(db::now())
    .fetch_optional(pool)
    .await
    .context("looking up a session")?;

    let Some((id, expires_at, last_seen_at, user_id, username, is_admin)) = row else {
        return Ok(None);
    };

    // Only once it has gone stale, which is what the resolution is for.
    if last_seen_at < from_now(-Duration::minutes(LAST_SEEN_RESOLUTION_MINUTES)) {
        sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE id = ?")
            .bind(db::now())
            .bind(id)
            .execute(pool)
            .await
            .context("recording session use")?;
    }

    Ok(Some(Session {
        id,
        user: User {
            id: user_id,
            username,
            is_admin,
        },
        expires_at,
    }))
}

/// Ends one session. Logging out of one browser leaves the others alone.
pub async fn destroy(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(auth::hash_secret(token))
        .execute(pool)
        .await
        .context("ending a session")?;

    Ok(())
}

/// Ends every session an account has, except optionally the one asking.
///
/// Sparing the caller's own is what makes this usable while changing a password:
/// somebody who just proved who they are should not be thrown out along with
/// whoever they are throwing out. Pass `None` and nothing is spared.
pub async fn destroy_all(pool: &SqlitePool, user_id: i64, except: Option<i64>) -> Result<u64> {
    // `?` never matches a row when the value is NULL, which is exactly the
    // "spare nothing" case, so one statement covers both.
    let done = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id IS NOT ?")
        .bind(user_id)
        .bind(except)
        .execute(pool)
        .await
        .context("ending an account's sessions")?;

    Ok(done.rows_affected())
}

/// Seconds a freshly issued session is good for, for the cookie's `Max-Age`.
pub fn lifetime_seconds() -> i64 {
    Duration::days(LIFETIME_DAYS).num_seconds()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two accounts with two sessions each, so that "all of them" can be told
    /// apart from "all of this one's".
    async fn two_users_logged_in_twice() -> (SqlitePool, i64, i64) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let timestamp = db::now();
        let mut ids = Vec::new();

        for name in ["ana", "bob"] {
            let user_id: i64 = sqlx::query_scalar(
                "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
                 VALUES (?, 'x', 0, ?, ?) RETURNING id",
            )
            .bind(name)
            .bind(&timestamp)
            .bind(&timestamp)
            .fetch_one(&pool)
            .await
            .unwrap();

            for _ in 0..2 {
                create(&pool, user_id).await.unwrap();
            }

            ids.push(user_id);
        }

        (pool, ids[0], ids[1])
    }

    async fn count(pool: &SqlitePool, user_id: i64) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// `IS NOT` rather than `!=` is the whole trick, and the difference only
    /// shows with NULL: `id != NULL` is never true, so a plain comparison would
    /// spare every row instead of none and this would quietly close nothing.
    #[tokio::test]
    async fn sparing_nothing_closes_everything() {
        let (pool, ana, bob) = two_users_logged_in_twice().await;

        let closed = destroy_all(&pool, ana, None).await.unwrap();

        assert_eq!(closed, 2);
        assert_eq!(count(&pool, ana).await, 0);
        assert_eq!(count(&pool, bob).await, 2, "nobody else was touched");
    }

    #[tokio::test]
    async fn one_session_can_be_spared() {
        let (pool, ana, _) = two_users_logged_in_twice().await;

        let keep: i64 = sqlx::query_scalar("SELECT min(id) FROM sessions WHERE user_id = ?")
            .bind(ana)
            .fetch_one(&pool)
            .await
            .unwrap();

        let closed = destroy_all(&pool, ana, Some(keep)).await.unwrap();

        assert_eq!(closed, 1);
        let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = ?")
            .bind(ana)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left, vec![keep]);
    }

    /// Sparing a session of somebody else's is not sparing one of theirs, which
    /// is what makes one rule serve both a self service change and an
    /// administrator changing a password for somebody.
    #[tokio::test]
    async fn sparing_a_stranger_spares_nothing_here() {
        let (pool, ana, bob) = two_users_logged_in_twice().await;

        let bobs: i64 = sqlx::query_scalar("SELECT min(id) FROM sessions WHERE user_id = ?")
            .bind(bob)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(destroy_all(&pool, ana, Some(bobs)).await.unwrap(), 2);
        assert_eq!(count(&pool, ana).await, 0);
        assert_eq!(count(&pool, bob).await, 2);
    }
}
