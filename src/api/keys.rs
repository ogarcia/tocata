// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! API keys, for the clients that authenticate with one.
//!
//! Tocata has advertised the `apiKeyAuthentication` extension since the first
//! commit and had no way of issuing a key, which meant the only way to use it was
//! to write a row by hand. This is that way.
//!
//! A key is shown once, when it is made. What is stored is a SHA-256 of it, for
//! the same reason a password is stored hashed: recognising a key never needs the
//! plaintext, so there is no reason for the database to hold one.
//!
//! A key does not expire. It is held by a music player, and OpenSubsonic gives a
//! player no way to renew anything, so an expiry date would only mean the music
//! stopping one day with nothing the client could do. Revoking is the way one
//! ends, which is the trade the whole mechanism is for: a password changes and
//! every client is locked out at once, a key is withdrawn and only that one is.

use super::error::{ApiError, ErrorBody};
use super::session::Panel;
use crate::{auth, db};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

/// A key as it can be talked about afterwards, which is to say without the key.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    id: i64,
    /// What it is for, so one can be revoked without guessing.
    #[schema(example = "phone")]
    label: String,
    created_at: String,
    /// When a request last arrived with it. Null means it has never been used,
    /// which is the interesting case when something is not working.
    last_used_at: Option<String>,
}

/// A key at the one moment it can be read.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedKey {
    id: i64,
    label: String,
    created_at: String,
    /// The key itself. Not stored and not shown again: what the database keeps is
    /// a hash of it.
    #[schema(example = "3b1f...")]
    key: String,
}

/// What it takes to issue one.
#[derive(Deserialize, ToSchema)]
pub struct NewKey {
    /// A name for it. Defaults to something unhelpful on purpose, so that whoever
    /// makes several is nudged into naming them.
    #[schema(example = "phone")]
    label: Option<String>,
}

/// Stands in when no label was given.
const UNLABELLED: &str = "unnamed";

/// Anybody may manage their own keys; only an administrator somebody else's.
///
/// Returns the account's own identifier, since that is what the rows hang off and
/// what makes a rename of the account not touch them.
async fn owner(pool: &SqlitePool, panel: &Panel, username: &str) -> Result<i64, ApiError> {
    if panel.user.username != username && !panel.user.is_admin {
        return Err(ApiError::NotAuthorized);
    }

    sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
        .map_err(|e| ApiError::internal(e, "looking up an account"))?
        .ok_or(ApiError::NotFound)
}

/// List API keys
///
/// The keys issued to an account, without the keys. Yours, or anybody's if you
/// administer the server.
#[utoipa::path(
    get,
    path = "/users/{username}/keys",
    tag = "keys",
    params(("username" = String, Path, description = "Whose keys")),
    responses(
        (status = 200, description = "Every key the account has", body = Vec<Key>),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn list(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Vec<Key>>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, label, created_at, last_used_at
           FROM api_keys WHERE user_id = ? ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing API keys"))?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, label, created_at, last_used_at)| Key {
                id,
                label,
                created_at,
                last_used_at,
            })
            .collect(),
    ))
}

/// Issue an API key
///
/// Makes a key and returns it. This is the only time it is readable: what the
/// database keeps is a hash, so a key that gets lost is replaced, not recovered.
///
/// The key does not expire. It stops working when it is revoked, or when the
/// account it belongs to is deleted.
#[utoipa::path(
    post,
    path = "/users/{username}/keys",
    tag = "keys",
    params(("username" = String, Path, description = "Whose keys")),
    request_body = NewKey,
    responses(
        (status = 201, description = "The key, for the only time it can be read", body = IssuedKey),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn issue(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
    Json(new): Json<NewKey>,
) -> Result<(StatusCode, Json<IssuedKey>), ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let label = new
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .unwrap_or(UNLABELLED)
        .to_string();

    let key = auth::generate_token().map_err(|e| ApiError::internal(e, "generating an API key"))?;
    let created_at = db::now();

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO api_keys (user_id, key_hash, label, created_at)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(auth::hash_secret(&key))
    .bind(&label)
    .bind(&created_at)
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "issuing an API key"))?;

    Ok((
        StatusCode::CREATED,
        Json(IssuedKey {
            id,
            label,
            created_at,
            key,
        }),
    ))
}

/// Revoke an API key
///
/// Whatever was using it stops working at once.
#[utoipa::path(
    delete,
    path = "/users/{username}/keys/{id}",
    tag = "keys",
    params(
        ("username" = String, Path, description = "Whose keys"),
        ("id" = i64, Path, description = "Which key"),
    ),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account, or no such key of theirs", body = ErrorBody),
    )
)]
pub async fn revoke(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
) -> Result<StatusCode, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    // Both conditions, so naming somebody else's key by its number revokes
    // nothing rather than revoking theirs.
    let deleted = sqlx::query("DELETE FROM api_keys WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "revoking an API key"))?;

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
