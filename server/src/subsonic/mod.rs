// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The OpenSubsonic API, served under `/rest`.

/// Whether a song announced as playing could still be playing.
///
/// A client says "now playing" when a song starts, and says it again for the next
/// one. Nothing obliges it to say anything when it stops: a phone that runs out of
/// battery, an app killed by the system or a browser tab closed all leave their
/// last announcement behind with nobody to replace it. Without a window that
/// entry stays for good, and the list of what is playing becomes part answer and
/// part graveyard.
///
/// How long a song could be playing is how long the song is. A minute is added for
/// a client that announced a little early, for one that reports the whole queue at
/// once, and for two clocks that do not quite agree. A length we do not know falls
/// back to five minutes: longer than most songs, and short enough to forget.
///
/// Nothing here can be read as a promise that the song stopped. It says only that
/// we no longer have grounds to claim it did not, which is the honest answer for
/// a client that stopped speaking.
///
/// `$started` and `$duration` name the columns to read, the second in
/// milliseconds.
macro_rules! still_playing {
    ($started:literal, $duration:literal) => {
        concat!(
            "(julianday('now') - julianday(",
            $started,
            ")) * 86400 < coalesce(",
            $duration,
            " / 1000, 300) + 60"
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
