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

/// What the first search took, which is a field per thing rather than one box.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    artist: Option<String>,
    album: Option<String>,
    title: Option<String>,
    any: Option<String>,
    count: Option<i64>,
    offset: Option<i64>,
    /// Milliseconds since the epoch. Only what arrived after it.
    newer_than: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchBody {
    search_result: SearchResult,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    offset: i64,
    total_hits: i64,
    /// `match` is a keyword, so the field is named for what it holds and the
    /// element keeps the name the protocol gave it.
    #[serde(rename = "match", skip_serializing_if = "Vec::is_empty")]
    matched: Vec<Child>,
}

/// The first search there ever was: songs only, one field per thing looked for.
///
/// Deprecated in favour of search2 four versions later and kept by every server
/// since, which is reason enough — a client old enough to ask for it is a client
/// with nothing else to fall back on, and what it got here until now was an HTTP
/// 404 it cannot tell from a broken server.
///
/// Each field is asked of the column that holds it, which the index makes exact:
/// `artist=Björk&title=Hyper` looks for the one among her songs and not for a
/// record called Hyper. `any` is asked of all three. Given nothing at all it
/// answers with the library, the way search3 does, because that is what the
/// specification says an empty search means.
pub async fn search(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let count = query.count.unwrap_or(DEFAULT_COUNT);
    let offset = query.offset.unwrap_or(0);
    let newer_than = query.newer_than.map(crate::db::from_epoch_millis);

    let matched = match by_field(&query) {
        Some(expression) => Matched::Search(expression),
        // Nothing to go on. Which is the whole library when nothing was typed at
        // all, and nothing when what was typed cannot match — the same two
        // answers search3 gives, decided the same way.
        None if asked_nothing(&query) => Matched::Everything,
        None => Matched::Nothing,
    };

    let (ids, total) = match matches(
        &pool,
        auth.user.id,
        &matched,
        newer_than.as_deref(),
        count,
        offset,
    )
    .await
    {
        Ok(found) => found,
        Err(e) => return failed(e, auth.format, "searching songs"),
    };

    match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(songs) => response::ok(
            auth.format,
            SearchBody {
                search_result: SearchResult {
                    offset,
                    total_hits: total,
                    matched: songs,
                },
            },
        ),
        Err(e) => failed(e, auth.format, "loading what a search found"),
    }
}

/// Whether the request named nothing to look for, which is not the same as naming
/// something unsearchable.
fn asked_nothing(query: &SearchQuery) -> bool {
    [&query.artist, &query.album, &query.title, &query.any]
        .into_iter()
        .all(|field| field.as_deref().unwrap_or_default().trim().is_empty())
}

/// The fields as one FTS5 expression, each asked of its own column.
///
/// The columns are the index's own — `title`, `album`, `artists` — and a filter on
/// one of them is written `{artists} : (…)`. Fields given together are all
/// required, which is what a form with several boxes filled in means.
fn by_field(query: &SearchQuery) -> Option<String> {
    let mut parts = Vec::new();

    for (column, given) in [
        (Some("title"), &query.title),
        (Some("artists"), &query.artist),
        (Some("album"), &query.album),
        (None, &query.any),
    ] {
        let Some(expression) = given.as_deref().and_then(crate::search::wanted) else {
            continue;
        };

        parts.push(match column {
            Some(column) => format!("{{{column}}} : ({expression})"),
            None => expression,
        });
    }

    (!parts.is_empty()).then(|| parts.join(" AND "))
}

/// The songs of one page, and how many there are in all.
///
/// The total comes back on every row rather than from a second statement:
/// `count(*) OVER ()` is worked out over everything that matched, before the limit
/// takes a page out of it. Which leaves one case to be plain about — a page past
/// the end has no rows to carry the number, and answers zero. A client that has
/// paged off the end and is told there is nothing more has been told the truth,
/// even if it arrived by that route.
async fn matches(
    pool: &SqlitePool,
    user_id: i64,
    matched: &Matched,
    newer_than: Option<&str>,
    count: i64,
    offset: i64,
) -> Result<(Vec<i64>, i64), sqlx::Error> {
    let rows: Vec<(i64, i64)> = match matched {
        Matched::Nothing => Vec::new(),
        Matched::Search(expression) => {
            sqlx::query_as(concat!(
                visible_libraries!(),
                "SELECT f.rowid, count(*) OVER ()
                   FROM tracks_fts f
                   JOIN tracks t ON t.id = f.rowid
                  WHERE tracks_fts MATCH ? AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)
                    AND (? IS NULL OR t.created_at >= ?)
                  ORDER BY f.rank
                  LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(expression)
            .bind(newer_than)
            .bind(newer_than)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        Matched::Everything => {
            sqlx::query_as(concat!(
                visible_libraries!(),
                "SELECT t.id, count(*) OVER ()
                   FROM tracks t
                  WHERE t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)
                    AND (? IS NULL OR t.created_at >= ?)
                  ORDER BY t.title COLLATE NOCASE
                  LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(newer_than)
            .bind(newer_than)
            .bind(count)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };

    let total = rows.first().map(|(_, total)| *total).unwrap_or(0);

    Ok((rows.into_iter().map(|(id, _)| id).collect(), total))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// Three songs, two of them with "hyper" somewhere, and one of those two with
    /// it in the record's name instead of the song's — which is the difference the
    /// first search is able to express and search3 is not.
    async fn a_shelf() -> (SqlitePool, i64) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'shelf', '/shelf', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'shelf', '/shelf', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (id, title, album, artists, when) in [
            (1, "Hyperballad", "Post", "Björk", "2020-01-01T00:00:00Z"),
            (2, "Hyper Hyper", "Hyper", "Scooter", "2026-06-01T00:00:00Z"),
            (3, "Army of Me", "Post", "Björk", "2020-01-01T00:00:00Z"),
        ] {
            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                     file_modified_at, content_type, suffix, title,
                                     last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, 1, 1, ?, 1, ?, 'audio/wav', 'wav', ?, 1, ?, ?)",
            )
            .bind(id)
            .bind(format!("trk{id}"))
            .bind(format!("/shelf/{id}.wav"))
            .bind(&at)
            .bind(title)
            .bind(when)
            .bind(when)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO tracks_fts (rowid, title, album, artists) VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(title)
            .bind(album)
            .bind(artists)
            .execute(&pool)
            .await
            .unwrap();
        }

        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&at)
        .bind(&at)
        .fetch_one(&pool)
        .await
        .unwrap();

        (pool, user)
    }

    /// A field is asked of the column that holds it. Without that this call is
    /// search3 with extra steps, and the answer to "her song called Hyper
    /// something" would include somebody else's record of that name.
    #[tokio::test]
    async fn each_field_is_asked_of_its_own_column() {
        let (pool, user) = a_shelf().await;

        let query = SearchQuery {
            artist: Some("Björk".into()),
            album: None,
            title: Some("Hyper".into()),
            any: None,
            count: None,
            offset: None,
            newer_than: None,
        };

        let expression = by_field(&query).expect("both fields are searchable");
        let (ids, total) = matches(&pool, user, &Matched::Search(expression), None, 20, 0)
            .await
            .unwrap();

        assert_eq!(ids, [1], "hers, and not the record called Hyper");
        assert_eq!(total, 1);
    }

    /// `any` is the box that asks all three, which is where the other one shows up.
    #[tokio::test]
    async fn anything_asks_every_column() {
        let (pool, user) = a_shelf().await;

        let query = SearchQuery {
            artist: None,
            album: None,
            title: None,
            any: Some("hyper".into()),
            count: None,
            offset: None,
            newer_than: None,
        };

        let expression = by_field(&query).unwrap();
        let (ids, total) = matches(&pool, user, &Matched::Search(expression), None, 20, 0)
            .await
            .unwrap();

        assert_eq!(ids.len(), 2, "the song and the record");
        assert_eq!(total, 2);
    }

    /// The count takes a page and the total still counts the lot, which is what a
    /// client pages by.
    #[tokio::test]
    async fn a_page_says_how_much_there_is_behind_it() {
        let (pool, user) = a_shelf().await;

        let expression = by_field(&SearchQuery {
            artist: None,
            album: None,
            title: None,
            any: Some("hyper".into()),
            count: None,
            offset: None,
            newer_than: None,
        })
        .unwrap();

        let (ids, total) = matches(&pool, user, &Matched::Search(expression), None, 1, 0)
            .await
            .unwrap();

        assert_eq!(ids.len(), 1, "one to a page");
        assert_eq!(total, 2, "and two in all");
    }

    /// Only what arrived after the moment given, which is how a client that has
    /// been away asks what is new.
    #[tokio::test]
    async fn newer_than_leaves_the_older_one_out() {
        let (pool, user) = a_shelf().await;

        let expression = by_field(&SearchQuery {
            artist: None,
            album: None,
            title: None,
            any: Some("hyper".into()),
            count: None,
            offset: None,
            newer_than: None,
        })
        .unwrap();

        let (ids, total) = matches(
            &pool,
            user,
            &Matched::Search(expression),
            Some("2026-01-01T00:00:00Z"),
            20,
            0,
        )
        .await
        .unwrap();

        assert_eq!(ids, [2], "the one that arrived this year");
        assert_eq!(total, 1);
    }

    /// Nothing asked for at all is the whole shelf, which is how a client fills a
    /// library it means to keep offline.
    #[tokio::test]
    async fn an_empty_search_is_the_whole_shelf() {
        let (pool, user) = a_shelf().await;

        let query = SearchQuery {
            artist: None,
            album: None,
            title: None,
            any: None,
            count: None,
            offset: None,
            newer_than: None,
        };

        assert!(by_field(&query).is_none());
        assert!(asked_nothing(&query), "nothing was named");

        let (ids, total) = matches(&pool, user, &Matched::Everything, None, 20, 0)
            .await
            .unwrap();

        assert_eq!(ids.len(), 3);
        assert_eq!(total, 3);
    }

    /// Punctuation is not an empty search: something was typed, and it cannot
    /// match. Handing over the library would be a strange answer to it.
    #[tokio::test]
    async fn a_search_that_cannot_match_is_not_a_search_for_everything() {
        let (pool, user) = a_shelf().await;

        let query = SearchQuery {
            artist: None,
            album: None,
            title: None,
            any: Some("\"".into()),
            count: None,
            offset: None,
            newer_than: None,
        };

        assert!(by_field(&query).is_none());
        assert!(!asked_nothing(&query), "a quote is still something typed");

        let (ids, total) = matches(&pool, user, &Matched::Nothing, None, 20, 0)
            .await
            .unwrap();

        assert!(ids.is_empty());
        assert_eq!(total, 0);
    }
}
