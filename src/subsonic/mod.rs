// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The OpenSubsonic API, served under `/rest`.

mod auth;
mod browsing;
mod error;
mod models;
mod response;
mod system;
mod xml;

use crate::state::AppState;
use axum::Router;
use axum::routing::get;

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

pub fn router(state: AppState) -> Router {
    endpoints! {
        "ping" => system::ping,
        "getLicense" => system::get_license,
        "getOpenSubsonicExtensions" => system::get_open_subsonic_extensions,
        "getScanStatus" => system::get_scan_status,
        "startScan" => system::start_scan,
        "getMusicFolders" => browsing::get_music_folders,
        "getArtists" => browsing::get_artists,
        "getArtist" => browsing::get_artist,
        "getAlbum" => browsing::get_album,
        "getSong" => browsing::get_song,
        "getIndexes" => browsing::get_indexes,
        "getMusicDirectory" => browsing::get_music_directory,
    }
    .with_state(state)
}
