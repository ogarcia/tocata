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

/// A resolved session: who it belongs to, and until when.
pub struct Session {
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

/// Seconds a freshly issued session is good for, for the cookie's `Max-Age`.
pub fn lifetime_seconds() -> i64 {
    Duration::days(LIFETIME_DAYS).num_seconds()
}
