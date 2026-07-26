// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Accounts.
//!
//! Two things nobody can do to themselves, whatever their rights: delete their
//! account, or take away their own administrator rights. That pair is what
//! guarantees the server never runs out of administrators, and it holds without
//! counting anything — only an administrator gets this far, and none of them can
//! touch themselves, so whoever made the change is still one afterwards. A rule
//! that counted the remaining administrators would need a lock, because two of
//! them demoting each other at the same moment would each see the other and
//! leave none.
//!
//! Renaming is allowed, unlike in `/rest` where the protocol has no call for it.
//! Nothing here keys on a username except the username column: sessions, keys,
//! favourites and playlists all hang off the row's own identifier, so a rename is
//! a rename and not a migration.
//!
//! Which libraries an account may see lives here too. No rows means all of them,
//! which is the ordinary case and the one that costs nothing.

use super::error::{ApiError, ErrorBody};
use super::session::{Administrator, Panel};
use crate::{auth, db};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

/// Which libraries an account is restricted to.
#[derive(Deserialize, ToSchema)]
pub struct LibraryAccess {
    /// Identifiers of the libraries this account may see. An empty list removes
    /// the restriction, which is not the same as seeing nothing: an account with
    /// no restriction sees every library that is switched on.
    libraries: Vec<i64>,
}

/// An account, as somebody entitled to see it may.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    #[schema(example = "admin")]
    username: String,
    email: Option<String>,
    /// Whether this account administers the server.
    admin: bool,
    /// Whether plays from this account are passed on to a scrobbling service.
    scrobbling: bool,
    /// Sessions logged in and not yet expired. What tells an administrator that
    /// an account is in use before they remove it.
    sessions: i64,
    /// API keys issued to this account and not revoked.
    keys: i64,
    /// Libraries this account is restricted to. Empty means no restriction, so
    /// every library that is switched on.
    libraries: Vec<i64>,
    created_at: String,
    updated_at: String,
}

type AccountRow = (
    String,
    Option<String>,
    bool,
    bool,
    i64,
    i64,
    Option<String>,
    String,
    String,
);

impl From<AccountRow> for Account {
    fn from(
        (username, email, admin, scrobbling, sessions, keys, libraries, created_at, updated_at): AccountRow,
    ) -> Self {
        Self {
            username,
            email,
            admin,
            scrobbling,
            sessions,
            keys,
            // group_concat gives back what we put in, so these parse or the
            // database has been edited by hand.
            libraries: libraries
                .unwrap_or_default()
                .split(',')
                .filter_map(|id| id.parse().ok())
                .collect(),
            created_at,
            updated_at,
        }
    }
}

/// A macro rather than a constant so `concat!` builds each statement at compile
/// time: sqlx does not take SQL assembled at runtime.
///
/// The expiry is compared against a bound timestamp rather than SQLite's
/// `datetime('now')`, which writes `2026-07-26 19:00:00` where the schema stores
/// `2026-07-26T19:00:00Z`. Compared as text the second is always the greater of
/// the two, so every session would have counted as live.
macro_rules! account_columns {
    () => {
        "SELECT u.username, u.email, u.is_admin, u.scrobbling_enabled,
                (SELECT count(*) FROM sessions s
                  WHERE s.user_id = u.id AND s.expires_at > ?),
                (SELECT count(*) FROM api_keys k WHERE k.user_id = u.id),
                (SELECT group_concat(ul.library_id) FROM user_libraries ul
                  WHERE ul.user_id = u.id),
                u.created_at, u.updated_at
           FROM users u"
    };
}

/// What it takes to create an account.
#[derive(Deserialize, ToSchema)]
pub struct NewAccount {
    #[schema(example = "oscar")]
    username: String,
    password: String,
    email: Option<String>,
    /// Defaults to false. An account that administers nothing is the safe thing
    /// to create by accident.
    #[serde(default)]
    admin: bool,
}

/// What may be changed. Anything left out is left alone.
#[derive(Deserialize, ToSchema)]
pub struct AccountChanges {
    /// A new name for the account. Nothing else has to move with it.
    #[schema(example = "oscar")]
    username: Option<String>,
    password: Option<String>,
    email: Option<String>,
    /// Only an administrator may set this, and none may clear their own.
    admin: Option<bool>,
    scrobbling: Option<bool>,
}

/// Anybody may look at their own account; only an administrator at somebody
/// else's.
fn self_or_admin(panel: &Panel, username: &str) -> Result<(), ApiError> {
    if panel.user.username == username || panel.user.is_admin {
        Ok(())
    } else {
        Err(ApiError::NotAuthorized)
    }
}

/// List accounts
///
/// Every account, with how many live sessions and API keys each has.
#[utoipa::path(
    get,
    path = "/users",
    tag = "users",
    responses(
        (status = 200, description = "Every account there is", body = Vec<Account>),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn list(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<Account>>, ApiError> {
    let rows: Vec<AccountRow> = sqlx::query_as(concat!(account_columns!(), " ORDER BY u.username"))
        .bind(db::now())
        .fetch_all(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "listing accounts"))?;

    Ok(Json(rows.into_iter().map(Account::from).collect()))
}

/// One account
///
/// Your own, or anybody's if you administer the server.
#[utoipa::path(
    get,
    path = "/users/{username}",
    tag = "users",
    params(("username" = String, Path, description = "Whose account")),
    responses(
        (status = 200, description = "The account", body = Account),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's account", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn one(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Account>, ApiError> {
    self_or_admin(&panel, &username)?;
    Ok(Json(load(&pool, &username).await?))
}

/// Create an account
#[utoipa::path(
    post,
    path = "/users",
    tag = "users",
    request_body = NewAccount,
    responses(
        (status = 201, description = "Created", body = Account),
        (status = 400, description = "The name or password is empty", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 409, description = "That name is taken", body = ErrorBody),
    )
)]
pub async fn create(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    Json(new): Json<NewAccount>,
) -> Result<(StatusCode, Json<Account>), ApiError> {
    let username = new.username.trim();
    if username.is_empty() {
        return Err(ApiError::Invalid("The username cannot be empty"));
    }
    if new.password.is_empty() {
        return Err(ApiError::Invalid("The password cannot be empty"));
    }

    let hash = auth::hash_password(&new.password)
        .map_err(|e| ApiError::internal(e, "hashing a new account's password"))?;

    let timestamp = db::now();
    let written = sqlx::query(
        "INSERT INTO users (username, password_hash, email, is_admin, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(username)
    .bind(&hash)
    .bind(&new.email)
    .bind(i64::from(new.admin))
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await;

    match written {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::Conflict("An account with that name exists"));
        }
        Err(e) => return Err(ApiError::internal(e, "creating an account")),
    }

    Ok((StatusCode::CREATED, Json(load(&pool, username).await?)))
}

/// Change an account
///
/// Your own, or anybody's if you administer the server. Renaming is a rename:
/// sessions, keys and everything the account owns follow it.
#[utoipa::path(
    patch,
    path = "/users/{username}",
    tag = "users",
    params(("username" = String, Path, description = "Whose account")),
    request_body = AccountChanges,
    responses(
        (status = 200, description = "The account as it now is", body = Account),
        (status = 400, description = "Nothing worth changing was asked for", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's account, or a right you do not have", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
        (status = 409, description = "The new name is taken", body = ErrorBody),
    )
)]
pub async fn change(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
    Json(changes): Json<AccountChanges>,
) -> Result<Json<Account>, ApiError> {
    self_or_admin(&panel, &username)?;

    let touching_self = panel.user.username == username;

    // Handing out administrator rights is administration. Without this, anybody
    // could promote themselves by editing their own account.
    if changes.admin.is_some() && !panel.user.is_admin {
        return Err(ApiError::NotAuthorized);
    }

    // The first of the two rules. Everything else about themselves an
    // administrator may change; not this.
    if touching_self && changes.admin == Some(false) {
        return Err(ApiError::Conflict(
            "An administrator cannot take away their own administrator rights",
        ));
    }

    let new_name = changes
        .username
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let hash = match changes.password.as_deref() {
        Some(password) if !password.is_empty() => Some(
            auth::hash_password(password)
                .map_err(|e| ApiError::internal(e, "hashing a changed password"))?,
        ),
        _ => None,
    };

    if new_name.is_none()
        && hash.is_none()
        && changes.email.is_none()
        && changes.admin.is_none()
        && changes.scrobbling.is_none()
    {
        return Err(ApiError::Invalid("Nothing to change was given"));
    }

    // Coalesce rather than assembling the statement: sqlx will not take SQL built
    // at runtime, and a null bind meaning "leave it" says the same in one query
    // instead of one per field.
    let changed = sqlx::query(
        "UPDATE users
            SET username = coalesce(?, username),
                password_hash = coalesce(?, password_hash),
                email = coalesce(?, email),
                is_admin = coalesce(?, is_admin),
                scrobbling_enabled = coalesce(?, scrobbling_enabled),
                updated_at = ?
          WHERE username = ?",
    )
    .bind(new_name)
    .bind(&hash)
    .bind(&changes.email)
    .bind(changes.admin.map(i64::from))
    .bind(changes.scrobbling.map(i64::from))
    .bind(db::now())
    .bind(&username)
    .execute(&pool)
    .await;

    match changed {
        Ok(result) if result.rows_affected() == 0 => return Err(ApiError::NotFound),
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::Conflict("An account with that name exists"));
        }
        Err(e) => return Err(ApiError::internal(e, "changing an account")),
    }

    Ok(Json(load(&pool, new_name.unwrap_or(&username)).await?))
}

/// Delete an account
///
/// Takes the account and everything only it had: its sessions, its API keys, its
/// favourites, ratings, play counts, playlists and bookmarks.
///
/// The second of the two rules is here. Somebody who wants their own account gone
/// asks another administrator, which is a small inconvenience next to a server
/// nobody can administer.
#[utoipa::path(
    delete,
    path = "/users/{username}",
    tag = "users",
    params(("username" = String, Path, description = "Whose account")),
    responses(
        (status = 204, description = "Gone, along with everything only it had"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
        (status = 409, description = "Your own account", body = ErrorBody),
    )
)]
pub async fn delete(
    admin: Administrator,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<StatusCode, ApiError> {
    if admin.user.username == username {
        return Err(ApiError::Conflict(
            "An administrator cannot delete their own account",
        ));
    }

    let deleted = sqlx::query("DELETE FROM users WHERE username = ?")
        .bind(&username)
        .execute(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "deleting an account"))?;

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Restrict an account to some libraries
///
/// Replaces whatever restriction the account had. An empty list means no
/// restriction: every library that is switched on, which is how an account starts
/// and how most stay.
///
/// A restriction is enforced everywhere, not merely in the list of folders a
/// client is offered. Anything the account may not see is absent from browsing,
/// from both searches, from the album lists, from its own playlists, and cannot
/// be streamed or have its cover fetched by naming an identifier directly.
#[utoipa::path(
    put,
    path = "/users/{username}/libraries",
    tag = "users",
    params(("username" = String, Path, description = "Whose access")),
    request_body = LibraryAccess,
    responses(
        (status = 200, description = "The account as it now is", body = Account),
        (status = 400, description = "One of those libraries does not exist", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn restrict(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
    Json(access): Json<LibraryAccess>,
) -> Result<Json<Account>, ApiError> {
    let user_id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(&username)
        .fetch_optional(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "looking up an account to restrict"))?;

    let Some(user_id) = user_id else {
        return Err(ApiError::NotFound);
    };

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::internal(e, "restricting an account"))?;

    // Replaced whole rather than merged: the request says what the restriction is,
    // not what to add to it, so there is no order in which two calls disagree.
    sqlx::query("DELETE FROM user_libraries WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::internal(e, "clearing a restriction"))?;

    for library_id in &access.libraries {
        let written = sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, ?)")
            .bind(user_id)
            .bind(library_id)
            .execute(&mut *tx)
            .await;

        match written {
            Ok(_) => {}
            // The foreign key is what catches a library that is not there, so
            // there is no separate lookup to race against.
            Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
                return Err(ApiError::Invalid("No such library"));
            }
            Err(e) => return Err(ApiError::internal(e, "restricting an account")),
        }
    }

    tx.commit()
        .await
        .map_err(|e| ApiError::internal(e, "restricting an account"))?;

    Ok(Json(load(&pool, &username).await?))
}

/// Reads one back, for the handlers that answer with what they just wrote.
async fn load(pool: &SqlitePool, username: &str) -> Result<Account, ApiError> {
    let row: Option<AccountRow> =
        sqlx::query_as(concat!(account_columns!(), " WHERE u.username = ?"))
            .bind(db::now())
            .bind(username)
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::internal(e, "loading an account"))?;

    row.map(Account::from).ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session count is the one figure here that compares timestamps, and it
    /// compares them as text.
    ///
    /// The expired one is midnight of the current day rather than some safely
    /// distant past, because that is the only shape that catches the mistake this
    /// guards against. Against SQLite's own `datetime('now')` — which writes
    /// `2026-07-26 20:00:00` where the schema writes `2026-07-26T20:00:00Z` — the
    /// two strings agree until the separator, and `'T' > ' '`, so a session that
    /// ran out this morning counts as live. A date years ago differs earlier and
    /// compares correctly either way, which is why it would prove nothing.
    #[tokio::test]
    async fn a_session_that_ran_out_today_is_not_counted_as_live() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let timestamp = db::now();
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('someone', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        // Midnight today, and a date nothing will outlive.
        let midnight = format!("{}T00:00:00Z", &timestamp[..10]);

        for expires_at in [midnight.as_str(), "2999-01-01T00:00:00Z"] {
            sqlx::query(
                "INSERT INTO sessions (user_id, token_hash, created_at, last_seen_at, expires_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(expires_at)
            .bind(&timestamp)
            .bind(&timestamp)
            .bind(expires_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let account = load(&pool, "someone").await.unwrap();
        assert_eq!(account.sessions, 1, "only the one that has not run out");
        assert_eq!(account.keys, 0);
    }
}
