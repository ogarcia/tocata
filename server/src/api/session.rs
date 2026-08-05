// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Logging in and out of the panel, and the extractor everything else sits
//! behind.

use super::error::ApiError;
use super::preferences;
use crate::attempts::Attempts;
use crate::types::{Credentials, ErrorBody, Identity};
use crate::user::User;
use crate::{session, user};
use axum::extract::{ConnectInfo, FromRef, FromRequestParts, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, RequestPartsExt};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::sync::Arc;

/// Name of the cookie the token travels in.
const COOKIE_NAME: &str = "tocata_session";

/// Scoped to `/api` because that is the only place it means anything, so a
/// request for a cover or a stream never carries it.
const COOKIE_PATH: &str = "/api";

/// A request that arrived with a valid session.
///
/// Asking for this in a handler is what makes the handler private: there is no
/// way to write one that forgets to check, because the check is the argument.
pub struct Panel {
    /// Which session this is, so a handler can tell it apart from the others the
    /// same account has open.
    pub id: i64,
    pub user: User,
    /// When this session runs out, so a handler can pass it on without asking
    /// the database a second time.
    pub expires_at: String,
}

impl<S> FromRequestParts<S> for Panel
where
    SqlitePool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token = token_from_cookies(&parts.headers).ok_or(ApiError::NotAuthenticated)?;
        let pool = SqlitePool::from_ref(state);

        match session::resolve(&pool, &token).await {
            Ok(Some(session)) => Ok(Self {
                id: session.id,
                user: session.user,
                expires_at: session.expires_at,
            }),
            Ok(None) => Err(ApiError::NotAuthenticated),
            Err(e) => Err(ApiError::internal(e, "resolving a session")),
        }
    }
}

/// A request from somebody who administers the server.
///
/// Layered on `Panel` rather than repeating its work, so there is one place that
/// knows how a session is found and one that knows what it is allowed to do.
/// Carries who it is, because the rules that keep a server administrable are
/// about identity: nobody may delete their own account or take away their own
/// administrator rights, and that pair is what guarantees there is always
/// somebody left who can.
pub struct Administrator {
    pub user: User,
}

impl<S> FromRequestParts<S> for Administrator
where
    SqlitePool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Panel { user, .. } = parts.extract_with_state::<Panel, S>(state).await?;

        if user.is_admin {
            Ok(Self { user })
        } else {
            Err(ApiError::NotAuthorized)
        }
    }
}

/// Pulls our cookie out of the header, ignoring everybody else's.
fn token_from_cookies(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, token)| token.to_string())
}

/// What this person would rather be called, if they have said.
///
/// Read here rather than carried on `User`, which is what every authentication
/// builds: a name for a greeting is of no interest to the twenty other things that
/// resolve a session, and this is asked twice in the life of a panel — on a login and
/// on a reload.
async fn shown_as(pool: &SqlitePool, user_id: i64) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar("SELECT display_name FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| ApiError::internal(e, "reading what to call somebody"))
}

impl Identity {
    /// The preferences travel with the identity because the panel needs them to
    /// draw itself, so both ways in fetch them: a reload lands on `current` and a
    /// login on `log_in`, and neither should mean a second call before the first
    /// paint.
    async fn of(pool: &SqlitePool, user: &User, expires_at: String) -> Result<Self, ApiError> {
        Ok(Self {
            username: user.username.clone(),
            display_name: shown_as(pool, user.id).await?,
            admin: user.is_admin,
            expires_at,
            preferences: preferences::load(pool, user.id).await?,
        })
    }
}

/// Log in
///
/// Exchanges a username and password for a session cookie. The cookie is
/// `HttpOnly`, so the panel's own scripts cannot read it back, and it is what
/// authenticates every other call under `/api` including the event stream.
#[utoipa::path(
    post,
    path = "/session",
    tag = "session",
    request_body = Credentials,
    responses(
        (status = 200, description = "Logged in; the session cookie is set", body = Identity),
        (status = 401, description = "Wrong username or password", body = ErrorBody),
        (status = 429, description = "Too many failed logins from here", body = ErrorBody),
    )
)]
pub async fn log_in(
    State(pool): State<SqlitePool>,
    State(attempts): State<Arc<Attempts>>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    Json(credentials): Json<Credentials>,
) -> Result<Response, ApiError> {
    // Before the password is even hashed. Argon2 is deliberately slow, so a
    // server that hashed first would be one that could be made to spend its
    // whole processor on guesses.
    if attempts.barred(from.ip()) {
        return Err(ApiError::TooManyAttempts);
    }

    let authenticated =
        user::authenticate_password(&pool, &credentials.username, &credentials.password)
            .await
            .map_err(|e| ApiError::internal(e, "authenticating a panel login"))?;

    let Some(user) = authenticated else {
        attempts.failed(from.ip());
        return Err(ApiError::WrongCredentials);
    };

    attempts.succeeded(from.ip());

    // Read now rather than held anywhere: an administrator who shortens this
    // means it for the next login, and the next login is this one.
    let days = crate::settings::load(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "reading the settings"))?
        .session_days;

    let (token, expires_at) = session::create(&pool, user.id, days)
        .await
        .map_err(|e| ApiError::internal(e, "creating a session"))?;

    let identity = Identity::of(&pool, &user, expires_at).await?;

    // A cookie with no `Max-Age` is one the browser drops when it closes, which
    // is what "do not keep me logged in" has to mean here. The row keeps its own
    // expiry either way: the session is still there, it is the browser that has
    // been told to forget the way in. Ending it outright is what Log out does.
    let cookie = match credentials.remember {
        false => format!("{COOKIE_NAME}={token}; Path={COOKIE_PATH}; HttpOnly; SameSite=Strict"),
        true => format!(
            "{COOKIE_NAME}={token}; Path={COOKIE_PATH}; HttpOnly; SameSite=Strict; Max-Age={}",
            session::lifetime_seconds(days)
        ),
    };

    Ok(([(SET_COOKIE, cookie)], Json(identity)).into_response())
}

/// Log out
///
/// Ends this session and clears the cookie. Sessions opened from other browsers
/// are left alone.
#[utoipa::path(
    delete,
    path = "/session",
    tag = "session",
    responses(
        (status = 204, description = "Logged out, whether or not there was a session"),
    )
)]
pub async fn log_out(
    State(pool): State<SqlitePool>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = token_from_cookies(&headers) {
        session::destroy(&pool, &token)
            .await
            .map_err(|e| ApiError::internal(e, "ending a session"))?;
    }

    // Cleared by expiring it in the past, which is the only way to remove a
    // cookie: there is no delete.
    let cookie =
        format!("{COOKIE_NAME}=; Path={COOKIE_PATH}; HttpOnly; SameSite=Strict; Max-Age=0");

    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)]).into_response())
}

/// Who am I
///
/// Reports the account this session belongs to. What the panel calls on load to
/// find out whether it should show a login form or itself.
#[utoipa::path(
    get,
    path = "/session",
    tag = "session",
    responses(
        (status = 200, description = "The account this session belongs to", body = Identity),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn current(
    panel: Panel,
    State(pool): State<SqlitePool>,
) -> Result<Json<Identity>, ApiError> {
    Identity::of(&pool, &panel.user, panel.expires_at)
        .await
        .map(Json)
}
