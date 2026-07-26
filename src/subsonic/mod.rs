// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The OpenSubsonic API, served under `/rest`.

mod response;
mod system;
mod xml;

use axum::Router;
use axum::routing::get;
use serde::Deserialize;

/// Parameters every endpoint of the API accepts. Only what is read today:
/// authentication adds its own as it lands.
#[derive(Debug, Deserialize)]
pub struct CommonParams {
    /// Requested response format.
    pub f: Option<String>,
}

/// Registers each endpoint twice, because clients are free to append `.view`
/// to any of them and plenty do.
macro_rules! endpoints {
    ($($path:literal => $handler:path),* $(,)?) => {
        Router::new()
            $(
                .route(concat!("/", $path), get($handler))
                .route(concat!("/", $path, ".view"), get($handler))
            )*
    };
}

pub fn router() -> Router {
    endpoints! {
        "ping" => system::ping,
    }
}
