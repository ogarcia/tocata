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

impl Panel {
    /// A second handle on the same session, for a test that calls more than one
    /// handler with it. Not `Clone`, because outside a test there is one request
    /// and one session and copying it would mean something went wrong.
    #[cfg(test)]
    pub(crate) fn clone_for_test(&self) -> Self {
        Self {
            id: self.id,
            user: crate::user::User {
                id: self.user.id,
                username: self.user.username.clone(),
                is_admin: self.user.is_admin,
            },
            expires_at: self.expires_at.clone(),
        }
    }
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
    headers: HeaderMap,
    Json(credentials): Json<Credentials>,
) -> Result<Response, ApiError> {
    // Before the password is even hashed. Argon2 is deliberately slow, so a
    // server that hashed first would be one that could be made to spend its
    // whole processor on guesses.
    if attempts.barred(from.ip()) {
        return Err(ApiError::TooManyAttempts);
    }

    let authenticated =
        user::authenticate_panel(&pool, &credentials.username, &credentials.password)
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

    // Read here and not inside the session, which is a mechanism and has no
    // business knowing that this one came in over HTTP: what it is given is a
    // sentence, and what asks for the sentence is the thing holding the request.
    let said = crate::browser::as_said(
        headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|header| header.to_str().ok()),
    );

    let (token, expires_at) = session::create(&pool, user.id, days, said)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth, db, settings};

    const PASSWORD: &str = "the one she chose";

    /// One account, and the settings a login reads to know how long to last.
    async fn a_server() -> (SqlitePool, Arc<Attempts>) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        settings::seed(&pool, &[]).await.unwrap();

        let at = db::now();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', ?, 0, ?, ?)",
        )
        .bind(auth::hash_password(PASSWORD).unwrap())
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        (pool, Arc::new(Attempts::new()))
    }

    fn from(address: &str) -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::new(address.parse().unwrap(), 51000))
    }

    /// The one header a login reads, as a browser would have sent it.
    fn saying(user_agent: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::USER_AGENT,
            user_agent.parse().expect("a header a browser could send"),
        );

        headers
    }

    fn credentials(password: &str, remember: bool) -> Json<Credentials> {
        Json(Credentials {
            username: "ana".to_string(),
            password: password.to_string(),
            remember,
        })
    }

    /// What the browser said it was is written down as it said it, which is the
    /// whole of what later lets somebody tell their own open sessions apart. Read
    /// back through the reader rather than compared as a string: what matters is
    /// that the sentence arrived whole enough to be read.
    #[tokio::test]
    async fn logging_in_writes_down_what_the_browser_said_it_was() {
        let (pool, attempts) = a_server().await;

        log_in(
            State(pool.clone()),
            State(attempts),
            from("203.0.113.7"),
            saying(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/17.4 Safari/605.1.15",
            ),
            credentials(PASSWORD, true),
        )
        .await
        .expect("the password is hers");

        let said: Option<String> = sqlx::query_scalar("SELECT user_agent FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(
            crate::browser::read(&said.expect("the header arrived with the request")),
            (Some("Safari"), Some("macOS")),
        );
    }

    /// And a login from something that is not a browser leaves the column empty
    /// rather than storing a sentence nobody wrote.
    #[tokio::test]
    async fn a_login_that_said_nothing_is_a_row_that_says_nothing() {
        let (pool, attempts) = a_server().await;

        log_in(
            State(pool.clone()),
            State(attempts),
            from("203.0.113.7"),
            HeaderMap::new(),
            credentials(PASSWORD, true),
        )
        .await
        .unwrap();

        let said: Option<String> = sqlx::query_scalar("SELECT user_agent FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(said, None);
    }

    /// What a `Set-Cookie` header said, if there was one.
    fn cookie(response: &Response) -> Option<String> {
        response
            .headers()
            .get(SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_string())
    }

    /// The token inside it, which is what a session is.
    fn token(response: &Response) -> String {
        let cookie = cookie(response).expect("a login hands back a cookie");
        cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{COOKIE_NAME}="))
            .to_string()
    }

    /// The right password opens a session that can then be used.
    #[tokio::test]
    async fn logging_in_hands_back_a_way_in_that_works() {
        let (pool, attempts) = a_server().await;

        let response = log_in(
            State(pool.clone()),
            State(attempts),
            from("203.0.113.7"),
            HeaderMap::new(),
            credentials(PASSWORD, true),
        )
        .await
        .expect("the password is hers");

        let session = session::resolve(&pool, &token(&response))
            .await
            .unwrap()
            .expect("the cookie names a session that exists");
        assert_eq!(session.user.username, "ana");
    }

    /// Whether the browser keeps the way in after it closes is the whole of what
    /// "remember me" means here, and it is one attribute of one header. The row
    /// lives just as long either way, which is why this is worth pinning: nothing
    /// else in the answer changes.
    #[tokio::test]
    async fn remembering_is_the_only_difference_between_the_two_logins() {
        let (pool, attempts) = a_server().await;

        let kept = log_in(
            State(pool.clone()),
            State(attempts.clone()),
            from("203.0.113.7"),
            HeaderMap::new(),
            credentials(PASSWORD, true),
        )
        .await
        .unwrap();
        assert!(
            cookie(&kept).unwrap().contains("Max-Age="),
            "asked to be remembered, the cookie outlives the browser"
        );

        let forgotten = log_in(
            State(pool.clone()),
            State(attempts),
            from("203.0.113.7"),
            HeaderMap::new(),
            credentials(PASSWORD, false),
        )
        .await
        .unwrap();
        assert!(
            !cookie(&forgotten).unwrap().contains("Max-Age="),
            "not asked, the browser drops it when it closes"
        );

        assert!(
            session::resolve(&pool, &token(&forgotten))
                .await
                .unwrap()
                .is_some(),
            "and the session itself is there either way"
        );
    }

    /// A wrong password hands back nothing at all — no cookie, and so no session
    /// to try.
    #[tokio::test]
    async fn a_wrong_password_opens_nothing() {
        let (pool, attempts) = a_server().await;

        let refused = log_in(
            State(pool.clone()),
            State(attempts),
            from("203.0.113.7"),
            HeaderMap::new(),
            credentials("not hers", true),
        )
        .await
        .expect_err("a wrong password is refused");

        assert!(matches!(refused, ApiError::WrongCredentials));

        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0, "nothing was opened");
    }

    /// Guessing is stopped before the password is hashed, and stopped for the
    /// place the guesses come from rather than for the account — otherwise anybody
    /// could lock anybody else out by getting their password wrong on purpose.
    #[tokio::test]
    async fn guessing_is_barred_by_where_it_comes_from() {
        let (pool, attempts) = a_server().await;

        for _ in 0..6 {
            let _ = log_in(
                State(pool.clone()),
                State(attempts.clone()),
                from("203.0.113.7"),
                HeaderMap::new(),
                credentials("not hers", true),
            )
            .await;
        }

        let barred = log_in(
            State(pool.clone()),
            State(attempts.clone()),
            from("203.0.113.7"),
            HeaderMap::new(),
            credentials(PASSWORD, true),
        )
        .await
        .expect_err("even the right password waits now");
        assert!(matches!(barred, ApiError::TooManyAttempts));

        log_in(
            State(pool.clone()),
            State(attempts),
            from("198.51.100.4"),
            HeaderMap::new(),
            credentials(PASSWORD, true),
        )
        .await
        .expect("and she can still log in from anywhere else");
    }

    /// Logging out ends this session and no others, and clears the cookie the only
    /// way a cookie can be cleared.
    #[tokio::test]
    async fn logging_out_ends_this_session_and_leaves_the_others() {
        let (pool, attempts) = a_server().await;

        let first = token(
            &log_in(
                State(pool.clone()),
                State(attempts.clone()),
                from("203.0.113.7"),
                HeaderMap::new(),
                credentials(PASSWORD, true),
            )
            .await
            .unwrap(),
        );
        let elsewhere = token(
            &log_in(
                State(pool.clone()),
                State(attempts),
                from("198.51.100.4"),
                HeaderMap::new(),
                credentials(PASSWORD, true),
            )
            .await
            .unwrap(),
        );

        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, format!("{COOKIE_NAME}={first}").parse().unwrap());

        let response = log_out(State(pool.clone()), headers).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            cookie(&response).unwrap().contains("Max-Age=0"),
            "a cookie is removed by being expired, since there is no delete"
        );

        assert!(
            session::resolve(&pool, &first).await.unwrap().is_none(),
            "this one is over"
        );
        assert!(
            session::resolve(&pool, &elsewhere).await.unwrap().is_some(),
            "the browser at the other end of the house is left alone"
        );
    }

    /// Documented to answer the same with or without one, because a panel that has
    /// already forgotten its cookie still has a Log out button to press.
    #[tokio::test]
    async fn logging_out_without_a_session_is_still_logging_out() {
        let (pool, _) = a_server().await;

        let response = log_out(State(pool), HeaderMap::new()).await.unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(cookie(&response).unwrap().contains("Max-Age=0"));
    }
}
