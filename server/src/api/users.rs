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

use super::error::ApiError;
use super::session::{Administrator, Panel};
use crate::types::{Account, AccountChanges, ErrorBody, Holdings, LibraryAccess, NewAccount};
use crate::{auth, db, session, user};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;

type AccountRow = (
    String,
    Option<String>,
    Option<String>,
    bool,
    bool,
    String,
    Option<String>,
    i64,
    i64,
    Option<String>,
    String,
    String,
);

impl From<AccountRow> for Account {
    fn from(
        (
            username,
            display_name,
            email,
            admin,
            scrobbling,
            password_set_at,
            last_seen_at,
            sessions,
            keys,
            libraries,
            created_at,
            updated_at,
        ): AccountRow,
    ) -> Self {
        Self {
            username,
            display_name,
            email,
            admin,
            scrobbling,
            password_set_at,
            last_seen_at,
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
        "SELECT u.username, u.display_name, u.email, u.is_admin, u.scrobbling_enabled,
                u.password_set_at,
                u.last_seen_at,
                (SELECT count(*) FROM sessions s
                  WHERE s.user_id = u.id AND s.expires_at > ?),
                (SELECT count(*) FROM api_keys k
                  WHERE k.user_id = u.id AND k.revoked_at IS NULL),
                (SELECT group_concat(ul.library_id) FROM user_libraries ul
                  WHERE ul.user_id = u.id),
                u.created_at, u.updated_at
           FROM users u"
    };
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
/// Your own, or anybody's if you administer the server.
///
/// Renaming is for administrators only — of anybody's account, including their own —
/// and it is a rename rather than a new account: sessions, keys and everything the
/// account owns follow it. What a listener may change about themselves is their
/// address, their password and their preferences.
///
/// Changing the password closes the account's other panel sessions, keeping only
/// the one that asked. API keys are left alone, since revoking one of those is
/// its own deliberate act.
///
/// Changing your own name, address or password needs `currentPassword` as well: a
/// live session is not proof that the person sitting there is the owner, and those
/// three are what would lock the owner out. An administrator changing somebody
/// else's account is not asked for it — they do not have it.
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
        (status = 403, description = "Somebody else's account, a rename or a right you may not grant yourself, or the wrong current password", body = ErrorBody),
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

    // Renaming is administration, wherever it is done from and whoever's account it
    // is. A name is how an administrator knows who somebody is, and it is what every
    // OpenSubsonic client authenticates with — so a listener who renamed themselves
    // stopped every player they own from logging in, without being told and without
    // being able to put it back the way it was under a name somebody else may have
    // taken meanwhile.
    //
    // An administrator renaming themselves is fine, and so is an administrator
    // renaming anybody else. What is refused is a listener renaming a listener, which
    // in practice is themselves: they cannot reach anybody else's account at all.
    //
    // Only when it is really a change. The panel sends the whole form on every save,
    // so a listener changing their address sends their own name along with it, and
    // refusing "rename yourself to what you are already called" would be refusing a
    // change of address for the sake of a rename nobody asked for.
    if let Some(wanted) = new_name
        && wanted != username
        && !panel.user.is_admin
    {
        return Err(ApiError::NotAuthorized);
    }

    let hash = match changes.password.as_deref() {
        Some(password) if !password.is_empty() => Some(
            auth::hash_password(password)
                .map_err(|e| ApiError::internal(e, "hashing a changed password"))?,
        ),
        _ => None,
    };

    // What to be called, or nothing to go back to being called by the account's
    // name. Emptying the field is a request rather than an omission, so it travels as
    // an empty string and arrives here as the mention of a value that is `None`.
    let shown_as = changes
        .display_name
        .as_deref()
        .map(|given| given.trim())
        .map(|given| (!given.is_empty()).then(|| given.to_string()));

    if new_name.is_none()
        && hash.is_none()
        && shown_as.is_none()
        && changes.email.is_none()
        && changes.admin.is_none()
        && changes.scrobbling.is_none()
    {
        return Err(ApiError::Invalid("Nothing to change was given"));
    }

    // What somebody would have to undo by asking an administrator: the name they
    // log in with, the address a lost password would be recovered through, and the
    // password itself. Everything else about an account is a preference, and a
    // preference set by mistake is set back by hand.
    //
    // Only on your own account. An administrator does not have somebody else's
    // password, and the account they could lock out is not the one they are sitting
    // in front of — theirs is protected by this same rule when they change it.
    let sensitive = new_name.is_some() || hash.is_some() || changes.email.is_some();

    if sensitive && touching_self {
        let given = changes
            .current_password
            .as_deref()
            .filter(|given| !given.is_empty())
            .ok_or(ApiError::Invalid(
                "Changing your own name, address or password needs the current password",
            ))?;

        // The password and only the password. An API key stands in for one when a
        // music player logs in, and it must not stand in for one here: a key is
        // held by whatever was set up with it, and the point of asking is that
        // somebody is there to answer.
        if user::authenticate_password(&pool, &username, given)
            .await
            .map_err(|e| ApiError::internal(e, "checking the current password"))?
            .is_none()
        {
            return Err(ApiError::WrongPassword);
        }
    }

    // Coalesce rather than assembling the statement: sqlx will not take SQL built
    // at runtime, and a null bind meaning "leave it" says the same in one query
    // instead of one per field.
    let now = db::now();

    let changed = sqlx::query(
        "UPDATE users
            SET username = coalesce(?, username),
                password_hash = coalesce(?, password_hash),
                -- Moves with the hash and with nothing else. `updated_at` below
                -- moves for a change of address just as readily, which is why it
                -- cannot answer when the password was last changed.
                password_set_at = CASE WHEN ? IS NULL THEN password_set_at ELSE ? END,
                -- Not a coalesce, unlike its neighbours: null here is a value
                -- somebody asked for, meaning stop calling me that, and a coalesce
                -- cannot tell it from the field having been left out. So the
                -- mention travels as its own bind.
                display_name = CASE WHEN ? THEN ? ELSE display_name END,
                email = coalesce(?, email),
                is_admin = coalesce(?, is_admin),
                scrobbling_enabled = coalesce(?, scrobbling_enabled),
                updated_at = ?
          WHERE username = ?",
    )
    .bind(new_name)
    .bind(&hash)
    .bind(&hash)
    .bind(&now)
    .bind(shown_as.is_some())
    .bind(shown_as.clone().flatten())
    .bind(&changes.email)
    .bind(changes.admin.map(i64::from))
    .bind(changes.scrobbling.map(i64::from))
    .bind(&now)
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

    let name_now = new_name.unwrap_or(&username);

    // A new password that left the old logins working would not be a new
    // password: the reason for changing one is almost always that somebody else
    // might have the old one, and a browser they left open never sends it again.
    //
    // The session doing the asking is spared, since it has just proved who it is.
    // When an administrator changes somebody else's password that session belongs
    // to a different account, so it spares nothing and every login of theirs
    // goes, which is the same rule arriving at the right answer twice.
    if hash.is_some() {
        let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
            .bind(name_now)
            .fetch_one(&pool)
            .await
            .map_err(|e| ApiError::internal(e, "looking up an account"))?;

        session::destroy_all(&pool, user_id, panel.id)
            .await
            .map_err(|e| ApiError::internal(e, "ending sessions after a password change"))?;
    }

    Ok(Json(load(&pool, name_now).await?))
}

/// What an account holds
///
/// The counts of everything that would go with it: sessions, keys, favourites,
/// ratings, plays, playlists and bookmarks. Yours, or anybody's if you administer
/// the server.
///
/// Its own call rather than seven more columns on the account, because a listing of
/// ten accounts would count seventy things to show none of them. This is what a
/// confirmation asks for at the moment it has something to warn about — a dialogue
/// that only asks whether you are sure is asking somebody to agree to something
/// they were not told.
#[utoipa::path(
    get,
    path = "/users/{username}/holdings",
    tag = "users",
    params(("username" = String, Path, description = "Whose account")),
    responses(
        (status = 200, description = "What only this account has", body = Holdings),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's account", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn holdings(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Holdings>, ApiError> {
    self_or_admin(&panel, &username)?;

    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(&username)
        .fetch_optional(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "looking up an account"))?
        .ok_or(ApiError::NotFound)?;

    // One statement rather than seven round trips, and every figure a count over an
    // indexed column. The identifier is bound once per subselect because that is
    // what a `?` is: the alternative is a numbered placeholder, which would make the
    // statement shorter and the reading of it harder.
    let row: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM sessions WHERE user_id = ? AND expires_at > ?),
                (SELECT count(*) FROM api_keys WHERE user_id = ? AND revoked_at IS NULL),
                (SELECT count(*) FROM user_track_stats
                  WHERE user_id = ? AND starred_at IS NOT NULL)
              + (SELECT count(*) FROM user_album_stats
                  WHERE user_id = ? AND starred_at IS NOT NULL)
              + (SELECT count(*) FROM user_artist_stats
                  WHERE user_id = ? AND starred_at IS NOT NULL),
                (SELECT count(*) FROM user_track_stats
                  WHERE user_id = ? AND rating IS NOT NULL)
              + (SELECT count(*) FROM user_album_stats
                  WHERE user_id = ? AND rating IS NOT NULL)
              + (SELECT count(*) FROM user_artist_stats
                  WHERE user_id = ? AND rating IS NOT NULL),
                (SELECT coalesce(sum(play_count), 0) FROM user_track_stats WHERE user_id = ?),
                (SELECT count(*) FROM playlists WHERE owner_id = ?),
                (SELECT count(*) FROM bookmarks WHERE user_id = ?)",
    )
    .bind(user_id)
    .bind(db::now())
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "counting what an account holds"))?;

    let (sessions, keys, favourites, ratings, plays, playlists, bookmarks) = row;

    Ok(Json(Holdings {
        sessions,
        keys,
        favourites,
        ratings,
        plays,
        playlists,
        bookmarks,
    }))
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

    let mut tx = crate::db::writing(&pool)
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
    use crate::session::A_MONTH;
    use crate::user::User;

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

    /// What a confirmation has to be able to say before it destroys anything. The
    /// three starred tables are counted as one figure, because "your favourites" is
    /// one thing to whoever has them.
    #[tokio::test]
    async fn what_an_account_holds_is_counted_across_its_tables() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();

        // A track needs a library and a folder to hang off, and the stats need the
        // track: the counts are of rows that only exist against real music.
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'music', '/music', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'music', '/music', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                 file_modified_at, content_type, suffix, title, duration_ms,
                                 last_seen_scan, created_at, updated_at)
             VALUES (1, 'trk1', 1, 1, '/one.wav', 1, ?, 'audio/wav', 'wav', 'One', 180000,
                     1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&at)
        .bind(&at)
        .fetch_one(&pool)
        .await
        .unwrap();

        // Starred and rated and played, all on the one track.
        sqlx::query(
            "INSERT INTO user_track_stats (user_id, track_id, play_count, rating, starred_at)
             VALUES (?, 1, 7, 4, ?)",
        )
        .bind(user_id)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO playlists (public_id, owner_id, name, created_at, updated_at)
             VALUES ('pl1', ?, 'road trip', ?, ?)",
        )
        .bind(user_id)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO bookmarks (user_id, track_id, position_ms, created_at, updated_at)
             VALUES (?, 1, 45000, ?, ?)",
        )
        .bind(user_id)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let panel = Panel {
            id: 1,
            user: User {
                id: user_id,
                username: "ana".to_string(),
                is_admin: false,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        };

        let Json(held) = holdings(panel, State(pool.clone()), Path("ana".to_string()))
            .await
            .unwrap();

        assert_eq!(held.favourites, 1, "one starred track and nothing else");
        assert_eq!(held.ratings, 1);
        assert_eq!(held.plays, 7, "times played, not rows");
        assert_eq!(held.playlists, 1);
        assert_eq!(held.bookmarks, 1);
        assert_eq!(held.sessions, 0);
        assert_eq!(held.keys, 0);
    }

    /// An account with three logins open, and a way to ask how many are left.
    async fn logged_in_thrice(admin: bool) -> (SqlitePool, i64, Vec<i64>) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let timestamp = db::now();
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', ?, ?, ?, ?) RETURNING id",
        )
        .bind(auth::hash_password("before").unwrap())
        .bind(i64::from(admin))
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        for _ in 0..3 {
            session::create(&pool, user_id, A_MONTH).await.unwrap();
        }

        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(&pool)
            .await
            .unwrap();

        (pool, user_id, ids)
    }

    fn panel_of(id: i64, user_id: i64, username: &str, admin: bool) -> Panel {
        Panel {
            id,
            user: User {
                id: user_id,
                username: username.to_string(),
                is_admin: admin,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        }
    }

    fn nothing() -> AccountChanges {
        AccountChanges {
            username: None,
            display_name: None,
            password: None,
            email: None,
            admin: None,
            scrobbling: None,
            current_password: None,
        }
    }

    /// What the account below was created with, and what changing anything
    /// sensitive about it therefore has to prove.
    const PASSWORD: &str = "before";

    /// The same, with the current password given: what a change to your own name,
    /// address or password needs alongside it.
    fn proving() -> AccountChanges {
        AccountChanges {
            current_password: Some(PASSWORD.to_string()),
            ..nothing()
        }
    }

    #[tokio::test]
    async fn a_new_password_closes_the_other_sessions() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;
        let mine = ids[0];

        let Json(_) = change(
            panel_of(mine, user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                password: Some("after".to_string()),
                ..proving()
            }),
        )
        .await
        .expect("the change goes through");

        let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(&pool)
            .await
            .unwrap();

        assert_eq!(left, vec![mine], "only the one that asked survives");
    }

    /// The account is renamed and given a new password in the same call, so the
    /// sessions have to be found by something the rename did not move.
    ///
    /// An administrator, because renaming is theirs alone — and their own account,
    /// which is the case that has both halves: a rename and a password change that
    /// closes the other sessions.
    #[tokio::test]
    async fn a_rename_alongside_it_does_not_lose_the_sessions() {
        let (pool, user_id, ids) = logged_in_thrice(true).await;

        let Json(account) = change(
            panel_of(ids[0], user_id, "ana", true),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                username: Some("anna".to_string()),
                password: Some("after".to_string()),
                ..proving()
            }),
        )
        .await
        .expect("the change goes through");

        // Without this the test would pass just as well on a rename that never
        // happened, and prove nothing about finding the sessions afterwards.
        assert_eq!(account.username, "anna");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    /// An administrator resetting somebody's password spares nothing of theirs,
    /// because the session being spared is not one of the account's at all.
    #[tokio::test]
    async fn a_reset_by_somebody_else_closes_all_of_them() {
        let (pool, user_id, _) = logged_in_thrice(false).await;

        let timestamp = db::now();
        let admin_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('boss', 'x', 1, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();
        session::create(&pool, admin_id, A_MONTH).await.unwrap();
        let admin_session: i64 =
            sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = ? LIMIT 1")
                .bind(admin_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        let Json(_) = change(
            panel_of(admin_session, admin_id, "boss", true),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                password: Some("after".to_string()),
                ..proving()
            }),
        )
        .await
        .expect("the change goes through");

        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "every login of theirs is gone"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id = ?")
                .bind(admin_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            1,
            "the administrator stays logged in"
        );
    }

    /// Changing anything else has no business logging anybody out.
    #[tokio::test]
    async fn changing_an_email_leaves_them_alone() {
        let (pool, user_id, _) = logged_in_thrice(false).await;

        let Json(_) = change(
            panel_of(1, user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                email: Some("ana@example.org".to_string()),
                ..proving()
            }),
        )
        .await
        .expect("the change goes through");

        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            3
        );
    }

    /// The date the password carries moves when the password does, and not when
    /// anything else does.
    ///
    /// Both halves matter. `updated_at` already says "something changed", so a date
    /// that moved with it would be a second copy of that and would answer the
    /// question it is there for — how long since I changed my password — with the day
    /// somebody corrected their email address.
    #[tokio::test]
    async fn the_password_date_moves_only_with_the_password() {
        let (pool, user_id, _) = logged_in_thrice(true).await;

        let set_at = |pool: SqlitePool| async move {
            sqlx::query_scalar::<_, String>("SELECT password_set_at FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap()
        };

        let when_made = set_at(pool.clone()).await;
        assert!(!when_made.is_empty(), "set when the account was made");

        // Something that is not the password, and a name change at that: the most
        // identity-like thing there is short of the password itself. By an
        // administrator, since a rename is not a listener's to make.
        let Json(_) = change(
            panel_of(1, user_id, "ana", true),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                username: Some("anna".to_string()),
                ..proving()
            }),
        )
        .await
        .expect("the rename goes through");

        assert_eq!(
            set_at(pool.clone()).await,
            when_made,
            "a rename is not a new password"
        );

        let Json(_) = change(
            panel_of(1, user_id, "anna", true),
            State(pool.clone()),
            Path("anna".to_string()),
            Json(AccountChanges {
                password: Some("after".to_string()),
                ..proving()
            }),
        )
        .await
        .expect("the new password goes through");

        // Compared as text, which is what the schema stores and what makes one
        // timestamp later than another.
        assert!(set_at(pool).await >= when_made, "and a new password is one");
    }

    /// The three that would lock somebody out of their own account, each refused on
    /// its own: a session is not proof that the owner is the one sitting there.
    #[tokio::test]
    async fn changing_your_own_credentials_needs_the_current_password() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        for asked in [
            AccountChanges {
                password: Some("after".to_string()),
                ..nothing()
            },
            AccountChanges {
                email: Some("ana@example.org".to_string()),
                ..nothing()
            },
        ] {
            let refused = change(
                panel_of(ids[0], user_id, "ana", false),
                State(pool.clone()),
                Path("ana".to_string()),
                Json(asked.clone()),
            )
            .await;

            assert!(
                matches!(refused, Err(ApiError::Invalid(_))),
                "{asked:?} should have been refused for want of the current password"
            );
        }

        let untouched = load(&pool, "ana").await.expect("still called ana");
        assert_eq!(untouched.email, None);
    }

    /// What somebody would rather be called is theirs, whoever they are, and it is
    /// the answer to renaming being administration: the name an administrator files
    /// you under and the name you are greeted by are two different things.
    #[tokio::test]
    async fn a_listener_may_choose_what_to_be_called() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let Json(account) = change(
            panel_of(ids[0], user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                display_name: Some("Ana María".to_string()),
                ..nothing()
            }),
        )
        .await
        .expect("theirs to set");

        assert_eq!(account.display_name.as_deref(), Some("Ana María"));
        assert_eq!(account.username, "ana", "and the account is still ana");
    }

    /// Nothing about it locks anybody out, so nothing about it is asked to prove
    /// itself. Which is the whole difference from the three fields above it.
    #[tokio::test]
    async fn choosing_what_to_be_called_proves_nothing() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let asked = change(
            panel_of(ids[0], user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                display_name: Some("Ana".to_string()),
                // No current password at all.
                ..nothing()
            }),
        )
        .await;

        assert!(asked.is_ok(), "it needs no password: {asked:?}");
    }

    /// And it can be given back. An empty one is a request — call me by my account's
    /// name again — which is why it is not the same as leaving the field out, and why
    /// a coalesce could not express it.
    #[tokio::test]
    async fn emptying_it_goes_back_to_the_account_name() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let called = |name: &str| AccountChanges {
            display_name: Some(name.to_string()),
            ..nothing()
        };

        let mine = || panel_of(ids[0], user_id, "ana", false);

        let Json(_) = change(
            mine(),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(called("Ana María")),
        )
        .await
        .unwrap();

        let Json(cleared) = change(
            mine(),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(called("   ")),
        )
        .await
        .expect("emptying it is allowed");

        assert_eq!(cleared.display_name, None, "back to being called ana");

        // And leaving the field out leaves it alone, which is the other half of the
        // same distinction.
        let Json(_) = change(
            mine(),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(called("Ana María")),
        )
        .await
        .unwrap();

        let Json(untouched) = change(
            mine(),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                scrobbling: Some(false),
                ..nothing()
            }),
        )
        .await
        .unwrap();

        assert_eq!(untouched.display_name.as_deref(), Some("Ana María"));
    }

    /// Renaming is administration, and a listener has no way to it.
    ///
    /// Their name is how an administrator knows who they are, and it is what every
    /// OpenSubsonic client logs in with: a listener who renamed themselves — which
    /// they could, with their own password — stopped every player they own from
    /// logging in, silently, and could not necessarily put it back, since the name
    /// they had may have been taken meanwhile.
    #[tokio::test]
    async fn a_listener_cannot_rename_themselves() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let refused = change(
            panel_of(ids[0], user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                username: Some("anna".to_string()),
                // With the current password, which is what made this reachable: it is
                // not the confirmation that was missing, it is the right to do it.
                current_password: Some("before".to_string()),
                ..nothing()
            }),
        )
        .await;

        assert_eq!(refused.err(), Some(ApiError::NotAuthorized));
        assert!(load(&pool, "ana").await.is_ok(), "still called ana");
        assert!(load(&pool, "anna").await.is_err(), "and not called anna");
    }

    /// And what they may still do, which is the reason the refusal is about the change
    /// and not about the field: the panel sends the whole form on every save, so a
    /// listener changing their address sends their own name along with it. Refusing
    /// that would be refusing a change of address over a rename nobody asked for.
    #[tokio::test]
    async fn a_listener_sending_the_name_they_already_have_is_not_renaming() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let Json(account) = change(
            panel_of(ids[0], user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                username: Some("ana".to_string()),
                email: Some("ana@example.org".to_string()),
                current_password: Some("before".to_string()),
                ..nothing()
            }),
        )
        .await
        .expect("their own name is not a rename");

        assert_eq!(account.username, "ana");
        assert_eq!(account.email.as_deref(), Some("ana@example.org"));
    }

    /// An administrator renames whoever they like, their own account included. That
    /// half was never in doubt and is what the refusal above is measured against.
    #[tokio::test]
    async fn an_administrator_renames_anybody_including_themselves() {
        let (pool, user_id, ids) = logged_in_thrice(true).await;

        let Json(renamed) = change(
            panel_of(ids[0], user_id, "ana", true),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                username: Some("anna".to_string()),
                current_password: Some("before".to_string()),
                ..nothing()
            }),
        )
        .await
        .expect("an administrator may rename themselves");

        assert_eq!(renamed.username, "anna");

        // And somebody else, who is not asked for a password because the
        // administrator does not have theirs.
        let at = db::now();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('bea', 'x', 0, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let Json(other) = change(
            panel_of(ids[0], user_id, "anna", true),
            State(pool.clone()),
            Path("bea".to_string()),
            Json(AccountChanges {
                username: Some("beatriz".to_string()),
                ..nothing()
            }),
        )
        .await
        .expect("and anybody else");

        assert_eq!(other.username, "beatriz");
    }

    /// A typo is a typo. It must not read as the session having gone, which is what
    /// a 401 would say and what would cost somebody their session for mistyping.
    #[tokio::test]
    async fn the_wrong_current_password_is_refused_without_ending_the_session() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let refused = change(
            panel_of(ids[0], user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                password: Some("after".to_string()),
                current_password: Some("not it".to_string()),
                ..nothing()
            }),
        )
        .await;

        assert_eq!(refused.err(), Some(ApiError::WrongPassword));
        assert_eq!(ApiError::WrongPassword.status(), 403);

        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            3,
            "and the sessions are all still there"
        );

        assert!(
            user::authenticate_password(&pool, "ana", PASSWORD)
                .await
                .unwrap()
                .is_some(),
            "the password is the one it was"
        );
    }

    /// An API key stands in for a password when a music player logs in. It must not
    /// stand in for one here: the whole point of asking is that somebody is present
    /// to answer, and a key is held by whatever was set up with it.
    #[tokio::test]
    async fn an_api_key_does_not_count_as_the_current_password() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let key = auth::generate_token().unwrap();
        sqlx::query(
            "INSERT INTO api_keys (user_id, key_hash, label, created_at)
             VALUES (?, ?, 'phone', ?)",
        )
        .bind(user_id)
        .bind(auth::hash_secret(&key))
        .bind(db::now())
        .execute(&pool)
        .await
        .unwrap();

        let refused = change(
            panel_of(ids[0], user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                password: Some("after".to_string()),
                current_password: Some(key),
                ..nothing()
            }),
        )
        .await;

        assert_eq!(refused.err(), Some(ApiError::WrongPassword));
    }

    /// What is not sensitive is not asked about. A preference set by mistake is set
    /// back by hand, and asking for a password to tick a box teaches people to type
    /// it without reading why.
    #[tokio::test]
    async fn a_preference_of_your_own_needs_nothing() {
        let (pool, user_id, ids) = logged_in_thrice(false).await;

        let Json(account) = change(
            panel_of(ids[0], user_id, "ana", false),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                scrobbling: Some(false),
                ..nothing()
            }),
        )
        .await
        .expect("no password needed for this");

        assert!(!account.scrobbling);
    }

    /// An administrator resetting somebody's password does not have it to give, and
    /// the account they could lock out is not theirs. Their own is protected by the
    /// same rule when they change it.
    #[tokio::test]
    async fn an_administrator_resetting_somebody_else_is_not_asked() {
        let (pool, user_id, _) = logged_in_thrice(false).await;

        let timestamp = db::now();
        let admin_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('boss', ?, 1, ?, ?) RETURNING id",
        )
        .bind(auth::hash_password("boss's own").unwrap())
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        let Json(_) = change(
            panel_of(99, admin_id, "boss", true),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(AccountChanges {
                password: Some("reset".to_string()),
                ..nothing()
            }),
        )
        .await
        .expect("an administrator may reset it");

        assert!(
            user::authenticate_password(&pool, "ana", "reset")
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "and every session of theirs is gone, since none of them was the asker"
        );
    }
}
