// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The OpenSubsonic API, served under `/rest`.

// Defined before the modules so they can all see it: a `macro_rules!` is in scope
// from where it is written to the end of the enclosing module, nested modules
// included.

/// The libraries this request may look at, hoisted to the front of the statement.
///
/// A library counts when it is switched on and the person asking is allowed it:
/// either they have no restriction at all, which is the ordinary case and costs
/// nothing, or they have one naming this library.
///
/// It is a common table expression rather than a predicate repeated inline, and
/// that is what makes the parameter count predictable. The identifier is named
/// once here however many times the filter is needed further down, so every
/// statement that uses this takes it as its **first** bind and nothing else moves.
///
/// The subquery does not correlate with the statement around it, so SQLite works
/// it out once rather than per row.
///
/// Forgetting the expression is loud — `no such table: visible_libraries` —
/// and forgetting the bind is not, but it fails closed: a null identifier matches
/// no user, the set comes out empty, and the answer is nothing rather than
/// somebody else's music.
macro_rules! visible_libraries_head {
    () => {
        "
    WITH visible_libraries (id) AS (
        SELECT l.id FROM libraries l
         WHERE l.enabled = 1
           AND EXISTS (
                   SELECT 1 FROM users u
                    WHERE u.id = "
    };
}

macro_rules! visible_libraries_tail {
    () => {
        "
                      AND (NOT EXISTS (SELECT 1 FROM user_libraries ul
                                        WHERE ul.user_id = u.id)
                           OR EXISTS (SELECT 1 FROM user_libraries ul
                                       WHERE ul.user_id = u.id
                                         AND ul.library_id = l.id))
               )
    )
"
    };
}

/// The whole expression, for the callers that bind the user themselves.
///
/// Split in two above for the same reason the column lists are: a `QueryBuilder`
/// cannot take an argument without also writing its own `?`, so a statement it
/// assembles has to be given the pieces either side of the parameter.
macro_rules! visible_libraries {
    () => {
        concat!(visible_libraries_head!(), "?", visible_libraries_tail!())
    };
}

/// Whether a thing has at least one track worth showing.
///
/// A track is worth showing when its file is still there and its library is
/// switched on. Everything above a track — an album, an artist — is visible
/// exactly when one of its tracks is, so this is the one place that decides it.
///
/// `$join` reaches the tracks in question and must end in its own `WHERE`, since
/// the conditions below are appended to it.
macro_rules! has_a_visible_track {
    ($join:expr) => {
        concat!(
            "EXISTS (SELECT 1 FROM tracks t ",
            $join,
            " AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries))"
        )
    };
}

/// An album with something in it.
macro_rules! album_is_visible {
    ($album:literal) => {
        has_a_visible_track!(concat!("WHERE t.album_id = ", $album))
    };
}

/// An artist credited on a track, or on an album that still has one.
macro_rules! artist_is_visible {
    ($artist:literal) => {
        concat!(
            "(",
            has_a_visible_track!(concat!(
                "JOIN track_artists ta ON ta.track_id = t.id WHERE ta.artist_id = ",
                $artist
            )),
            " OR ",
            has_a_visible_track!(concat!(
                "JOIN album_artists aa ON aa.album_id = t.album_id WHERE aa.artist_id = ",
                $artist
            )),
            ")"
        )
    };
}

mod annotation;
mod auth;
mod bookmarks;
mod browsing;
mod error;
mod lists;
mod media;
mod models;
mod playlists;
mod response;
mod search;
mod system;
mod unsupported;
mod users;
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
        "getLyrics" => media::get_lyrics,
        "getLyricsBySongId" => media::get_lyrics_by_song_id,
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
        "getBookmarks" => bookmarks::get_bookmarks,
        "createBookmark" => bookmarks::create_bookmark,
        "deleteBookmark" => bookmarks::delete_bookmark,
        "getPlayQueue" => bookmarks::get_play_queue,
        "savePlayQueue" => bookmarks::save_play_queue,
        "getUser" => users::get_user,
        "getUsers" => users::get_users,
        "createUser" => users::create_user,
        "updateUser" => users::update_user,
        "deleteUser" => users::delete_user,
        "changePassword" => users::change_password,
        "getAlbumList" => lists::get_album_list,
        "getStarred" => lists::get_starred,
        "search2" => search::search2,
        // Out of scope. A listing comes back empty; anything naming one thing
        // comes back as not found. See the unsupported module for why they are
        // registered at all.
        "getVideos" => unsupported::get_videos,
        "getChatMessages" => unsupported::get_chat_messages,
        "getShares" => unsupported::get_shares,
        "getPodcasts" => unsupported::get_podcasts,
        "getInternetRadioStations" => unsupported::get_internet_radio_stations,
        "getVideoInfo" => unsupported::not_found,
        "getCaptions" => unsupported::not_found,
        "hls.m3u8" => unsupported::not_found,
        "getAvatar" => unsupported::not_found,
        "getArtistInfo" => unsupported::not_found,
        "getArtistInfo2" => unsupported::not_found,
        "getAlbumInfo" => unsupported::not_found,
        "getAlbumInfo2" => unsupported::not_found,
        "getSimilarSongs" => unsupported::not_found,
        "getSimilarSongs2" => unsupported::not_found,
        "getTopSongs" => unsupported::not_found,
    }
    .with_state(state)
}
