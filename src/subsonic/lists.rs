// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The lists a client fills its home screen with.

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
    Query(query): Query<AlbumListQuery>,
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
    let rows: Result<Vec<(String, i64, i64)>, _> = sqlx::query_as(
        "SELECT g.name,
                (SELECT count(*) FROM track_genres tg
                   JOIN tracks t ON t.id = tg.track_id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NULL) AS song_count,
                (SELECT count(DISTINCT ag.album_id) FROM album_genres ag
                  WHERE ag.genre_id = g.id) AS album_count
           FROM genres g
          ORDER BY g.name COLLATE NOCASE",
    )
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
    Query(query): Query<RandomSongsQuery>,
) -> Response {
    let size = clamp(query.size, DEFAULT_RANDOM_SIZE, MAX_RANDOM_SIZE);

    let ids: Result<Vec<i64>, _> = sqlx::query_scalar(
        "SELECT t.id FROM tracks t
          WHERE t.missing_since IS NULL
            AND (? IS NULL OR EXISTS (
                    SELECT 1 FROM track_genres tg
                      JOIN genres g ON g.id = tg.genre_id
                     WHERE tg.track_id = t.id AND g.name = ?
                ))
            AND (? IS NULL OR t.year >= ?)
            AND (? IS NULL OR t.year <= ?)
          ORDER BY random()
          LIMIT ?",
    )
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
    Query(query): Query<SongsByGenreQuery>,
) -> Response {
    let count = clamp(query.count, DEFAULT_SIZE, MAX_SIZE);
    let offset = query.offset.unwrap_or(0).max(0);

    let ids: Result<Vec<i64>, _> = sqlx::query_scalar(
        "SELECT t.id FROM tracks t
           JOIN track_genres tg ON tg.track_id = t.id
           JOIN genres g ON g.id = tg.genre_id
          WHERE t.missing_since IS NULL AND g.name = ?
          ORDER BY t.title COLLATE NOCASE
          LIMIT ? OFFSET ?",
    )
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
    let rows: Result<Vec<(i64, String, String, i64)>, _> = sqlx::query_as(
        "SELECT np.track_id, u.username, np.client,
                cast((julianday('now') - julianday(np.started_at)) * 24 * 60 AS INTEGER)
           FROM now_playing np
           JOIN users u ON u.id = np.user_id
           JOIN tracks t ON t.id = np.track_id
          WHERE t.missing_since IS NULL
          ORDER BY np.started_at DESC",
    )
    .fetch_all(&pool)
    .await;

    let rows = match rows {
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

async fn starred_ids(
    pool: &SqlitePool,
    user_id: i64,
    what: Starred_,
) -> Result<Vec<i64>, sqlx::Error> {
    // Newest favourite first, which is the order a client shows them in.
    let query = match what {
        Starred_::Tracks => {
            "SELECT s.track_id FROM user_track_stats s
               JOIN tracks t ON t.id = s.track_id
              WHERE s.user_id = ? AND s.starred_at IS NOT NULL
                AND t.missing_since IS NULL
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
            sqlx::query_scalar("SELECT id FROM albums ORDER BY random() LIMIT ?")
                .bind(size)
                .fetch_all(pool)
                .await?
        }
        "newest" => {
            sqlx::query_scalar("SELECT id FROM albums ORDER BY created_at DESC LIMIT ? OFFSET ?")
                .bind(size)
                .bind(offset)
                .fetch_all(pool)
                .await?
        }
        "highest" => {
            sqlx::query_scalar(
                "SELECT s.album_id FROM user_album_stats s
                  WHERE s.user_id = ? AND s.rating IS NOT NULL
                  ORDER BY s.rating DESC LIMIT ? OFFSET ?",
            )
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "frequent" => {
            sqlx::query_scalar(
                "SELECT s.album_id FROM user_album_stats s
                  WHERE s.user_id = ? AND s.play_count > 0
                  ORDER BY s.play_count DESC LIMIT ? OFFSET ?",
            )
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "recent" => {
            sqlx::query_scalar(
                "SELECT s.album_id FROM user_album_stats s
                  WHERE s.user_id = ? AND s.last_played IS NOT NULL
                  ORDER BY s.last_played DESC LIMIT ? OFFSET ?",
            )
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "starred" => {
            sqlx::query_scalar(
                "SELECT s.album_id FROM user_album_stats s
                  WHERE s.user_id = ? AND s.starred_at IS NOT NULL
                  ORDER BY s.starred_at DESC LIMIT ? OFFSET ?",
            )
            .bind(user_id)
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "alphabeticalByName" => {
            sqlx::query_scalar(
                "SELECT id FROM albums
                  ORDER BY coalesce(sort_name, name) COLLATE NOCASE LIMIT ? OFFSET ?",
            )
            .bind(size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        "alphabeticalByArtist" => {
            sqlx::query_scalar(
                "SELECT al.id FROM albums al
                   LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.position = 0
                   LEFT JOIN artists ar ON ar.id = aa.artist_id
                  ORDER BY coalesce(ar.sort_name, ar.name) COLLATE NOCASE,
                           coalesce(al.sort_name, al.name) COLLATE NOCASE
                  LIMIT ? OFFSET ?",
            )
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
                sqlx::query_scalar(
                    "SELECT id FROM albums WHERE year BETWEEN ? AND ?
                      ORDER BY year DESC LIMIT ? OFFSET ?",
                )
                .bind(low)
                .bind(high)
                .bind(size)
                .bind(offset)
                .fetch_all(pool)
                .await?
            } else {
                sqlx::query_scalar(
                    "SELECT id FROM albums WHERE year BETWEEN ? AND ?
                      ORDER BY year LIMIT ? OFFSET ?",
                )
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

            sqlx::query_scalar(
                "SELECT DISTINCT al.id FROM albums al
                   JOIN album_genres ag ON ag.album_id = al.id
                   JOIN genres g ON g.id = ag.genre_id
                  WHERE g.name = ?
                  ORDER BY coalesce(al.sort_name, al.name) COLLATE NOCASE
                  LIMIT ? OFFSET ?",
            )
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
    Query(query): Query<AlbumListQuery>,
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
