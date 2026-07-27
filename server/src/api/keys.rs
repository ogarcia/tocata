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
//! A key does not expire unless it is asked to. It is held by a music player,
//! and OpenSubsonic gives a player no way to renew anything, so a date is the
//! music stopping with nothing announcing it — worth choosing for a key lent to
//! somebody or made to try a client out, and no way to treat one by default.
//! Ordinarily a key ends when it is revoked, which is the trade the whole
//! mechanism is for: a password changes and every client is locked out at once,
//! a key is withdrawn and only that one is.
//!
//! An expired key is kept until it is revoked. It authenticates nothing, but the
//! date can be pushed out and the same key works again, so a week's trial does
//! not have to become setting the client up a second time.

use super::error::ApiError;
use super::session::Panel;
use crate::types::{ErrorBody, IssuedKey, Key, KeyChanges, NewKey, Revoked};
use crate::{auth, db};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;

/// Every column the two readers below select, in the order they select it.
type KeyRow = (i64, String, String, Option<String>, Option<String>);

impl Key {
    /// Whether the expiry has passed is settled against a timestamp passed in,
    /// so a whole listing is judged against one moment rather than against a
    /// clock that moves between rows.
    fn from_row((id, label, created_at, expires_at, last_used_at): KeyRow, today: &str) -> Self {
        Self {
            id,
            label,
            created_at,
            expired: expires_at.as_deref().is_some_and(|at| at <= today),
            expires_at,
            last_used_at,
        }
    }
}

/// Stands in when no label was given.
const UNLABELLED: &str = "unnamed";

/// Refused rather than guessed at. A date nobody can parse would otherwise
/// become no expiry at all, which is the opposite of what was asked for.
const BAD_DATE: ApiError = ApiError::Invalid("The expiry is not an ISO-8601 moment");

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

    // Expired keys are listed too. They are kept on purpose, and a key that had
    // vanished from the panel could not be given a new date.
    let rows: Vec<KeyRow> = sqlx::query_as(
        "SELECT id, label, created_at, expires_at, last_used_at
           FROM api_keys WHERE user_id = ? ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing API keys"))?;

    let today = db::now();

    Ok(Json(
        rows.into_iter()
            .map(|row| Key::from_row(row, &today))
            .collect(),
    ))
}

/// Issue an API key
///
/// Makes a key and returns it. This is the only time it is readable: what the
/// database keeps is a hash, so a key that gets lost is replaced, not recovered.
///
/// Without an expiry the key does not expire, and stops working only when it is
/// revoked or the account goes. With one, remember that nothing will warn the
/// client: it will simply stop being let in on that date.
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

    let expires_at = match new.expires_at.as_deref() {
        Some(given) => Some(db::timestamp_from(given).ok_or(BAD_DATE)?),
        None => None,
    };

    let key = auth::generate_token().map_err(|e| ApiError::internal(e, "generating an API key"))?;
    let created_at = db::now();

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO api_keys (user_id, key_hash, label, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(auth::hash_secret(&key))
    .bind(&label)
    .bind(&created_at)
    .bind(&expires_at)
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "issuing an API key"))?;

    Ok((
        StatusCode::CREATED,
        Json(IssuedKey {
            id,
            label,
            created_at,
            expires_at,
            key,
        }),
    ))
}

/// Change an API key
///
/// Renames it, or moves its expiry — including pushing out one that has already
/// passed, which is why an expired key is kept rather than swept away. The key
/// itself never changes, so whatever holds it keeps working without being set up
/// again.
#[utoipa::path(
    patch,
    path = "/users/{username}/keys/{id}",
    tag = "keys",
    params(
        ("username" = String, Path, description = "Whose keys"),
        ("id" = i64, Path, description = "Which key"),
    ),
    request_body = KeyChanges,
    responses(
        (status = 200, description = "The key as it now is", body = Key),
        (status = 400, description = "Nothing to change, or a date that is not one", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account, or no such key of theirs", body = ErrorBody),
    )
)]
pub async fn change(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
    Json(changes): Json<KeyChanges>,
) -> Result<Json<Key>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let label = changes
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty());

    // Only when the field was written at all. `Some(None)` is a request to drop
    // the expiry, and it has to reach the statement as a real null.
    let expires_at = match &changes.expires_at {
        Some(Some(given)) => Some(Some(db::timestamp_from(given).ok_or(BAD_DATE)?)),
        Some(None) => Some(None),
        None => None,
    };

    if label.is_none() && expires_at.is_none() {
        return Err(ApiError::Invalid("Nothing to change was given"));
    }

    // Coalesce keeps an absent label, but the expiry cannot use it: null there
    // means "no expiry", which is a value and not an absence. So it is applied
    // by its own statement, and only when it was asked for.
    let changed =
        sqlx::query("UPDATE api_keys SET label = coalesce(?, label) WHERE id = ? AND user_id = ?")
            .bind(label)
            .bind(id)
            .bind(user_id)
            .execute(&pool)
            .await
            .map_err(|e| ApiError::internal(e, "changing an API key"))?;

    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    if let Some(expires_at) = expires_at {
        sqlx::query("UPDATE api_keys SET expires_at = ? WHERE id = ? AND user_id = ?")
            .bind(expires_at)
            .bind(id)
            .bind(user_id)
            .execute(&pool)
            .await
            .map_err(|e| ApiError::internal(e, "changing when an API key expires"))?;
    }

    load(&pool, id).await
}

/// One key as it stands, for handing back after a change.
async fn load(pool: &SqlitePool, id: i64) -> Result<Json<Key>, ApiError> {
    let row: KeyRow = sqlx::query_as(
        "SELECT id, label, created_at, expires_at, last_used_at FROM api_keys WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|e| ApiError::internal(e, "loading an API key"))?;

    Ok(Json(Key::from_row(row, &db::now())))
}

/// Revoke every API key
///
/// For cutting an account off rather than tidying up: somebody has lost the phone
/// their key is on, or has left, and every client holding one has to stop working
/// at once. Changing the password does not do this — a key is not the password,
/// which is the whole point of having keys.
///
/// Yours, or anybody's if you administer the server.
#[utoipa::path(
    delete,
    path = "/users/{username}/keys",
    tag = "keys",
    params(("username" = String, Path, description = "Whose keys")),
    responses(
        (status = 200, description = "How many were revoked", body = Revoked),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn revoke_all(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Revoked>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let deleted = sqlx::query("DELETE FROM api_keys WHERE user_id = ?")
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "revoking every API key"))?;

    Ok(Json(Revoked {
        revoked: deleted.rows_affected(),
    }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    /// Absent and null have to survive the parse as different things, since one
    /// means "leave the expiry alone" and the other "have none".
    #[test]
    fn an_expiry_left_out_is_not_an_expiry_set_to_nothing() {
        let absent: KeyChanges = serde_json::from_str(r#"{"label":"phone"}"#).unwrap();
        assert!(absent.expires_at.is_none(), "not mentioned");

        let cleared: KeyChanges = serde_json::from_str(r#"{"expiresAt":null}"#).unwrap();
        assert!(matches!(cleared.expires_at, Some(None)), "never again");

        let given: KeyChanges =
            serde_json::from_str(r#"{"expiresAt":"2027-01-01T00:00:00Z"}"#).unwrap();
        assert!(matches!(given.expires_at, Some(Some(_))), "on that day");
    }

    async fn an_account() -> (SqlitePool, Panel) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let timestamp = db::now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        let panel = Panel {
            id: 1,
            user: User {
                id,
                username: "ana".to_string(),
                is_admin: false,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        };

        (pool, panel)
    }

    async fn issue_with(pool: &SqlitePool, panel: &Panel, expires_at: Option<&str>) -> i64 {
        let (_, Json(issued)) = issue(
            panel_like(panel),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(NewKey {
                label: Some("trying a client".to_string()),
                expires_at: expires_at.map(str::to_string),
            }),
        )
        .await
        .expect("the key is issued");

        issued.id
    }

    /// The whole arc the feature is for: a key for a week, dead afterwards, then
    /// given another week without the client having to be set up again.
    #[tokio::test]
    async fn an_expired_key_can_be_given_more_time() {
        let (pool, panel) = an_account().await;
        // Midnight today rather than a distant date, for the reason spelled out
        // where the same trick is used against the authentication query.
        let ran_out = format!("{}T00:00:00Z", &db::now()[..10]);
        let id = issue_with(&pool, &panel, Some(&ran_out)).await;

        let Json(keys) = list(panel_like(&panel), State(pool.clone()), Path("ana".into()))
            .await
            .unwrap();
        assert!(keys[0].expired, "past its date");
        assert_eq!(keys[0].expires_at.as_deref(), Some(ran_out.as_str()));

        let Json(key) = change(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
            Json(serde_json::from_str(r#"{"expiresAt":"2999-01-01T00:00:00Z"}"#).unwrap()),
        )
        .await
        .unwrap();

        assert!(!key.expired, "alive again");
    }

    #[tokio::test]
    async fn an_expiry_can_be_taken_off_entirely() {
        let (pool, panel) = an_account().await;
        let id = issue_with(&pool, &panel, Some("2027-01-01T00:00:00Z")).await;

        let Json(key) = change(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
            Json(serde_json::from_str(r#"{"expiresAt":null}"#).unwrap()),
        )
        .await
        .unwrap();

        assert_eq!(key.expires_at, None);
        assert!(!key.expired);
    }

    /// Renaming must not quietly drop the date, which is what a coalesce over
    /// both fields at once would have done.
    #[tokio::test]
    async fn renaming_leaves_the_expiry_where_it_was() {
        let (pool, panel) = an_account().await;
        let id = issue_with(&pool, &panel, Some("2027-01-01T00:00:00Z")).await;

        let Json(key) = change(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
            Json(serde_json::from_str(r#"{"label":"laptop"}"#).unwrap()),
        )
        .await
        .unwrap();

        assert_eq!(key.label, "laptop");
        assert_eq!(key.expires_at.as_deref(), Some("2027-01-01T00:00:00Z"));
    }

    /// An offset that is not normalised would be compared as though it were UTC,
    /// and the key would die at the wrong hour.
    #[tokio::test]
    async fn an_offset_is_stored_as_utc() {
        let (pool, panel) = an_account().await;
        let id = issue_with(&pool, &panel, Some("2027-01-01T11:00:00+02:00")).await;

        let Json(key) = load(&pool, id).await.unwrap();

        assert_eq!(key.expires_at.as_deref(), Some("2027-01-01T09:00:00Z"));
    }

    #[tokio::test]
    async fn a_date_that_is_not_one_is_refused() {
        let (pool, panel) = an_account().await;

        let refused = issue(
            panel_like(&panel),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(NewKey {
                label: None,
                expires_at: Some("next tuesday".to_string()),
            }),
        )
        .await;

        assert!(matches!(refused, Err(ApiError::Invalid(_))));
    }

    fn panel_like(panel: &Panel) -> Panel {
        Panel {
            id: panel.id,
            user: panel.user.clone(),
            expires_at: panel.expires_at.clone(),
        }
    }
}
