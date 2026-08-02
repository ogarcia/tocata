// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Searching the catalogue.

use super::auth::Authenticated;
use super::browsing;
use super::error::ApiError;
use super::models::{AlbumId3, ArtistId3, Child, NamedEntry};
use super::response;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::error;

/// What the specification says to return when a client does not say otherwise.
const DEFAULT_COUNT: i64 = 20;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Search3Query {
    query: Option<String>,
    artist_count: Option<i64>,
    artist_offset: Option<i64>,
    album_count: Option<i64>,
    album_offset: Option<i64>,
    song_count: Option<i64>,
    song_offset: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Search3Body {
    search_result3: SearchResult3,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult3 {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artist: Vec<ArtistId3>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    album: Vec<AlbumId3>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    song: Vec<Child>,
}

pub async fn search3(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<Search3Query>,
) -> Response {
    let user_id = auth.user.id;
    let terms = query.query.as_deref().unwrap_or_default();

    let matched = match crate::search::wanted(terms) {
        Some(expression) => Matched::Search(expression),
        // An absent or blank query means everything, which the specification
        // requires so a client can pull the whole library down for offline use.
        None if terms.trim().is_empty() => Matched::Everything,
        // Something was typed, but nothing in it can match: a lone quote, or
        // punctuation. Handing back the entire library would be a strange answer
        // to a search.
        None => Matched::Nothing,
    };

    let artists = match load_artists(
        &pool,
        user_id,
        &matched,
        query.artist_count.unwrap_or(DEFAULT_COUNT),
        query.artist_offset.unwrap_or(0),
    )
    .await
    {
        Ok(artists) => artists,
        Err(e) => return failed(e, auth.format, "searching artists"),
    };

    let albums = match load_albums(
        &pool,
        user_id,
        &matched,
        query.album_count.unwrap_or(DEFAULT_COUNT),
        query.album_offset.unwrap_or(0),
    )
    .await
    {
        Ok(albums) => albums,
        Err(e) => return failed(e, auth.format, "searching albums"),
    };

    let songs = match load_songs(
        &pool,
        user_id,
        &matched,
        query.song_count.unwrap_or(DEFAULT_COUNT),
        query.song_offset.unwrap_or(0),
    )
    .await
    {
        Ok(songs) => songs,
        Err(e) => return failed(e, auth.format, "searching songs"),
    };

    response::ok(
        auth.format,
        Search3Body {
            search_result3: SearchResult3 {
                artist: artists,
                album: albums,
                song: songs,
            },
        },
    )
}

fn failed(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}

enum Matched {
    /// A full text expression, already escaped.
    Search(String),
    /// No query at all: list everything.
    Everything,
    /// A query with no searchable term in it.
    Nothing,
}

/// Turns what a person typed into an FTS5 expression.
///
/// This has to be done: FTS5 has a query syntax of its own, with AND, OR, NOT,
/// NEAR and quoting, so passing a raw string through means an unbalanced quote
/// becomes a syntax error and a five hundred, and a search for "and" matches
/// nothing at all. Each word is quoted as a literal instead.
///
async fn load_artists(
    pool: &SqlitePool,
    user_id: i64,
    matched: &Matched,
    count: i64,
    offset: i64,
) -> Result<Vec<ArtistId3>, sqlx::Error> {
    let ids: Vec<i64> = match matched {
        Matched::Nothing => Vec::new(),
        Matched::Search(expression) => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT f.rowid FROM artists_fts f
                       JOIN artists a ON a.id = f.rowid
                      WHERE artists_fts MATCH ? AND ",
                artist_is_visible!("a.id"),
                " ORDER BY f.rank
                      LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(expression)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        Matched::Everything => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT id FROM artists WHERE ",
                artist_is_visible!("artists.id"),
                " ORDER BY coalesce(sort_name, name) COLLATE NOCASE
                      LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };

    browsing::load_artists_by_ids(pool, user_id, &ids).await
}

async fn load_albums(
    pool: &SqlitePool,
    user_id: i64,
    matched: &Matched,
    count: i64,
    offset: i64,
) -> Result<Vec<AlbumId3>, sqlx::Error> {
    let ids: Vec<i64> = match matched {
        Matched::Nothing => Vec::new(),
        Matched::Search(expression) => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT f.rowid FROM albums_fts f
                       JOIN albums a ON a.id = f.rowid
                      WHERE albums_fts MATCH ? AND ",
                album_is_visible!("a.id"),
                " ORDER BY f.rank
                      LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(expression)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        Matched::Everything => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT id FROM albums WHERE ",
                album_is_visible!("albums.id"),
                " ORDER BY coalesce(sort_name, name) COLLATE NOCASE
                      LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };

    browsing::load_albums_by_ids(pool, user_id, &ids).await
}

async fn load_songs(
    pool: &SqlitePool,
    user_id: i64,
    matched: &Matched,
    count: i64,
    offset: i64,
) -> Result<Vec<Child>, sqlx::Error> {
    // Absent tracks are filtered here rather than removed from the index: the
    // row stays so the user's data does, and a file that comes back becomes
    // searchable again without reindexing.
    let ids: Vec<i64> = match matched {
        Matched::Nothing => Vec::new(),
        Matched::Search(expression) => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT f.rowid FROM tracks_fts f
                   JOIN tracks t ON t.id = f.rowid
                  WHERE tracks_fts MATCH ? AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)
                  ORDER BY f.rank
                  LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(expression)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        Matched::Everything => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT id FROM tracks
                  WHERE missing_since IS NULL
                    AND library_id IN (SELECT id FROM visible_libraries)
                  ORDER BY title COLLATE NOCASE
                  LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };

    browsing::load_tracks_by_ids(pool, user_id, &ids).await
}

/// The pre-ID3 search. Same selection, albums dressed as directories.
pub async fn search2(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<Search3Query>,
) -> Response {
    let user_id = auth.user.id;
    let terms = query.query.as_deref().unwrap_or_default();

    let matched = match crate::search::wanted(terms) {
        Some(expression) => Matched::Search(expression),
        None if terms.trim().is_empty() => Matched::Everything,
        None => Matched::Nothing,
    };

    let artists = match load_artists(
        &pool,
        user_id,
        &matched,
        query.artist_count.unwrap_or(DEFAULT_COUNT),
        query.artist_offset.unwrap_or(0),
    )
    .await
    {
        Ok(artists) => artists
            .into_iter()
            .map(|artist| NamedEntry {
                id: artist.id,
                name: artist.name,
            })
            .collect(),
        Err(e) => return failed(e, auth.format, "searching artists"),
    };

    let albums = match load_albums(
        &pool,
        user_id,
        &matched,
        query.album_count.unwrap_or(DEFAULT_COUNT),
        query.album_offset.unwrap_or(0),
    )
    .await
    {
        Ok(albums) => albums.iter().map(super::lists::as_directory).collect(),
        Err(e) => return failed(e, auth.format, "searching albums"),
    };

    let songs = match load_songs(
        &pool,
        user_id,
        &matched,
        query.song_count.unwrap_or(DEFAULT_COUNT),
        query.song_offset.unwrap_or(0),
    )
    .await
    {
        Ok(songs) => songs,
        Err(e) => return failed(e, auth.format, "searching songs"),
    };

    response::ok(
        auth.format,
        Search2Body {
            search_result2: SearchResult2 {
                artist: artists,
                album: albums,
                song: songs,
            },
        },
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Search2Body {
    search_result2: SearchResult2,
}

#[derive(Serialize)]
struct SearchResult2 {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artist: Vec<NamedEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    album: Vec<Child>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    song: Vec<Child>,
}
