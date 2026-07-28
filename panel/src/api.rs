// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Every call to `/api/v1`, in one place.
//!
//! The shapes come from the server's own crate, so a field that moves there
//! stops this compiling. What is left here is the plumbing: the cookie, the
//! method, and turning a failure into something a screen can show.

use gloo_net::http::{Request, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tocata::types::{
    Account, AccountChanges, Closed, Credentials, Identity, Key, Library, LibraryAccess,
    LibraryChanges, NewAccount, NewKey, NewLibrary, Revoked, Stats,
};
use web_sys::RequestCredentials;

/// Relative, because the panel is served by the server it talks to. Nothing to
/// configure and nothing to get wrong across deployments.
const BASE: &str = "/api/v1";

/// Where the event stream lives. Public because `EventSource` opens it itself
/// rather than going through anything here.
pub const EVENTS: &str = "/api/v1/events";

/// What went wrong, in the terms a screen cares about.
///
/// The distinction that matters is between "your session is gone" — which sends
/// the whole panel back to the login form — and anything else, which is a
/// message inside the screen that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 401: no session, or one that has run out.
    Unauthenticated,
    /// The server said no, and said why in a code we pass along untranslated.
    Refused(String),
    /// It did not answer at all.
    Unreachable,
}

/// The cookie is `HttpOnly`, so nothing here reads it; the browser sends it and
/// `SameOrigin` is what makes it do so.
///
/// Building can fail on a malformed URL, which would be our own mistake rather
/// than anything that happened on the network — but it arrives as the same kind
/// of failure, since a screen can do nothing different about it either way.
fn get(path: &str) -> Result<Request, Failure> {
    build(Request::get(&url(path)))
}

fn delete(path: &str) -> Result<Request, Failure> {
    build(Request::delete(&url(path)))
}

/// A POST with a body, which is the only reason a request here carries one.
fn post<T: Serialize>(path: &str, body: &T) -> Result<Request, Failure> {
    with_body(Request::post(&url(path)), body)
}

/// A PATCH, for the calls that change part of something.
fn patch<T: Serialize>(path: &str, body: &T) -> Result<Request, Failure> {
    with_body(Request::patch(&url(path)), body)
}

/// A PUT, for the one call that replaces a whole set rather than changing part
/// of one.
fn put<T: Serialize>(path: &str, body: &T) -> Result<Request, Failure> {
    with_body(Request::put(&url(path)), body)
}

fn with_body<T: Serialize>(request: RequestBuilder, body: &T) -> Result<Request, Failure> {
    request
        .credentials(RequestCredentials::SameOrigin)
        .json(body)
        .map_err(|_| Failure::Unreachable)
}

fn url(path: &str) -> String {
    format!("{BASE}{path}")
}

fn build(request: RequestBuilder) -> Result<Request, Failure> {
    request
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(|_| Failure::Unreachable)
}

/// Sends a request and reads the answer, mapping every way it can go wrong.
async fn read<T: DeserializeOwned>(request: Request) -> Result<T, Failure> {
    let response = request.send().await.map_err(|_| Failure::Unreachable)?;

    match response.status() {
        200..=299 => response.json().await.map_err(|_| Failure::Unreachable),
        401 => Err(Failure::Unauthenticated),
        _ => Err(refused(response).await),
    }
}

/// The server sends a stable code and an English message. The code is what we
/// keep: the panel says it in the reader's own language.
async fn refused(response: gloo_net::http::Response) -> Failure {
    let code = response
        .json::<tocata::types::ErrorBody>()
        .await
        .map(|body| body.code)
        .unwrap_or_else(|_| "unknown".to_string());

    Failure::Refused(code)
}

/// Who the cookie belongs to, if it belongs to anybody.
pub async fn whoami() -> Result<Identity, Failure> {
    read(get("/session")?).await
}

pub async fn log_in(username: String, password: String) -> Result<Identity, Failure> {
    read(post("/session", &Credentials { username, password })?).await
}

/// Ends this session. A failure here changes nothing worth telling: the panel is
/// going back to the login form either way.
pub async fn log_out() {
    if let Ok(request) = delete("/session") {
        let _ = request.send().await;
    }
}

pub async fn stats() -> Result<Stats, Failure> {
    read(get("/stats")?).await
}

/// Starts one. `full` reads every file again instead of trusting size and
/// modification time.
pub async fn start_scan(full: bool) -> Result<(), Failure> {
    let path = if full { "/scan?full=true" } else { "/scan" };

    // Nothing worth reading comes back: what happens next arrives on the stream.
    match Request::post(&url(path))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(|_| Failure::Unreachable)?
        .send()
        .await
    {
        Ok(response) if response.ok() => Ok(()),
        Ok(response) if response.status() == 401 => Err(Failure::Unauthenticated),
        Ok(response) => Err(Failure::Refused(response.status().to_string())),
        Err(_) => Err(Failure::Unreachable),
    }
}

/// Every library, in the order the server lists them.
pub async fn libraries() -> Result<Vec<Library>, Failure> {
    read(get("/libraries")?).await
}

/// Adds one. The path has to exist on the server, which is the one thing this
/// cannot check from here.
pub async fn add_library(path: String, name: Option<String>) -> Result<Library, Failure> {
    read(post("/libraries", &NewLibrary { path, name })?).await
}

/// Renames one, switches it on or off, or both.
pub async fn change_library(id: i64, changes: LibraryChanges) -> Result<Library, Failure> {
    read(patch(&format!("/libraries/{id}"), &changes)?).await
}

/// Removes one, and with it everything scanned from it. The server refuses while
/// the library is still enabled, which arrives here as a `Refused`.
pub async fn remove_library(id: i64) -> Result<(), Failure> {
    plain(delete(&format!("/libraries/{id}"))?).await
}

/// Sends a request whose answer carries nothing worth reading, and maps the ways
/// it can go wrong the same way everything else does.
async fn plain(request: Request) -> Result<(), Failure> {
    match request.send().await {
        Ok(response) if response.ok() => Ok(()),
        Ok(response) if response.status() == 401 => Err(Failure::Unauthenticated),
        Ok(response) => Err(refused(response).await),
        Err(_) => Err(Failure::Unreachable),
    }
}

/// Every account. Only an administrator may ask.
pub async fn accounts() -> Result<Vec<Account>, Failure> {
    read(get("/users")?).await
}

/// One account. Yours, or anybody's if you administer the server.
pub async fn account(username: &str) -> Result<Account, Failure> {
    read(get(&format!("/users/{username}"))?).await
}

pub async fn add_account(new: NewAccount) -> Result<Account, Failure> {
    read(post("/users", &new)?).await
}

pub async fn change_account(username: &str, changes: AccountChanges) -> Result<Account, Failure> {
    read(patch(&format!("/users/{username}"), &changes)?).await
}

pub async fn remove_account(username: &str) -> Result<(), Failure> {
    plain(delete(&format!("/users/{username}"))?).await
}

/// Which libraries an account may see. An empty list means no restriction, which
/// is not the same as seeing nothing.
pub async fn restrict(username: &str, libraries: Vec<i64>) -> Result<Account, Failure> {
    read(put(
        &format!("/users/{username}/libraries"),
        &LibraryAccess { libraries },
    )?)
    .await
}

/// The keys an account holds, without the keys themselves.
pub async fn keys(username: &str) -> Result<Vec<Key>, Failure> {
    read(get(&format!("/users/{username}/keys"))?).await
}

/// Issues one. This is the only time the key itself can be read.
pub async fn issue_key(username: &str, new: NewKey) -> Result<tocata::types::IssuedKey, Failure> {
    read(post(&format!("/users/{username}/keys"), &new)?).await
}

/// Gives a key a new secret and keeps the rest. Readable once, like a new one.
pub async fn rotate_key(username: &str, id: i64) -> Result<tocata::types::IssuedKey, Failure> {
    read(post(&format!("/users/{username}/keys/{id}/rotate"), &())?).await
}

pub async fn revoke_key(username: &str, id: i64) -> Result<(), Failure> {
    plain(delete(&format!("/users/{username}/keys/{id}"))?).await
}

/// Cuts an account off from every client holding a key.
pub async fn revoke_keys(username: &str) -> Result<Revoked, Failure> {
    read(delete(&format!("/users/{username}/keys"))?).await
}

/// The panel logins an account has open.
pub async fn sessions(username: &str) -> Result<Vec<tocata::types::Login>, Failure> {
    read(get(&format!("/users/{username}/sessions"))?).await
}

pub async fn close_session(username: &str, id: i64) -> Result<(), Failure> {
    plain(delete(&format!("/users/{username}/sessions/{id}"))?).await
}

/// Closes all of them, this one included when the account is yours.
pub async fn close_sessions(username: &str) -> Result<Closed, Failure> {
    read(delete(&format!("/users/{username}/sessions"))?).await
}

/// Asks the running scan to give up. What it had written is thrown away by the
/// server, so this is not a pause.
pub async fn cancel_scan() -> Result<(), Failure> {
    match delete("/scan")?.send().await {
        Ok(response) if response.ok() => Ok(()),
        Ok(response) if response.status() == 401 => Err(Failure::Unauthenticated),
        Ok(response) => Err(Failure::Refused(response.status().to_string())),
        Err(_) => Err(Failure::Unreachable),
    }
}
