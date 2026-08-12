// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The OpenSubsonic API, served under `/rest`.

mod annotation;
mod asked;
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
///
/// It writes down the names as well as registering them, so [`REGISTERED`] and
/// the router come out of the same list. A second list kept by hand is a list
/// that falls behind the first the day somebody adds an endpoint, and the whole
/// value of walking them in a test is that the walk cannot miss one.
macro_rules! endpoints {
    ($($path:literal => $handler:path),* $(,)?) => {
        pub fn router(state: AppState) -> Router {
            Router::new()
                $(
                    .route(concat!("/", $path), get($handler))
                    .route(concat!("/", $path, ".view"), get($handler))
                )*
                .with_state(state)
        }

        /// Every name this API answers to, for the test that calls them all.
        #[cfg(test)]
        const REGISTERED: &[&str] = &[$($path),*];
    };
}

endpoints! {
    "ping" => system::ping,
        "getLicense" => system::get_license,
        "getOpenSubsonicExtensions" => system::get_open_subsonic_extensions,
        "tokenInfo" => system::token_info,
        "getScanStatus" => system::get_scan_status,
        "startScan" => system::start_scan,
        "getMusicFolders" => browsing::get_music_folders,
        "getArtists" => browsing::get_artists,
        "getArtist" => browsing::get_artist,
        "getAlbum" => browsing::get_album,
        "getSong" => browsing::get_song,
        "getTopSongs" => browsing::get_top_songs,
        "getArtistInfo" => browsing::get_artist_info,
        "getArtistInfo2" => browsing::get_artist_info2,
        // One handler for both, because both answer in `albumInfo`.
        "getAlbumInfo" => browsing::get_album_info,
        "getAlbumInfo2" => browsing::get_album_info,
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
        "search" => search::search,
        // Out of scope. A listing comes back empty; anything naming one thing
        // comes back as not found. See the unsupported module for why they are
        // registered at all.
        "getVideos" => unsupported::get_videos,
        "getChatMessages" => unsupported::get_chat_messages,
        "addChatMessage" => unsupported::not_found,
        "getShares" => unsupported::get_shares,
        "getPodcasts" => unsupported::get_podcasts,
        "getInternetRadioStations" => unsupported::get_internet_radio_stations,
        "getVideoInfo" => unsupported::not_found,
        "getCaptions" => unsupported::not_found,
        "hls.m3u8" => unsupported::not_found,
        "getAvatar" => unsupported::not_found,
        "getSimilarSongs" => unsupported::not_found,
        "getSimilarSongs2" => unsupported::not_found,
}

#[cfg(test)]
mod every_endpoint {
    use super::*;
    use crate::{attempts, auth, config::Config, db, net, resources, scanner, settings};
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use std::sync::Arc;
    use tokio::sync::watch;
    use tower::ServiceExt;

    /// A key rather than a password, and the reason is the clock: a password is
    /// verified with argon2, which is deliberately slow, and sixty-three requests
    /// of it turned this one test into seventeen seconds — four times the whole
    /// suite. A key is checked against a digest. Both reach the same extractor,
    /// which is all this test is trying to get past.
    const KEY: &str = "a-key-for-the-test";

    /// A server with somebody who may use it, and nothing else.
    ///
    /// Empty on purpose. A listing with nothing in it is the state every server
    /// passes through on its first day, and it is the one where a statement that
    /// cannot run still has to run: what this catches is SQL that is wrong
    /// whatever the data, which is what the favourites were.
    async fn a_server() -> (axum::Router, String) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        settings::seed(&pool, &[]).await.unwrap();

        let at = db::now();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', ?, 1, ?, ?)",
        )
        .bind(auth::hash_password("unused").unwrap())
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO api_keys (user_id, key_hash, label, created_at)
             VALUES (1, ?, 'the test', ?)",
        )
        .bind(auth::hash_secret(KEY))
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let state = AppState {
            pool,
            scan: Arc::new(scanner::Progress::default()),
            portraits: Arc::new(crate::portraits::Fetching::default()),
            attempts: Arc::new(attempts::Attempts::new()),
            config: Arc::new(Config::for_tests(
                std::env::temp_dir().join("tocata-every-endpoint"),
            )),
            meter: Arc::new(resources::Meter::new().unwrap()),
            net: net::Net::new(),
            shutdown: watch::channel(false).1,
        };

        // The extension is explicit that a key travels alone, so no `u` beside it.
        let credentials = format!("apiKey={KEY}&c=test&v=1.16.1");

        (router(state), credentials)
    }

    /// Every endpoint answers, and none of them answers with our own mistake.
    ///
    /// It says nothing about whether the answers are right — that is what the
    /// tests beside each handler are for. It says the statement runs, which is
    /// not a low bar: getStarred and getStarred2 named a table that only exists
    /// when the expression defining it is carried along, and every client asking
    /// for its favourites got error 0 back for as long as the endpoint existed.
    /// Nothing here knew, because nothing here called them.
    ///
    /// Walking [`REGISTERED`] rather than a list written out again is what makes
    /// this cover the endpoint somebody adds next month without them doing
    /// anything.
    #[tokio::test]
    async fn answers_without_a_fault_of_ours() {
        let (router, credentials) = a_server().await;

        let mut broken = Vec::new();

        // Both shapes, because the trouble travels inside the body and each
        // format spells it differently — a check written for one of them reads
        // the other as a success. XML is what a client that asked for nothing
        // gets, so it is not the exotic case.
        for (format, generic) in [("json", r#""code":0"#), ("xml", r#"code="0""#)] {
            for name in REGISTERED {
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(format!("/{name}?{credentials}&f={format}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                    .await
                    .unwrap();
                let body = String::from_utf8_lossy(&body).replace('\n', " ");

                // OpenSubsonic answers 200 whatever happened and puts the trouble
                // in the body, so the status alone would call every failure a
                // success. Code 0 is the generic one, which is how a statement
                // that will not run comes back — never how a wrong request does,
                // since those have codes of their own.
                if status.is_server_error() || body.contains(generic) {
                    broken.push(format!("{name} as {format}: {status} {body}"));
                }
            }
        }

        assert!(
            broken.is_empty(),
            "endpoints answering with a fault of ours:\n  {}",
            broken.join("\n  ")
        );
    }

    /// A call asked for without what it needs answers in the protocol.
    ///
    /// It used to answer with axum's own refusal: HTTP 400 and a line of English in
    /// the body. A client has no way to read that — everything else here, a wrong
    /// password included, arrives as 200 with the trouble inside — and several
    /// treat it as the server being broken and stop asking. Twelve calls were
    /// answering that way, and the walk above could not see it: a 400 is not a
    /// server error and carries no code 0.
    #[tokio::test]
    async fn a_call_asked_for_without_its_parameter_is_answered_in_the_protocol() {
        let (router, credentials) = a_server().await;

        for (format, code, message) in [
            (
                "json",
                r#""code":10"#,
                r#""Required parameter id is missing""#,
            ),
            (
                "xml",
                r#"code="10""#,
                r#"message="Required parameter id is missing""#,
            ),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/getAlbum?{credentials}&f={format}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                axum::http::StatusCode::OK,
                "the transport succeeded; the call did not"
            );

            let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            let body = String::from_utf8_lossy(&body).into_owned();

            assert!(body.contains(code), "as {format}: {body}");
            assert!(
                body.contains(message),
                "and it says which parameter, as {format}: {body}"
            );
        }
    }
}
