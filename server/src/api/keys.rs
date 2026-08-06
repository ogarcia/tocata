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
//! Withdrawing and removing are two steps, and the order is the same one a
//! library goes through before it can be deleted: a key works, then it is revoked
//! or its date passes, and only then can the row go. Revoking is what stops the
//! client; removing is tidying up afterwards, and it is worth being a second
//! press because the row is where the key's name is — which is the only thing
//! anybody can check a revocation against, and gone for good once the row is.
//!
//! Revoking is final for the key it revokes. There is no unrevoking: whatever
//! held it has to be given a new one, which is the same as it was when the only
//! way to withdraw a key was to delete it.
//!
//! An expired key is kept until it is removed. It authenticates nothing, but the
//! date can be pushed out and the same key works again, so a week's trial does
//! not have to become setting the client up a second time.

use super::error::ApiError;
use super::session::Panel;
use crate::db::InTurn;
use crate::types::{ErrorBody, IssuedKey, Key, KeyChanges, NewKey, Revoked};
use crate::{auth, db};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;

/// Every column the two readers below select, in the order they select it.
type KeyRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// The columns themselves, written once because two statements select exactly
/// these and a listing that disagreed with a reload would be two shapes of the
/// same key. A macro rather than a constant so `concat!` finishes each statement
/// at compile time: sqlx does not take SQL assembled at runtime.
macro_rules! key_columns {
    () => {
        "SELECT id, label, created_at, expires_at, last_used_at, revoked_at FROM api_keys"
    };
}

impl Key {
    /// Whether the expiry has passed is settled against a timestamp passed in,
    /// so a whole listing is judged against one moment rather than against a
    /// clock that moves between rows.
    fn from_row(
        (id, label, created_at, expires_at, last_used_at, revoked_at): KeyRow,
        today: &str,
    ) -> Self {
        Self {
            id,
            label,
            created_at,
            expired: expires_at.as_deref().is_some_and(|at| at <= today),
            expires_at,
            last_used_at,
            revoked_at,
        }
    }
}

/// Stands in when no label was given.
const UNLABELLED: &str = "unnamed";

/// Refused rather than guessed at. A date nobody can parse would otherwise
/// become no expiry at all, which is the opposite of what was asked for.
const BAD_DATE: ApiError = ApiError::Invalid("The expiry is not an ISO-8601 moment");

/// A revoked key is not there to be given a new secret. Rotating one would hand
/// back a key that reads as usable and opens nothing.
const ALREADY_REVOKED: ApiError = ApiError::Conflict("The key is revoked");

/// The order the two steps go in, said to whoever skipped the first one.
const STILL_LIVE: ApiError = ApiError::Conflict("Revoke the key before removing it");

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

/// Where one of the account's keys stands: when it was withdrawn, and when it
/// runs out. `NotFound` when the account has no such key, which is also the
/// answer for somebody else's key named by its number.
///
/// The two timestamps rather than a verdict, because the two callers below ask
/// different questions of them.
async fn standing(
    pool: &SqlitePool,
    user_id: i64,
    id: i64,
) -> Result<(Option<String>, Option<String>), ApiError> {
    sqlx::query_as("SELECT revoked_at, expires_at FROM api_keys WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ApiError::internal(e, "looking up an API key"))?
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

    // Expired and revoked keys are listed too. Both are kept on purpose: an
    // expired one can be given a new date, and a revoked one is what tells
    // whoever revoked it that they revoked the right one.
    let rows: Vec<KeyRow> = sqlx::query_as(concat!(
        key_columns!(),
        " WHERE user_id = ? ORDER BY created_at"
    ))
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
    .in_turn(&pool)
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

/// Rotate an API key
///
/// Gives the key a new secret and keeps everything else: its name, its expiry, its
/// row. What it is for has not changed — the same client, the same shelf in the
/// panel — only the secret has, which is the whole point of rotating one.
///
/// Done here rather than by issuing one and revoking the other, because that is
/// two calls with a gap in the middle: fail on the second and the account is left
/// with two live keys, one of which nobody knows the purpose of.
///
/// The new secret is readable once, exactly like a new key. `lastUsedAt` goes back
/// to null, because nothing has used this secret yet and saying otherwise would
/// describe the one it replaced.
///
/// A revoked key is not rotated. Withdrawing one is meant to be the end of it, and
/// a new secret on that row would be a way back that nothing else offers.
#[utoipa::path(
    post,
    path = "/users/{username}/keys/{id}/rotate",
    tag = "keys",
    params(
        ("username" = String, Path, description = "Whose key"),
        ("id" = i64, Path, description = "Which one"),
    ),
    responses(
        (status = 200, description = "The key, for the only time it can be read", body = IssuedKey),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account, or no such key of theirs", body = ErrorBody),
        (status = 409, description = "The key is revoked", body = ErrorBody),
    )
)]
pub async fn rotate(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
) -> Result<Json<IssuedKey>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    // Read before written, so a key of somebody else's is a miss and a revoked
    // one of your own is a refusal — two different answers to what would
    // otherwise both have been zero rows changed.
    let (revoked_at, _) = standing(&pool, user_id, id).await?;

    if revoked_at.is_some() {
        return Err(ALREADY_REVOKED);
    }

    let key = auth::generate_token().map_err(|e| ApiError::internal(e, "generating an API key"))?;

    sqlx::query(
        "UPDATE api_keys SET key_hash = ?, last_used_at = NULL
          WHERE id = ? AND user_id = ?",
    )
    .bind(auth::hash_secret(&key))
    .bind(id)
    .bind(user_id)
    .in_turn(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "rotating an API key"))?;

    let Json(now) = load(&pool, id).await?;

    Ok(Json(IssuedKey {
        id: now.id,
        label: now.label,
        created_at: now.created_at,
        expires_at: now.expires_at,
        key,
    }))
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
            .in_turn(&pool)
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
            .in_turn(&pool)
            .await
            .map_err(|e| ApiError::internal(e, "changing when an API key expires"))?;
    }

    load(&pool, id).await
}

/// One key as it stands, for handing back after a change.
async fn load(pool: &SqlitePool, id: i64) -> Result<Json<Key>, ApiError> {
    let row: KeyRow = sqlx::query_as(concat!(key_columns!(), " WHERE id = ?"))
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
/// The rows stay, revoked, so that afterwards there is still a list saying what
/// was withdrawn. One already revoked is left as it was, with the moment it was
/// first withdrawn.
///
/// Yours, or anybody's if you administer the server.
#[utoipa::path(
    post,
    path = "/users/{username}/keys/revoke",
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

    // The ones that still work, which is what makes the count the number of
    // clients this just stopped rather than the number of rows there happened
    // to be.
    let withdrawn =
        sqlx::query("UPDATE api_keys SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
            .bind(db::now())
            .bind(user_id)
            .in_turn(&pool)
            .await
            .map_err(|e| ApiError::internal(e, "revoking every API key"))?;

    Ok(Json(Revoked {
        revoked: withdrawn.rows_affected(),
    }))
}

/// Revoke an API key
///
/// Whatever was using it stops working at once. The key stays in the listing,
/// revoked and named, until it is removed.
///
/// Asking twice changes nothing and keeps the first moment: what matters about a
/// revocation is when the key stopped working, and that already happened.
#[utoipa::path(
    post,
    path = "/users/{username}/keys/{id}/revoke",
    tag = "keys",
    params(
        ("username" = String, Path, description = "Whose keys"),
        ("id" = i64, Path, description = "Which key"),
    ),
    responses(
        (status = 200, description = "The key as it now is", body = Key),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account, or no such key of theirs", body = ErrorBody),
    )
)]
pub async fn revoke(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
) -> Result<Json<Key>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    // Both conditions, so naming somebody else's key by its number revokes
    // nothing rather than revoking theirs.
    let withdrawn = sqlx::query(
        "UPDATE api_keys SET revoked_at = coalesce(revoked_at, ?) WHERE id = ? AND user_id = ?",
    )
    .bind(db::now())
    .bind(id)
    .bind(user_id)
    .in_turn(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "revoking an API key"))?;

    if withdrawn.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    load(&pool, id).await
}

/// Remove an API key
///
/// Takes the row away, name and all. What it was for is no longer written down
/// anywhere, which is the difference from revoking it.
///
/// Only a key that has already stopped working: revoked, or past its date. A key
/// that is still letting a client in is refused, because removing it would be
/// withdrawing it under another name — and the word for withdrawing something is
/// not one everybody reads as the end of it.
#[utoipa::path(
    delete,
    path = "/users/{username}/keys/{id}",
    tag = "keys",
    params(
        ("username" = String, Path, description = "Whose keys"),
        ("id" = i64, Path, description = "Which key"),
    ),
    responses(
        (status = 204, description = "Gone"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's keys", body = ErrorBody),
        (status = 404, description = "No such account, or no such key of theirs", body = ErrorBody),
        (status = 409, description = "The key still works; revoke it first", body = ErrorBody),
    )
)]
pub async fn remove(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
) -> Result<StatusCode, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let (revoked_at, expires_at) = standing(&pool, user_id, id).await?;
    let today = db::now();
    let expired = expires_at.as_deref().is_some_and(|at| at <= today.as_str());

    if revoked_at.is_none() && !expired {
        return Err(STILL_LIVE);
    }

    sqlx::query("DELETE FROM api_keys WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .in_turn(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "removing an API key"))?;

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

    /// What rotating is for: the client gets a new secret and the shelf it sits
    /// on in the panel does not move. The old secret is what has to stop working,
    /// and nothing else.
    #[tokio::test]
    async fn rotating_changes_the_secret_and_nothing_else() {
        let (pool, panel) = an_account().await;

        let (_, Json(first)) = issue(
            panel_like(&panel),
            State(pool.clone()),
            Path("ana".to_string()),
            Json(NewKey {
                label: Some("phone".to_string()),
                expires_at: Some("2027-01-01T00:00:00Z".to_string()),
            }),
        )
        .await
        .unwrap();

        // So that forgetting it can be told apart from never having had one.
        sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
            .bind(db::now())
            .bind(first.id)
            .execute(&pool)
            .await
            .unwrap();

        let Json(second) = rotate(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), first.id)),
        )
        .await
        .unwrap();

        assert_eq!(second.id, first.id, "the same key");
        assert_eq!(second.label, "phone");
        assert_eq!(second.expires_at.as_deref(), Some("2027-01-01T00:00:00Z"));
        assert_ne!(second.key, first.key, "a new secret");

        // The row holds the new secret and only the new one, which is the whole
        // of what rotating does: the old one now hashes to nothing that is stored.
        let (stored, used): (String, Option<String>) =
            sqlx::query_as("SELECT key_hash, last_used_at FROM api_keys WHERE id = ?")
                .bind(first.id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(
            stored,
            auth::hash_secret(&second.key),
            "the new one opens it"
        );
        assert_ne!(stored, auth::hash_secret(&first.key), "the old one is gone");
        assert_eq!(used, None, "nothing has used this secret yet");
    }

    /// The two steps, in the order they go in. Revoking is what stops the client;
    /// the row goes on a second press, and not before.
    #[tokio::test]
    async fn a_working_key_is_revoked_before_it_can_be_removed() {
        let (pool, panel) = an_account().await;
        let id = issue_with(&pool, &panel, None).await;

        let refused = remove(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await;

        assert!(matches!(refused, Err(ApiError::Conflict(_))));
        assert_eq!(rows(&pool).await, 1, "still there to be revoked");

        let Json(key) = revoke(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await
        .expect("revoking it");

        assert!(key.revoked_at.is_some(), "withdrawn");
        assert_eq!(
            key.label, "trying a client",
            "and still says what it was for"
        );
        assert_eq!(rows(&pool).await, 1, "revoking is not deleting");

        remove(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await
        .expect("and now it goes");

        assert_eq!(rows(&pool).await, 0);
    }

    /// The other way to the same end: a key whose date has passed has already
    /// stopped working, so there is nothing left for revoking it to stop.
    #[tokio::test]
    async fn an_expired_key_is_removed_without_being_revoked() {
        let (pool, panel) = an_account().await;
        let ran_out = format!("{}T00:00:00Z", &db::now()[..10]);
        let id = issue_with(&pool, &panel, Some(&ran_out)).await;

        remove(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await
        .expect("it stopped working on its own");

        assert_eq!(rows(&pool).await, 0);
    }

    /// What matters about a revocation is when the key stopped working, and asking
    /// again does not move that moment.
    #[tokio::test]
    async fn revoking_twice_keeps_the_first_moment() {
        let (pool, panel) = an_account().await;
        let id = issue_with(&pool, &panel, None).await;

        let Json(first) = revoke(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await
        .unwrap();

        sqlx::query("UPDATE api_keys SET revoked_at = '2020-01-01T00:00:00Z' WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        let Json(again) = revoke(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await
        .unwrap();

        assert_eq!(again.revoked_at.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_ne!(again.revoked_at, first.revoked_at, "and not written afresh");
    }

    /// Rotating a revoked key would hand back a secret that opens nothing, on a
    /// row that says it is finished.
    #[tokio::test]
    async fn a_revoked_key_is_not_rotated() {
        let (pool, panel) = an_account().await;
        let id = issue_with(&pool, &panel, None).await;

        let Json(_) = revoke(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await
        .unwrap();

        let refused = rotate(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), id)),
        )
        .await;

        assert!(matches!(refused, Err(ApiError::Conflict(_))));
    }

    /// Cutting an account off: every key that worked stops, the rows stay so that
    /// what was withdrawn can still be read, and the count is the number of
    /// clients this just stopped.
    #[tokio::test]
    async fn revoking_every_key_counts_only_the_ones_that_worked() {
        let (pool, panel) = an_account().await;
        let already = issue_with(&pool, &panel, None).await;
        issue_with(&pool, &panel, None).await;
        issue_with(&pool, &panel, None).await;

        let Json(_) = revoke(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), already)),
        )
        .await
        .unwrap();

        let Json(counted) = revoke_all(panel_like(&panel), State(pool.clone()), Path("ana".into()))
            .await
            .unwrap();

        assert_eq!(counted.revoked, 2, "the two that were still working");
        assert_eq!(rows(&pool).await, 3, "and all three are still listed");

        let Json(keys) = list(panel_like(&panel), State(pool.clone()), Path("ana".into()))
            .await
            .unwrap();
        assert!(keys.iter().all(|key| key.revoked_at.is_some()));
    }

    async fn rows(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM api_keys")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Naming somebody else's key by its number must rotate nothing, which is why
    /// the update carries the owner as well as the id.
    #[tokio::test]
    async fn a_key_of_somebody_elses_is_not_there_to_rotate() {
        let (pool, panel) = an_account().await;
        let timestamp = db::now();

        let other: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('bruno', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        let theirs: i64 = sqlx::query_scalar(
            "INSERT INTO api_keys (user_id, key_hash, label, created_at)
             VALUES (?, 'theirs', 'their phone', ?) RETURNING id",
        )
        .bind(other)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        let refused = rotate(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), theirs)),
        )
        .await;

        assert!(matches!(refused, Err(ApiError::NotFound)));

        let untouched: String = sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE id = ?")
            .bind(theirs)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(untouched, "theirs", "their key still opens what it opened");
    }

    fn panel_like(panel: &Panel) -> Panel {
        Panel {
            id: panel.id,
            user: panel.user.clone(),
            expires_at: panel.expires_at.clone(),
        }
    }
}
