// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The OpenSubsonic API, served under `/rest`.

mod annotation;
mod auth;
mod browsing;
mod error;
mod lists;
mod media;
mod models;
mod playlists;
mod response;
mod search;
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
        "stream" => media::stream,
        "download" => media::download,
        "getCoverArt" => media::get_cover_art,
        "search3" => search::search3,
        "star" => annotation::star,
        "unstar" => annotation::unstar,
        "setRating" => annotation::set_rating,
        "scrobble" => annotation::scrobble,
        "getAlbumList2" => lists::get_album_list2,
        "getStarred2" => lists::get_starred2,
        "getGenres" => lists::get_genres,
        "getRandomSongs" => lists::get_random_songs,
        "getSongsByGenre" => lists::get_songs_by_genre,
        "getNowPlaying" => lists::get_now_playing,
        "getPlaylists" => playlists::get_playlists,
        "getPlaylist" => playlists::get_playlist,
        "createPlaylist" => playlists::create_playlist,
        "updatePlaylist" => playlists::update_playlist,
        "deletePlaylist" => playlists::delete_playlist,
    }
    .with_state(state)
}
