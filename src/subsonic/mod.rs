// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The OpenSubsonic API, served under `/rest`.

mod auth;
mod error;
mod response;
mod system;
mod xml;

use axum::Router;
use axum::routing::get;
use sqlx::SqlitePool;

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

pub fn router(pool: SqlitePool) -> Router {
    endpoints! {
        "ping" => system::ping,
        "getLicense" => system::get_license,
        "getOpenSubsonicExtensions" => system::get_open_subsonic_extensions,
    }
    .with_state(pool)
}
