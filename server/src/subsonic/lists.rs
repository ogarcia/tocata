// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The lists a client fills its home screen with.

use super::asked::Asked;
use super::auth::Authenticated;
use super::browsing;
use super::error::ApiError;
use super::models::{AlbumId3, ArtistId3, Child, NamedEntry};
use super::response;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::error;

/// Albums returned when a client does not ask for a number.
const DEFAULT_SIZE: i64 = 10;

/// Ceiling the specification puts on one request.
const MAX_SIZE: i64 = 500;

/// Songs returned by getRandomSongs when unasked, and its ceiling.
const DEFAULT_RANDOM_SIZE: i64 = 10;
const MAX_RANDOM_SIZE: i64 = 500;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumListQuery {
    r#type: String,
    size: Option<i64>,
    offset: Option<i64>,
    from_year: Option<i64>,
    to_year: Option<i64>,
    genre: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomSongsQuery {
    size: Option<i64>,
    genre: Option<String>,
    from_year: Option<i64>,
    to_year: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongsByGenreQuery {
    genre: String,
    count: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumList2Body {
    album_list2: AlbumList2,
}

#[derive(Serialize)]
struct AlbumList2 {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    album: Vec<AlbumId3>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Starred2Body {
    starred2: Starred2,
}

#[derive(Serialize)]
struct Starred2 {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artist: Vec<ArtistId3>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    album: Vec<AlbumId3>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    song: Vec<Child>,
}

#[derive(Serialize)]
struct GenresBody {
    genres: Genres,
}

#[derive(Serialize)]
struct Genres {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    genre: Vec<Genre>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Genre {
    /// The name goes in the element's text, which the XML renderer does for a
    /// bare scalar and JSON turns into a plain field.
    value: String,
    song_count: i64,
    album_count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RandomSongsBody {
    random_songs: SongList,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SongsByGenreBody {
    songs_by_genre: SongList,
}

#[derive(Serialize)]
struct SongList {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    song: Vec<Child>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NowPlayingBody {
    now_playing: NowPlaying,
}

#[derive(Serialize)]
struct NowPlaying {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entry: Vec<NowPlayingEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NowPlayingEntry {
    #[serde(flatten)]
    song: Child,
    username: String,
    /// Minutes since the song was reported. What clients display beside the
    /// entry.
    minutes_ago: i64,
    player_name: String,
}

pub async fn get_album_list2(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<AlbumListQuery>,
) -> Response {
    let size = clamp(query.size, DEFAULT_SIZE, MAX_SIZE);
    let offset = query.offset.unwrap_or(0).max(0);

    let ids = match album_ids(&pool, auth.user.id, &query, size, offset).await {
        Ok(ids) => ids,
        Err(Rejected::Missing(name)) => {
            return ApiError::MissingParameter(name)
                .in_format(auth.format)
                .into_response();
        }
        Err(Rejected::UnknownType) => {
            // The type is the whole point of the call, so an unrecognised one is
            // a bad parameter rather than an empty list.
            return ApiError::MissingParameter("type")
                .in_format(auth.format)
                .into_response();
        }
        Err(Rejected::Database(e)) => return internal(e, auth.format, "listing albums"),
    };

    match browsing::load_albums_by_ids(&pool, auth.user.id, &ids).await {
        Ok(albums) => response::ok(
            auth.format,
            AlbumList2Body {
                album_list2: AlbumList2 { album: albums },
            },
        ),
        Err(e) => internal(e, auth.format, "loading listed albums"),
    }
}

pub async fn get_starred2(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let user_id = auth.user.id;

    let artists = match starred_ids(&pool, user_id, Starred_::Artists).await {
        Ok(ids) => match browsing::load_artists_by_ids(&pool, user_id, &ids).await {
            Ok(artists) => artists,
            Err(e) => return internal(e, auth.format, "loading starred artists"),
        },
        Err(e) => return internal(e, auth.format, "listing starred artists"),
    };

    let albums = match starred_ids(&pool, user_id, Starred_::Albums).await {
        Ok(ids) => match browsing::load_albums_by_ids(&pool, user_id, &ids).await {
            Ok(albums) => albums,
            Err(e) => return internal(e, auth.format, "loading starred albums"),
        },
        Err(e) => return internal(e, auth.format, "listing starred albums"),
    };

    let songs = match starred_ids(&pool, user_id, Starred_::Tracks).await {
        Ok(ids) => match browsing::load_tracks_by_ids(&pool, user_id, &ids).await {
            Ok(songs) => songs,
            Err(e) => return internal(e, auth.format, "loading starred songs"),
        },
        Err(e) => return internal(e, auth.format, "listing starred songs"),
    };

    response::ok(
        auth.format,
        Starred2Body {
            starred2: Starred2 {
                artist: artists,
                album: albums,
                song: songs,
            },
        },
    )
}

pub async fn get_genres(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    // Counts come from what is still present, so a genre whose files all went
    // away reports zero rather than promising music that cannot be played.
    let rows: Result<Vec<(String, i64, i64)>, _> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT g.name,
                    (SELECT count(*) FROM track_genres tg
                       JOIN tracks t ON t.id = tg.track_id
                      WHERE tg.genre_id = g.id
                        AND t.missing_since IS NULL
                        AND t.library_id IN (SELECT id FROM visible_libraries))
                        AS song_count,
                    (SELECT count(DISTINCT ag.album_id) FROM album_genres ag
                      WHERE ag.genre_id = g.id AND ",
        album_is_visible!("ag.album_id"),
        ") AS album_count
               FROM genres g
              ORDER BY g.name COLLATE NOCASE"
    ))
    .bind(auth.user.id)
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => response::ok(
            auth.format,
            GenresBody {
                genres: Genres {
                    genre: rows
                        .into_iter()
                        .map(|(value, song_count, album_count)| Genre {
                            value,
                            song_count,
                            album_count,
                        })
                        .collect(),
                },
            },
        ),
        Err(e) => internal(e, auth.format, "listing genres"),
    }
}

pub async fn get_random_songs(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<RandomSongsQuery>,
) -> Response {
    let size = clamp(query.size, DEFAULT_RANDOM_SIZE, MAX_RANDOM_SIZE);

    let ids: Result<Vec<i64>, _> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT t.id FROM tracks t
          WHERE t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
            AND (? IS NULL OR EXISTS (
                    SELECT 1 FROM track_genres tg
                      JOIN genres g ON g.id = tg.genre_id
                     WHERE tg.track_id = t.id AND g.name = ?
                ))
            AND (? IS NULL OR t.year >= ?)
            AND (? IS NULL OR t.year <= ?)
          ORDER BY random()
          LIMIT ?"
    ))
    .bind(auth.user.id)
    .bind(&query.genre)
    .bind(&query.genre)
    .bind(query.from_year)
    .bind(query.from_year)
    .bind(query.to_year)
    .bind(query.to_year)
    .bind(size)
    .fetch_all(&pool)
    .await;

    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => return internal(e, auth.format, "picking random songs"),
    };

    match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(songs) => response::ok(
            auth.format,
            RandomSongsBody {
                random_songs: SongList { song: songs },
            },
        ),
        Err(e) => internal(e, auth.format, "loading random songs"),
    }
}

pub async fn get_songs_by_genre(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<SongsByGenreQuery>,
) -> Response {
    let count = clamp(query.count, DEFAULT_SIZE, MAX_SIZE);
    let offset = query.offset.unwrap_or(0).max(0);

    let ids: Result<Vec<i64>, _> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT t.id FROM tracks t
           JOIN track_genres tg ON tg.track_id = t.id
           JOIN genres g ON g.id = tg.genre_id
          WHERE t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries) AND g.name = ?
          ORDER BY t.title COLLATE NOCASE
          LIMIT ? OFFSET ?"
    ))
    .bind(auth.user.id)
    .bind(&query.genre)
    .bind(count)
    .bind(offset)
    .fetch_all(&pool)
    .await;

    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => return internal(e, auth.format, "listing songs of a genre"),
    };

    match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(songs) => response::ok(
            auth.format,
            SongsByGenreBody {
                songs_by_genre: SongList { song: songs },
            },
        ),
        Err(e) => internal(e, auth.format, "loading songs of a genre"),
    }
}

pub async fn get_now_playing(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let rows = match playing_now(&pool, auth.user.id).await {
        Ok(rows) => rows,
        Err(e) => return internal(e, auth.format, "listing what is playing"),
    };

    let ids: Vec<i64> = rows.iter().map(|(id, _, _, _)| *id).collect();
    let songs = match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(songs) => songs,
        Err(e) => return internal(e, auth.format, "loading what is playing"),
    };

    // load_tracks_by_ids returns them in the order asked for, so zipping is safe.
    let entries = songs
        .into_iter()
        .zip(rows)
        .map(
            |(song, (_, username, player_name, minutes_ago))| NowPlayingEntry {
                song,
                username,
                minutes_ago: minutes_ago.max(0),
                player_name,
            },
        )
        .collect();

    response::ok(
        auth.format,
        NowPlayingBody {
            now_playing: NowPlaying { entry: entries },
        },
    )
}

/// Everybody's announcements that are still worth believing, newest first.
///
/// One row per player rather than per person: the table is keyed by client, so
/// somebody listening on their phone and on their desktop is two entries with two
/// player names, which is what a client showing this expects to see.
///
/// Announcements are not visibility checked when they are made — a client says
/// what it is playing, not what everyone may read — so the filtering happens
/// here, and what is playing out of a library this account cannot see is not
/// playing as far as this answer is concerned.
async fn playing_now(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<(i64, String, String, i64)>, sqlx::Error> {
    sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT np.track_id, u.username, np.client,
                cast((julianday('now') - julianday(np.started_at)) * 24 * 60 AS INTEGER)
           FROM now_playing np
           JOIN users u ON u.id = np.user_id
           JOIN tracks t ON t.id = np.track_id
          WHERE t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
            AND ",
        still_playing!("np.started_at", "t.duration_ms"),
        " ORDER BY np.started_at DESC"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await
}

fn internal(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}

/// Keeps a client from asking for the whole library in one call, and fills in
/// the default when it asks for nothing.
fn clamp(requested: Option<i64>, default: i64, max: i64) -> i64 {
    requested.unwrap_or(default).clamp(1, max)
}

enum Rejected {
    Missing(&'static str),
    UnknownType,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for Rejected {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Clone, Copy)]
enum Starred_ {
    Tracks,
    Albums,
    Artists,
}

/// What somebody has starred, of one kind, newest first.
///
/// Only what they starred: whether they may still see it is the loader's to
/// decide, and it does, for all three kinds alike. Half-deciding it here is what
/// broke this — the statement for songs named `visible_libraries` without the
/// expression that defines it, which SQLite answers with `no such table`, so
/// every client asking for its favourites got an error where a list should be.
/// The condition was not even needed: the loader drops what is missing and what
/// belongs to a library this person may not see.
async fn starred_ids(
    pool: &SqlitePool,
    user_id: i64,
    what: Starred_,
) -> Result<Vec<i64>, sqlx::Error> {
    // Newest favourite first, which is the order a client shows them in.
    let query = match what {
        Starred_::Tracks => {
            "SELECT s.track_id FROM user_track_stats s
              WHERE s.user_id = ? AND s.starred_at IS NOT NULL
              ORDER BY s.starred_at DESC"
        }
        Starred_::Albums => {
            "SELECT s.album_id FROM user_album_stats s
              WHERE s.user_id = ? AND s.starred_at IS NOT NULL
              ORDER BY s.starred_at DESC"
        }
        Starred_::Artists => {
            "SELECT s.artist_id FROM user_artist_stats s
              WHERE s.user_id = ? AND s.starred_at IS NOT NULL
              ORDER BY s.starred_at DESC"
        }
    };

    sqlx::query_scalar(query)
        .bind(user_id)
        .fetch_all(pool)
        .await
}

/// The album ids for one kind of list.
///
/// Each arm is its own statement rather than one query with a computed ORDER BY,
/// because sqlx will not take SQL assembled at runtime and because the arms
/// differ in more than their ordering: some filter, some join.
async fn album_ids(
    pool: &SqlitePool,
    user_id: i64,
    query: &AlbumListQuery,
    size: i64,
    offset: i64,
) -> Result<Vec<i64>, Rejected> {
    let ids = match query.r#type.as_str() {
        "random" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT id FROM albums WHERE ",
                album_is_visible!("albums.id"),
                " ORDER BY random() LIMIT ?"
            ))
            .bind(user_id)
            .bind(size)
            .fetch_all(pool)
            .await?
        }
        "newest" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT id FROM albums WHERE ",
                album_is_visible!("albums.id"),
                " ORDER BY created_at DESC LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "highest" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT s.album_id FROM user_album_stats s
                      WHERE s.user_id = ? AND s.rating IS NOT NULL AND ",
                album_is_visible!("s.album_id"),
                " ORDER BY s.rating DESC LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "frequent" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT s.album_id FROM user_album_stats s
                      WHERE s.user_id = ? AND s.play_count > 0 AND ",
                album_is_visible!("s.album_id"),
                " ORDER BY s.play_count DESC LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "recent" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT s.album_id FROM user_album_stats s
                      WHERE s.user_id = ? AND s.last_played IS NOT NULL AND ",
                album_is_visible!("s.album_id"),
                " ORDER BY s.last_played DESC LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "starred" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT s.album_id FROM user_album_stats s
                      WHERE s.user_id = ? AND s.starred_at IS NOT NULL AND ",
                album_is_visible!("s.album_id"),
                " ORDER BY s.starred_at DESC LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "alphabeticalByName" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT id FROM albums WHERE ",
                album_is_visible!("albums.id"),
                " ORDER BY coalesce(sort_name, name) COLLATE NOCASE LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "alphabeticalByArtist" => {
            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT al.id FROM albums al
                       LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.position = 0
                       LEFT JOIN artists ar ON ar.id = aa.artist_id
                      WHERE ",
                album_is_visible!("al.id"),
                " ORDER BY coalesce(ar.sort_name, ar.name) COLLATE NOCASE,
                               coalesce(al.sort_name, al.name) COLLATE NOCASE
                      LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "byYear" => {
            let from = query.from_year.ok_or(Rejected::Missing("fromYear"))?;
            let to = query.to_year.ok_or(Rejected::Missing("toYear"))?;

            // A client asking for 2010 down to 2000 wants them counting down,
            // which is how the specification describes a reversed range.
            let descending = from > to;
            let (low, high) = if descending { (to, from) } else { (from, to) };

            if descending {
                sqlx::query_scalar(concat!(
                    visible_libraries!(),
                    "SELECT id FROM albums WHERE year BETWEEN ? AND ? AND ",
                    album_is_visible!("albums.id"),
                    " ORDER BY year DESC LIMIT ? OFFSET ?"
                ))
                .bind(user_id)
                .bind(low)
                .bind(high)
                .bind(size)
                .bind(offset)
                .fetch_all(pool)
                .await?
            } else {
                sqlx::query_scalar(concat!(
                    visible_libraries!(),
                    "SELECT id FROM albums WHERE year BETWEEN ? AND ? AND ",
                    album_is_visible!("albums.id"),
                    " ORDER BY year LIMIT ? OFFSET ?"
                ))
                .bind(user_id)
                .bind(low)
                .bind(high)
                .bind(size)
                .bind(offset)
                .fetch_all(pool)
                .await?
            }
        }
        "byGenre" => {
            let genre = query.genre.as_deref().ok_or(Rejected::Missing("genre"))?;

            sqlx::query_scalar(concat!(
                visible_libraries!(),
                "SELECT DISTINCT al.id FROM albums al
                       JOIN album_genres ag ON ag.album_id = al.id
                       JOIN genres g ON g.id = ag.genre_id
                      WHERE g.name = ? AND ",
                album_is_visible!("al.id"),
                " ORDER BY coalesce(al.sort_name, al.name) COLLATE NOCASE
                      LIMIT ? OFFSET ?"
            ))
            .bind(user_id)
            .bind(genre)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        _ => return Err(Rejected::UnknownType),
    };

    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_kept_within_bounds() {
        assert_eq!(clamp(None, 10, 500), 10, "the default when unasked");
        assert_eq!(clamp(Some(50), 10, 500), 50);
        assert_eq!(clamp(Some(10_000), 10, 500), 500, "capped");
        assert_eq!(clamp(Some(0), 10, 500), 1, "zero would be a pointless call");
        assert_eq!(clamp(Some(-5), 10, 500), 1, "and so would a negative one");
    }

    #[test]
    fn a_genre_entry_puts_its_name_where_both_formats_want_it() {
        let value = serde_json::to_value(Genre {
            value: "Rock".into(),
            song_count: 3,
            album_count: 1,
        })
        .unwrap();

        assert_eq!(value["value"], "Rock");
        assert_eq!(value["songCount"], 3);
        assert_eq!(value["albumCount"], 1);
    }
}

#[cfg(test)]
mod starred_tests {
    use super::*;
    use crate::db;

    /// Two libraries holding a song, a record and a name each, all six starred,
    /// and somebody allowed only the first library.
    async fn a_favourite_of_each_kind_in_each_library() -> (SqlitePool, i64) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();

        for (id, name) in [(1, "kept"), (2, "hidden")] {
            sqlx::query(
                "INSERT INTO libraries (id, name, path, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(name)
            .bind(format!("/{name}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
                 VALUES (?, ?, ?, ?, '', 1)",
            )
            .bind(id)
            .bind(format!("f{id}"))
            .bind(id)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO albums (id, public_id, grouping_key, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("alb{id}"))
            .bind(format!("key {name}"))
            .bind(format!("Album {name}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO artists (id, public_id, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("art{id}"))
            .bind(format!("Artist {name}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                     file_size, file_modified_at, content_type, suffix, title,
                                     last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, 1, ?, 'audio/wav', 'wav', ?, 1, ?, ?)",
            )
            .bind(id)
            .bind(format!("trk{id}"))
            .bind(id)
            .bind(id)
            .bind(id)
            .bind(format!("/{name}/one.wav"))
            .bind(&at)
            .bind(format!("Song {name}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO track_artists (track_id, artist_id, role) VALUES (?, ?, 'artist')",
            )
            .bind(id)
            .bind(id)
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

        sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, 1)")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();

        // The one in the library they may not see was starred later, so it comes
        // first and a list that stopped filtering would show it at the top.
        for (id, when) in [(1, "2026-01-01T00:00:00Z"), (2, "2026-02-01T00:00:00Z")] {
            for statement in [
                "INSERT INTO user_track_stats (user_id, track_id, starred_at) VALUES (?, ?, ?)",
                "INSERT INTO user_album_stats (user_id, album_id, starred_at) VALUES (?, ?, ?)",
                "INSERT INTO user_artist_stats (user_id, artist_id, starred_at) VALUES (?, ?, ?)",
            ] {
                sqlx::query(statement)
                    .bind(user)
                    .bind(id)
                    .bind(when)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }

        (pool, user)
    }

    /// Every kind of favourite comes back, and only what this person may see.
    ///
    /// The statement for songs used to name `visible_libraries` without carrying
    /// the expression that defines it. SQLite answers that with `no such table`,
    /// and the endpoint hands back an error rather than a shorter list — so
    /// getStarred and getStarred2 were broken outright for every client while
    /// looking perfectly reasonable in the source.
    #[tokio::test]
    async fn every_kind_of_favourite_is_listed_and_none_of_a_library_barred() {
        let (pool, user) = a_favourite_of_each_kind_in_each_library().await;

        let songs = starred_ids(&pool, user, Starred_::Tracks).await.unwrap();
        let albums = starred_ids(&pool, user, Starred_::Albums).await.unwrap();
        let artists = starred_ids(&pool, user, Starred_::Artists).await.unwrap();

        assert_eq!(songs, vec![2, 1], "what they starred, newest first");
        assert_eq!(albums, vec![2, 1]);
        assert_eq!(artists, vec![2, 1]);

        // And the loaders are where being allowed to see it is decided.
        let songs = browsing::load_tracks_by_ids(&pool, user, &songs)
            .await
            .unwrap();
        assert_eq!(songs.len(), 1, "the song of the library they may see");
        assert_eq!(songs[0].title, "Song kept");

        let albums = browsing::load_albums_by_ids(&pool, user, &albums)
            .await
            .unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Album kept");

        let artists = browsing::load_artists_by_ids(&pool, user, &artists)
            .await
            .unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, "Artist kept");
    }
}

#[cfg(test)]
mod now_playing_tests {
    use super::*;
    use crate::db;

    /// Three songs of the lengths that matter — one ordinary, one long, one whose
    /// length the tags never said — and somebody to play them.
    async fn a_library() -> (SqlitePool, i64) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'music', '/music', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'music', '/music', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (id, title, duration) in [
            (1, "Short", Some(3 * 60 * 1000)),
            (2, "Long", Some(20 * 60 * 1000)),
            (3, "Untagged", None),
        ] {
            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                     file_modified_at, content_type, suffix, title, duration_ms,
                                     last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, 1, 1, ?, 1, ?, 'audio/wav', 'wav', ?, ?, 1, ?, ?)",
            )
            .bind(id)
            .bind(format!("trk{id}"))
            .bind(format!("/{title}.wav"))
            .bind(&at)
            .bind(title)
            .bind(duration)
            .bind(&at)
            .bind(&at)
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

    /// Announces a song as having started however long ago, in SQLite's own words
    /// so the stored text is the shape the server writes.
    async fn announced(pool: &SqlitePool, user: i64, client: &str, track: i64, ago: &str) {
        sqlx::query(
            "INSERT INTO now_playing (user_id, client, track_id, started_at)
             VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?))",
        )
        .bind(user)
        .bind(client)
        .bind(track)
        .bind(ago)
        .execute(pool)
        .await
        .unwrap();
    }

    fn players(rows: &[(i64, String, String, i64)]) -> Vec<&str> {
        rows.iter()
            .map(|(_, _, client, _)| client.as_str())
            .collect()
    }

    /// The question this is all for: one person, two devices, two entries. The
    /// table is keyed by client, so neither announcement stands on the other.
    #[tokio::test]
    async fn two_players_are_two_entries() {
        let (pool, ana) = a_library().await;

        announced(&pool, ana, "Phone", 1, "-30 seconds").await;
        announced(&pool, ana, "Desktop", 2, "-10 seconds").await;

        let rows = playing_now(&pool, ana).await.unwrap();

        assert_eq!(players(&rows), ["Desktop", "Phone"], "newest first");
    }

    /// The point of the window. A client that stops speaking mid-song is a client
    /// whose entry has to go by itself, or it is there for ever.
    #[tokio::test]
    async fn an_announcement_nobody_came_back_to_stops_counting() {
        let (pool, ana) = a_library().await;

        announced(&pool, ana, "Phone", 1, "-10 minutes").await;

        assert!(
            playing_now(&pool, ana).await.unwrap().is_empty(),
            "a three minute song cannot still be playing ten minutes later"
        );
    }

    /// And the reason the window is the song rather than a number: at ten minutes
    /// in, one of these two is over and the other is not.
    #[tokio::test]
    async fn a_long_song_outlasts_a_short_one() {
        let (pool, ana) = a_library().await;

        announced(&pool, ana, "Phone", 1, "-10 minutes").await;
        announced(&pool, ana, "Desktop", 2, "-10 minutes").await;

        let rows = playing_now(&pool, ana).await.unwrap();

        assert_eq!(players(&rows), ["Desktop"], "the twenty minute one");
    }

    /// A song whose length nobody tagged still has to expire, on the fallback.
    #[tokio::test]
    async fn a_song_of_unknown_length_expires_too() {
        let (pool, ana) = a_library().await;

        announced(&pool, ana, "Phone", 3, "-2 minutes").await;
        let rows = playing_now(&pool, ana).await.unwrap();
        assert_eq!(players(&rows), ["Phone"], "recent enough to believe");

        sqlx::query("DELETE FROM now_playing")
            .execute(&pool)
            .await
            .unwrap();

        announced(&pool, ana, "Phone", 3, "-30 minutes").await;
        assert!(playing_now(&pool, ana).await.unwrap().is_empty());
    }

    /// The margin exists for the client that announces a moment early and for two
    /// clocks that disagree, so the end of a song is not the end of its entry.
    #[tokio::test]
    async fn a_song_just_finished_is_given_a_moment() {
        let (pool, ana) = a_library().await;

        announced(&pool, ana, "Phone", 1, "-185 seconds").await;

        assert_eq!(
            players(&playing_now(&pool, ana).await.unwrap()),
            ["Phone"],
            "five seconds past a three minute song is not gone yet"
        );
    }

    /// Announcements are not checked against anybody's libraries when they are
    /// made, so the check has to be here — expiring things is no reason to stop
    /// filtering them.
    #[tokio::test]
    async fn what_plays_out_of_an_unreadable_library_is_not_listed() {
        let (pool, ana) = a_library().await;

        let nosy: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('nosy', 'x', 0, '', '') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Allowed a library that is not the one the music is in.
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (2, 'other', '/other', '', '')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, 2)")
            .bind(nosy)
            .execute(&pool)
            .await
            .unwrap();

        announced(&pool, ana, "Phone", 1, "-10 seconds").await;

        assert_eq!(players(&playing_now(&pool, ana).await.unwrap()), ["Phone"]);
        assert!(playing_now(&pool, nosy).await.unwrap().is_empty());
    }
}

// ---------------------------------------------------------------------------
// The older shapes
// ---------------------------------------------------------------------------
//
// Same data, serialised the way the pre-ID3 calls want it: albums as Child with
// isDir set, artists as the plainer object. Clients that only speak these are
// still out there, and supporting them is a matter of another wrapper rather
// than another query.

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumListBody {
    album_list: AlbumList,
}

#[derive(Serialize)]
struct AlbumList {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    album: Vec<Child>,
}

#[derive(Serialize)]
struct StarredBody {
    starred: Starred,
}

#[derive(Serialize)]
struct Starred {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artist: Vec<NamedEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    album: Vec<Child>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    song: Vec<Child>,
}

/// The pre-ID3 album list. Same selection as getAlbumList2, each album dressed
/// as a directory.
pub async fn get_album_list(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<AlbumListQuery>,
) -> Response {
    let size = clamp(query.size, DEFAULT_SIZE, MAX_SIZE);
    let offset = query.offset.unwrap_or(0).max(0);

    let ids = match album_ids(&pool, auth.user.id, &query, size, offset).await {
        Ok(ids) => ids,
        Err(Rejected::Missing(name)) => {
            return ApiError::MissingParameter(name)
                .in_format(auth.format)
                .into_response();
        }
        Err(Rejected::UnknownType) => {
            return ApiError::MissingParameter("type")
                .in_format(auth.format)
                .into_response();
        }
        Err(Rejected::Database(e)) => return internal(e, auth.format, "listing albums"),
    };

    match browsing::load_albums_by_ids(&pool, auth.user.id, &ids).await {
        Ok(albums) => response::ok(
            auth.format,
            AlbumListBody {
                album_list: AlbumList {
                    album: albums.iter().map(as_directory).collect(),
                },
            },
        ),
        Err(e) => internal(e, auth.format, "loading listed albums"),
    }
}

pub async fn get_starred(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let user_id = auth.user.id;

    let artists = match starred_ids(&pool, user_id, Starred_::Artists).await {
        Ok(ids) => match browsing::load_artists_by_ids(&pool, user_id, &ids).await {
            Ok(artists) => artists
                .into_iter()
                .map(|artist| NamedEntry {
                    id: artist.id,
                    name: artist.name,
                })
                .collect(),
            Err(e) => return internal(e, auth.format, "loading starred artists"),
        },
        Err(e) => return internal(e, auth.format, "listing starred artists"),
    };

    let albums = match starred_ids(&pool, user_id, Starred_::Albums).await {
        Ok(ids) => match browsing::load_albums_by_ids(&pool, user_id, &ids).await {
            Ok(albums) => albums.iter().map(as_directory).collect(),
            Err(e) => return internal(e, auth.format, "loading starred albums"),
        },
        Err(e) => return internal(e, auth.format, "listing starred albums"),
    };

    let songs = match starred_ids(&pool, user_id, Starred_::Tracks).await {
        Ok(ids) => match browsing::load_tracks_by_ids(&pool, user_id, &ids).await {
            Ok(songs) => songs,
            Err(e) => return internal(e, auth.format, "loading starred songs"),
        },
        Err(e) => return internal(e, auth.format, "listing starred songs"),
    };

    response::ok(
        auth.format,
        StarredBody {
            starred: Starred {
                artist: artists,
                album: albums,
                song: songs,
            },
        },
    )
}

/// An album as the older calls describe one: a directory carrying the album's
/// own fields.
pub(super) fn as_directory(album: &AlbumId3) -> Child {
    let mut child = Child::directory(album.id.clone(), album.name.clone(), None);
    child.album = Some(album.name.clone());
    child.artist = album.display_artist.clone();
    child.artist_id = album.artist_id.clone();
    child.year = album.year;
    child.genre = album.genre.clone();
    child.cover_art = album.cover_art.clone();
    child.duration = Some(album.duration);
    child.created = Some(album.created.clone());
    child.starred = album.starred.clone();
    child.user_rating = album.user_rating;
    child.play_count = album.play_count;
    child
}
