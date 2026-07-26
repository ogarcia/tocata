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

/// Resolves an API key to its owner and records the use.
pub async fn authenticate_api_key(pool: &SqlitePool, key: &str) -> Result<Option<User>> {
    let key_hash = auth::hash_secret(key);

    let row: Option<(i64, i64, String, bool)> = sqlx::query_as(
        "SELECT k.id, u.id, u.username, u.is_admin
           FROM api_keys k
           JOIN users u ON u.id = k.user_id
          WHERE k.key_hash = ?",
    )
    .bind(&key_hash)
    .fetch_optional(pool)
    .await
    .context("looking up API key")?;

    let Some((key_id, id, username, is_admin)) = row else {
        return Ok(None);
    };

    sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
        .bind(now())
        .bind(key_id)
        .execute(pool)
        .await
        .context("recording API key use")?;

    Ok(Some(User {
        id,
        username,
        is_admin,
    }))
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
