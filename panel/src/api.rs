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
use tocata::types::{Credentials, Identity, Stats};
use web_sys::RequestCredentials;

/// Relative, because the panel is served by the server it talks to. Nothing to
/// configure and nothing to get wrong across deployments.
const BASE: &str = "/api/v1";

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
    Request::post(&url(path))
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
        _ => {
            // The server sends a stable code and an English message. The code is
            // what we keep: the panel says it in the reader's own language.
            let code = response
                .json::<tocata::types::ErrorBody>()
                .await
                .map(|body| body.code)
                .unwrap_or_else(|_| "unknown".to_string());
            Err(Failure::Refused(code))
        }
    }
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
