// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The panel logins an account has open, and closing them.
//!
//! Every other thing this API creates could already be taken back: a library
//! removed, an account deleted, a key revoked. A session could only be counted.
//! That left the one credential nobody could withdraw, which matters most in the
//! case a person is most likely to be in a hurry over — a browser left open
//! somewhere it should not have been.
//!
//! There is no token here, in or out. A session is pointed at by its row, so
//! ending somebody else's never needs the thing that would let you use it.

use super::error::ApiError;
use super::session::Panel;
use crate::session;
use crate::types::{Closed, ErrorBody, Login};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;

/// Anybody may manage their own sessions; only an administrator somebody else's.
///
/// Returns the account's own identifier, which is what the rows hang off.
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

/// List sessions
///
/// The panel logins an account has open. Yours, or anybody's if you administer
/// the server. Expired ones are not shown: they are not open.
#[utoipa::path(
    get,
    path = "/users/{username}/sessions",
    tag = "sessions",
    params(("username" = String, Path, description = "Whose sessions")),
    responses(
        (status = 200, description = "Every session still open", body = Vec<Login>),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's sessions", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn list(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Vec<Login>>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    // Expired rows are only cleared on the next login, so they can still be
    // sitting here. Showing one as an open session would be a lie.
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, created_at, last_seen_at, expires_at
           FROM sessions WHERE user_id = ? AND expires_at > ?
          ORDER BY created_at",
    )
    .bind(user_id)
    .bind(crate::db::now())
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing sessions"))?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, created_at, last_seen_at, expires_at)| Login {
                id,
                created_at,
                last_seen_at,
                expires_at,
                current: id == panel.id,
            })
            .collect(),
    ))
}

/// Close a session
///
/// Ends one login. Closing your own is the same as logging out, except that the
/// browser keeps a cookie that no longer resolves.
#[utoipa::path(
    delete,
    path = "/users/{username}/sessions/{id}",
    tag = "sessions",
    params(
        ("username" = String, Path, description = "Whose session"),
        ("id" = i64, Path, description = "Which one, from the listing"),
    ),
    responses(
        (status = 204, description = "Closed"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's session", body = ErrorBody),
        (status = 404, description = "No such account, or no such session of theirs", body = ErrorBody),
    )
)]
pub async fn close(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
) -> Result<StatusCode, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    // Scoped to the account named in the path, so an id belonging to somebody
    // else is a miss rather than a way to close a stranger's session.
    let done = sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "closing a session"))?;

    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Close every session
///
/// Ends all of the account's logins, this one included when the account is
/// yours. Everything, rather than everything else: somebody reaching for this
/// wants no way in left open, and one that survived because it happened to be
/// the one they clicked from would be the wrong answer.
#[utoipa::path(
    delete,
    path = "/users/{username}/sessions",
    tag = "sessions",
    params(("username" = String, Path, description = "Whose sessions")),
    responses(
        (status = 200, description = "How many were closed", body = Closed),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's sessions", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn close_all(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Closed>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let closed = session::destroy_all(&pool, user_id, None)
        .await
        .map_err(|e| ApiError::internal(e, "closing an account's sessions"))?;

    Ok(Json(Closed { closed }))
}
