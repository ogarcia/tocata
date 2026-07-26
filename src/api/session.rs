// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Logging in and out of the panel, and the extractor everything else sits
//! behind.

use super::error::{ApiError, ErrorBody};
use crate::user::User;
use crate::{session, user};
use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, RequestPartsExt};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

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
/// Carries nothing, because so far the only question asked of it is whether the
/// caller may, not which of them it is.
pub struct Administrator;

impl<S> FromRequestParts<S> for Administrator
where
    SqlitePool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Panel { user, .. } = parts.extract_with_state::<Panel, S>(state).await?;

        if user.is_admin {
            Ok(Self)
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

/// What a login asks for.
#[derive(Deserialize, ToSchema)]
pub struct Credentials {
    #[schema(example = "admin")]
    username: String,
    password: String,
}

/// Who is logged in.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    #[schema(example = "admin")]
    username: String,
    /// Whether this account administers the server, which is what decides how
    /// much of the panel is worth drawing.
    admin: bool,
    /// When the session stops working, so the panel can say so before it does
    /// rather than after a call has already failed.
    #[schema(example = "2026-08-25T18:00:00Z")]
    expires_at: String,
}

impl Identity {
    fn of(user: &User, expires_at: String) -> Self {
        Self {
            username: user.username.clone(),
            admin: user.is_admin,
            expires_at,
        }
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
    )
)]
pub async fn log_in(
    State(pool): State<SqlitePool>,
    Json(credentials): Json<Credentials>,
) -> Result<Response, ApiError> {
    let authenticated =
        user::authenticate_password(&pool, &credentials.username, &credentials.password)
            .await
            .map_err(|e| ApiError::internal(e, "authenticating a panel login"))?;

    let Some(user) = authenticated else {
        return Err(ApiError::WrongCredentials);
    };

    let (token, expires_at) = session::create(&pool, user.id)
        .await
        .map_err(|e| ApiError::internal(e, "creating a session"))?;

    let identity = Identity::of(&user, expires_at);
    let cookie = format!(
        "{COOKIE_NAME}={token}; Path={COOKIE_PATH}; HttpOnly; SameSite=Strict; Max-Age={}",
        session::lifetime_seconds()
    );

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
pub async fn current(panel: Panel) -> Json<Identity> {
    Json(Identity::of(&panel.user, panel.expires_at))
}
