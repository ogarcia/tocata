// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Accounts.
//!
//! Two things nobody can do, whatever their rights: delete themselves, or take
//! their own administrator rights away. That pair is what guarantees the server
//! never runs out of administrators, and it holds without counting anything —
//! only an administrator can reach these calls, and they cannot touch
//! themselves, so whoever made the change is still one afterwards. A rule that
//! counted the remaining administrators would need a lock, because two of them
//! demoting each other at the same moment would each see the other and leave
//! none.

use super::auth::Authenticated;
use super::error::ApiError;
use super::response::{self, Empty};
use crate::{auth, db};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::error;

#[derive(Debug, Deserialize)]
pub struct UsernameQuery {
    username: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserQuery {
    username: String,
    password: String,
    email: Option<String>,
    admin_role: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserQuery {
    username: String,
    password: Option<String>,
    email: Option<String>,
    admin_role: Option<bool>,
    scrobbling_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordQuery {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct UserBody {
    user: User,
}

#[derive(Serialize)]
struct UsersBody {
    users: Users,
}

#[derive(Serialize)]
struct Users {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    user: Vec<User>,
}

/// An account as the API describes it.
///
/// The roles are not columns in a table: they say what this server can do for
/// this person. Streaming and downloading are true for everybody because that is
/// what a music server is for; podcasts, the jukebox, sharing and video
/// conversion are false because Tocata does none of them, and claiming otherwise
/// would have clients offer buttons that lead nowhere.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct User {
    username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    scrobbling_enabled: bool,
    admin_role: bool,
    settings_role: bool,
    download_role: bool,
    upload_role: bool,
    playlist_role: bool,
    cover_art_role: bool,
    comment_role: bool,
    podcast_role: bool,
    stream_role: bool,
    jukebox_role: bool,
    share_role: bool,
    video_conversion_role: bool,
}

impl User {
    fn new(username: String, email: Option<String>, is_admin: bool, scrobbling: bool) -> Self {
        Self {
            username,
            email,
            scrobbling_enabled: scrobbling,
            admin_role: is_admin,
            // Changing server settings is administration by another name.
            settings_role: is_admin,
            download_role: true,
            stream_role: true,
            playlist_role: true,
            cover_art_role: true,
            // Nothing here accepts uploads, comments or the rest.
            upload_role: false,
            comment_role: false,
            podcast_role: false,
            jukebox_role: false,
            share_role: false,
            video_conversion_role: false,
        }
    }
}

type UserRow = (String, Option<String>, bool, bool);

/// A macro rather than a constant so `concat!` builds each statement at compile
/// time: sqlx does not take SQL assembled at runtime.
macro_rules! user_columns {
    () => {
        "SELECT username, email, is_admin, scrobbling_enabled FROM users"
    };
}

pub async fn get_user(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<UsernameQuery>,
) -> Response {
    // Anybody may ask about themselves; only an administrator about others.
    if query.username != auth.user.username && !auth.user.is_admin {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    let row: Result<Option<UserRow>, _> =
        sqlx::query_as(concat!(user_columns!(), " WHERE username = ?"))
            .bind(&query.username)
            .fetch_optional(&pool)
            .await;

    match row {
        Ok(Some((username, email, is_admin, scrobbling))) => response::ok(
            auth.format,
            UserBody {
                user: User::new(username, email, is_admin, scrobbling),
            },
        ),
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "loading a user"),
    }
}

pub async fn get_users(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    if !auth.user.is_admin {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    let rows: Result<Vec<UserRow>, _> =
        sqlx::query_as(concat!(user_columns!(), " ORDER BY username"))
            .fetch_all(&pool)
            .await;

    match rows {
        Ok(rows) => response::ok(
            auth.format,
            UsersBody {
                users: Users {
                    user: rows
                        .into_iter()
                        .map(|(username, email, is_admin, scrobbling)| {
                            User::new(username, email, is_admin, scrobbling)
                        })
                        .collect(),
                },
            },
        ),
        Err(e) => internal(e, auth.format, "listing users"),
    }
}

pub async fn create_user(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<CreateUserQuery>,
) -> Response {
    if !auth.user.is_admin {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    if query.username.trim().is_empty() {
        return ApiError::MissingParameter("username")
            .in_format(auth.format)
            .into_response();
    }

    let Some(password) = auth::decode_password(&query.password) else {
        return ApiError::MissingParameter("password")
            .in_format(auth.format)
            .into_response();
    };

    let hash = match auth::hash_password(&password) {
        Ok(hash) => hash,
        Err(e) => {
            error!("hashing a new user's password: {e:#}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };

    let timestamp = db::now();
    let written = sqlx::query(
        "INSERT INTO users (username, password_hash, email, is_admin, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(query.username.trim())
    .bind(&hash)
    .bind(&query.email)
    .bind(i64::from(query.admin_role.unwrap_or(false)))
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await;

    match written {
        Ok(_) => response::ok(auth.format, Empty {}),
        // The username is unique, so the likely failure is that it is taken.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            ApiError::Generic("A user with that name already exists".into())
                .in_format(auth.format)
                .into_response()
        }
        Err(e) => internal(e, auth.format, "creating a user"),
    }
}

pub async fn update_user(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<UpdateUserQuery>,
) -> Response {
    if !auth.user.is_admin {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    let touching_self = query.username == auth.user.username;

    // The first of the two rules. Everything else about themselves an
    // administrator may change; not this.
    if touching_self && query.admin_role == Some(false) {
        return ApiError::Generic(
            "An administrator cannot take their own administrator rights away".into(),
        )
        .in_format(auth.format)
        .into_response();
    }

    match apply_update(&pool, &query).await {
        Ok(true) => response::ok(auth.format, Empty {}),
        Ok(false) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "updating a user"),
    }
}

pub async fn delete_user(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<UsernameQuery>,
) -> Response {
    if !auth.user.is_admin {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    // The second rule. Somebody who wants their own account gone asks another
    // administrator, which is a small inconvenience next to a server nobody can
    // administer.
    if query.username == auth.user.username {
        return ApiError::Generic("An administrator cannot delete their own account".into())
            .in_format(auth.format)
            .into_response();
    }

    let deleted = sqlx::query("DELETE FROM users WHERE username = ?")
        .bind(&query.username)
        .execute(&pool)
        .await;

    match deleted {
        Ok(result) if result.rows_affected() > 0 => response::ok(auth.format, Empty {}),
        Ok(_) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "deleting a user"),
    }
}

/// Changes a password: your own, or anybody's if you administer the server.
pub async fn change_password(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<ChangePasswordQuery>,
) -> Response {
    if query.username != auth.user.username && !auth.user.is_admin {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    let Some(password) = auth::decode_password(&query.password) else {
        return ApiError::MissingParameter("password")
            .in_format(auth.format)
            .into_response();
    };

    let hash = match auth::hash_password(&password) {
        Ok(hash) => hash,
        Err(e) => {
            error!("hashing a changed password: {e:#}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };

    let written =
        sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE username = ?")
            .bind(&hash)
            .bind(db::now())
            .bind(&query.username)
            .execute(&pool)
            .await;

    match written {
        Ok(result) if result.rows_affected() > 0 => response::ok(auth.format, Empty {}),
        Ok(_) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "changing a password"),
    }
}

/// Applies whatever the request named, leaving the rest alone. `false` when
/// there is no such user.
async fn apply_update(pool: &SqlitePool, query: &UpdateUserQuery) -> Result<bool, sqlx::Error> {
    let Some(id): Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(&query.username)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(false);
    };

    let mut tx = pool.begin().await?;

    if let Some(email) = &query.email {
        sqlx::query("UPDATE users SET email = ? WHERE id = ?")
            .bind(email)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    if let Some(is_admin) = query.admin_role {
        sqlx::query("UPDATE users SET is_admin = ? WHERE id = ?")
            .bind(i64::from(is_admin))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    if let Some(scrobbling) = query.scrobbling_enabled {
        sqlx::query("UPDATE users SET scrobbling_enabled = ? WHERE id = ?")
            .bind(i64::from(scrobbling))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    if let Some(password) = &query.password {
        // Only fails if the system RNG does, which is not a database error but
        // has to travel as one from here.
        let Some(decoded) = auth::decode_password(password) else {
            return Ok(false);
        };
        let Ok(hash) = auth::hash_password(&decoded) else {
            error!("hashing a password during an update");
            return Err(sqlx::Error::WorkerCrashed);
        };

        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(&hash)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query("UPDATE users SET updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(true)
}

fn internal(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}
