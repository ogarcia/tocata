// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Browsing what has been scanned: tracks, albums, artists and genres.
//!
//! Four listings that answer the same way — a page and a total — because the
//! screens over them work the same way: an endless list that asks for more when
//! it reaches the bottom, and a heading that counts what a search has narrowed
//! things to. The total is what makes both possible; without it a list cannot
//! tell "nothing more" from "not yet".
//!
//! Every one of them is filtered by which libraries the person asking may see,
//! and every one of them hides tracks whose files are gone — except the track
//! listing itself, which shows them and says so, because a listing that quietly
//! dropped them would be a listing that disagreed with the number the Overview
//! prints.
//!
//! The searches go through FTS5 with the last word marked as a prefix, so they
//! answer while somebody is still typing. Genres have no index of their own:
//! there are a few dozen of them and a scan of a few dozen rows costs less than
//! the table that would avoid it.

use super::error::ApiError;
use super::session::Panel;
use crate::types::{
    Album, Albums, Artist, Artists, ErrorBody, Genre, Genres, Queue, Track, Tracks,
};
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use utoipa::IntoParams;

/// How many rows a page holds when nobody says. Enough to fill a tall window
/// once, so the first screenful takes one request.
const PAGE: i64 = 50;

/// And the most anybody may ask for at once. A listing is for reading; asking for
/// everything is what the queue is for, and it answers in identifiers.
const MOST: i64 = 200;

/// What a listing has been narrowed to.
///
/// All four are optional and they compound: an artist and a genre together is
/// that artist's songs in that genre. Which of them a given listing reads is
/// whichever of them could narrow it — an album has no album to belong to.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    /// Words to look for. The last one counts as a prefix, so this answers as it
    /// is typed.
    pub search: Option<String>,
    /// The public identifier of an album.
    pub album: Option<String>,
    /// The public identifier of an artist.
    pub artist: Option<String>,
    /// A genre, by name.
    pub genre: Option<String>,
}

/// Where in the listing, for a list that asks for the next few as it is scrolled.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Paging {
    #[serde(default)]
    pub offset: i64,
    pub limit: Option<i64>,
}

impl Paging {
    /// The window, with an unreasonable one brought back to something reasonable
    /// rather than refused: a listing is a thing to read, and there is no answer
    /// worth failing over here.
    fn window(&self) -> (i64, i64) {
        (
            self.limit.unwrap_or(PAGE).clamp(1, MOST),
            self.offset.max(0),
        )
    }
}

/// How the queue is to be drawn up.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Playing {
    /// Shuffled rather than as they sit on their records. Drawn before the limit
    /// below is applied, so a shuffled few hundred are a sample of everything
    /// rather than the first few hundred in a jumble.
    #[serde(default)]
    pub shuffle: bool,
    /// At most this many. Left out, every one of them.
    ///
    /// Whoever is asking decides how much music is enough — the panel stops at a
    /// sitting's worth when nothing has been narrowed down, and takes the lot
    /// when something has. That is a judgement about what somebody meant by
    /// pressing play, and it belongs to the screen that drew the button rather
    /// than to a contract other clients also read.
    pub limit: Option<i64>,
}

/// The tracks
///
/// In the order they sit on their records: by artist, then album, then disc and
/// track. A search reorders nothing — what is narrowed is which rows, not how
/// they are arranged — so a discography stays a discography.
#[utoipa::path(
    get,
    path = "/tracks",
    tag = "collection",
    params(Filter, Paging),
    responses(
        (status = 200, description = "A page of them, and how many in all", body = Tracks),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn tracks(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Query(filter): Query<Filter>,
    Query(paging): Query<Paging>,
) -> Result<Json<Tracks>, ApiError> {
    let (limit, offset) = paging.window();
    let who = panel.user.id;

    let total = count(&pool, who, &filter, Countable::Tracks).await?;

    let mut builder = QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(who);
    builder.push(concat!(
        visible_libraries_tail!(),
        "SELECT t.public_id, t.title,
                (SELECT group_concat(a.name, ', ')
                   FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
                  WHERE ta.track_id = t.id AND ta.role = 'main') AS artists,
                al.name AS album, al.public_id AS album_id,
                (SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                  WHERE tg.track_id = t.id ORDER BY g.name LIMIT 1) AS genre,
                t.duration_ms, t.missing_since IS NOT NULL AS missing
           FROM tracks t
           LEFT JOIN albums al ON al.id = t.album_id
           LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.role = 'main'
           LEFT JOIN artists ar ON ar.id = aa.artist_id
          WHERE t.library_id IN (SELECT id FROM visible_libraries)"
    ));
    narrow(&mut builder, &filter);
    // By artist, then record, then the order the songs are in on it.
    //
    // The sort is over three tables and no index covers it, so SQLite orders the
    // lot before it takes the page. Measured against twenty-four thousand tracks:
    // twelve milliseconds for the first page and fifty-four for the last, which
    // is only reached by somebody who has scrolled through four hundred of them.
    // Worth knowing if it ever wants improving — the answer would be a sort key
    // written by the scanner — and not worth carrying that key around before
    // then.
    builder.push(
        " ORDER BY ar.sort_name COLLATE NOCASE, ar.name COLLATE NOCASE,
                   al.year, al.name COLLATE NOCASE,
                   t.disc_number, t.track_number, t.title COLLATE NOCASE
            LIMIT ",
    );
    builder.push_bind(limit);
    builder.push(" OFFSET ");
    builder.push_bind(offset);

    let rows: Vec<TrackRow> = builder
        .build_query_as()
        .fetch_all(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "listing the tracks"))?;

    Ok(Json(Tracks {
        total,
        tracks: rows.into_iter().map(Track::from).collect(),
    }))
}

/// What to play
///
/// Every track the filter matches, as identifiers and nothing else, in the order
/// they would be listed or shuffled. This is what "play what you are looking at"
/// asks for: the whole of what was narrowed to, rather than the rows that have
/// been fetched so far.
///
#[utoipa::path(
    get,
    path = "/tracks/ids",
    tag = "collection",
    params(Filter, Playing),
    responses(
        (status = 200, description = "What to play, in order", body = Queue),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn queue(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Query(filter): Query<Filter>,
    Query(playing): Query<Playing>,
) -> Result<Json<Queue>, ApiError> {
    let mut builder = QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(panel.user.id);
    builder.push(concat!(
        visible_libraries_tail!(),
        "SELECT t.public_id
           FROM tracks t
           LEFT JOIN albums al ON al.id = t.album_id
           LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.role = 'main'
           LEFT JOIN artists ar ON ar.id = aa.artist_id
          WHERE t.library_id IN (SELECT id FROM visible_libraries)
            AND t.missing_since IS NULL"
    ));
    narrow(&mut builder, &filter);

    if playing.shuffle {
        builder.push(" ORDER BY random()");
    } else {
        builder.push(
            " ORDER BY ar.sort_name COLLATE NOCASE, ar.name COLLATE NOCASE,
                       al.year, al.name COLLATE NOCASE,
                       t.disc_number, t.track_number, t.title COLLATE NOCASE",
        );
    }

    // After the ordering, which is what makes a shuffled limit a sample of
    // everything rather than the first few hundred in a jumble.
    if let Some(most) = playing.limit.filter(|most| *most > 0) {
        builder.push(" LIMIT ");
        builder.push_bind(most);
    }

    let tracks: Vec<String> = builder
        .build_query_scalar()
        .fetch_all(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "working out what to play"))?;

    Ok(Json(Queue { tracks }))
}

/// The albums
#[utoipa::path(
    get,
    path = "/albums",
    tag = "collection",
    params(Filter, Paging),
    responses(
        (status = 200, description = "A page of them, and how many in all", body = Albums),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn albums(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Query(filter): Query<Filter>,
    Query(paging): Query<Paging>,
) -> Result<Json<Albums>, ApiError> {
    let (limit, offset) = paging.window();
    let who = panel.user.id;

    let total = count(&pool, who, &filter, Countable::Albums).await?;

    let mut builder = QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(who);
    builder.push(concat!(
        visible_libraries_tail!(),
        "SELECT al.public_id, al.name,
                (SELECT group_concat(a.name, ', ')
                   FROM album_artists aa JOIN artists a ON a.id = aa.artist_id
                  WHERE aa.album_id = al.id AND aa.role = 'main') AS artist,
                al.year,
                (SELECT count(*) FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS tracks,
                al.artwork_id IS NOT NULL AS cover
           FROM albums al
           LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.role = 'main'
           LEFT JOIN artists ar ON ar.id = aa.artist_id
          WHERE ",
        album_is_visible!("al.id")
    ));
    narrow_albums(&mut builder, &filter);
    builder.push(
        " ORDER BY ar.sort_name COLLATE NOCASE, ar.name COLLATE NOCASE,
                   al.year, al.name COLLATE NOCASE
            LIMIT ",
    );
    builder.push_bind(limit);
    builder.push(" OFFSET ");
    builder.push_bind(offset);

    let rows: Vec<AlbumRow> = builder
        .build_query_as()
        .fetch_all(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "listing the albums"))?;

    Ok(Json(Albums {
        total,
        albums: rows.into_iter().map(Album::from).collect(),
    }))
}

/// The artists
#[utoipa::path(
    get,
    path = "/artists",
    tag = "collection",
    params(Filter, Paging),
    responses(
        (status = 200, description = "A page of them, and how many in all", body = Artists),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn artists(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Query(filter): Query<Filter>,
    Query(paging): Query<Paging>,
) -> Result<Json<Artists>, ApiError> {
    let (limit, offset) = paging.window();
    let who = panel.user.id;

    let total = count(&pool, who, &filter, Countable::Artists).await?;

    let mut builder = QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(who);
    builder.push(concat!(
        visible_libraries_tail!(),
        "SELECT a.public_id, a.name, a.artwork_id IS NOT NULL AS image,
                (SELECT count(DISTINCT t.album_id) FROM tracks t
                   JOIN track_artists ta ON ta.track_id = t.id
                  WHERE ta.artist_id = a.id AND t.missing_since IS NULL
                    AND t.album_id IS NOT NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS albums,
                (SELECT count(*) FROM tracks t
                   JOIN track_artists ta ON ta.track_id = t.id
                  WHERE ta.artist_id = a.id AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS tracks
           FROM artists a
          WHERE ",
        artist_is_visible!("a.id")
    ));
    if let Some(matching) = searching(&filter) {
        builder.push(" AND a.id IN (SELECT f.rowid FROM artists_fts f WHERE artists_fts MATCH ");
        builder.push_bind(matching);
        builder.push(")");
    }
    builder.push(" ORDER BY a.sort_name COLLATE NOCASE, a.name COLLATE NOCASE LIMIT ");
    builder.push_bind(limit);
    builder.push(" OFFSET ");
    builder.push_bind(offset);

    let rows: Vec<ArtistRow> = builder
        .build_query_as()
        .fetch_all(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "listing the artists"))?;

    Ok(Json(Artists {
        total,
        artists: rows.into_iter().map(Artist::from).collect(),
    }))
}

/// The genres
#[utoipa::path(
    get,
    path = "/genres",
    tag = "collection",
    params(Filter, Paging),
    responses(
        (status = 200, description = "A page of them, and how many in all", body = Genres),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn genres(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Query(filter): Query<Filter>,
    Query(paging): Query<Paging>,
) -> Result<Json<Genres>, ApiError> {
    let (limit, offset) = paging.window();
    let who = panel.user.id;

    let total = count(&pool, who, &filter, Countable::Genres).await?;

    let mut builder = QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(who);
    builder.push(concat!(
        visible_libraries_tail!(),
        "SELECT g.name,
                (SELECT count(DISTINCT t.album_id) FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NULL
                    AND t.album_id IS NOT NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS albums,
                (SELECT count(*) FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS tracks
           FROM genres g
          WHERE ",
        has_a_visible_track!("JOIN track_genres tg ON tg.track_id = t.id WHERE tg.genre_id = g.id")
    ));
    // No index of its own: a few dozen rows compared with `like` cost less than
    // keeping a fourth full text table in step with them.
    if let Some(text) = plain(&filter) {
        builder.push(" AND g.name LIKE ");
        builder.push_bind(format!("%{text}%"));
    }
    builder.push(" ORDER BY g.name COLLATE NOCASE LIMIT ");
    builder.push_bind(limit);
    builder.push(" OFFSET ");
    builder.push_bind(offset);

    let rows: Vec<GenreRow> = builder
        .build_query_as()
        .fetch_all(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "listing the genres"))?;

    Ok(Json(Genres {
        total,
        genres: rows.into_iter().map(Genre::from).collect(),
    }))
}

/// Count a play
///
/// Writes down that this track was listened to, which is what keeps the play
/// counts on the Overview and the Profile — and the tally of what a purge would
/// cost — true of the panel as well as of everything else.
///
/// When to call it is the player's judgement and not this call's: the usual
/// convention is once a song is mostly over rather than when it starts, so that
/// skipping through a record does not count as having heard it.
///
/// Answers the same whether or not the track exists, since a play is not a
/// question and a client that has just heard something has nothing to do with a
/// refusal.
#[utoipa::path(
    post,
    path = "/tracks/{id}/played",
    tag = "collection",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 204, description = "Counted"),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn played(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    crate::plays::record_play(&pool, panel.user.id, &id, &crate::db::now())
        .await
        .map_err(|e| ApiError::internal(e, "counting a play"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Which listing is being counted.
enum Countable {
    Tracks,
    Albums,
    Artists,
    Genres,
}

/// How many the filter matches in all.
///
/// A second statement rather than a window function beside the rows: the rows are
/// a page and the count is over everything, and `count(*) OVER ()` would have
/// SQLite build the whole result to hand back fifty of it.
async fn count(
    pool: &SqlitePool,
    who: i64,
    filter: &Filter,
    what: Countable,
) -> Result<i64, ApiError> {
    let mut builder = QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(who);

    match what {
        Countable::Tracks => {
            builder.push(concat!(
                visible_libraries_tail!(),
                "SELECT count(*) FROM tracks t
                   LEFT JOIN albums al ON al.id = t.album_id
                  WHERE t.library_id IN (SELECT id FROM visible_libraries)"
            ));
            narrow(&mut builder, filter);
        }
        Countable::Albums => {
            builder.push(concat!(
                visible_libraries_tail!(),
                "SELECT count(*) FROM albums al WHERE ",
                album_is_visible!("al.id")
            ));
            narrow_albums(&mut builder, filter);
        }
        Countable::Artists => {
            builder.push(concat!(
                visible_libraries_tail!(),
                "SELECT count(*) FROM artists a WHERE ",
                artist_is_visible!("a.id")
            ));
            if let Some(matching) = searching(filter) {
                builder.push(
                    " AND a.id IN (SELECT f.rowid FROM artists_fts f WHERE artists_fts MATCH ",
                );
                builder.push_bind(matching);
                builder.push(")");
            }
        }
        Countable::Genres => {
            builder.push(concat!(
                visible_libraries_tail!(),
                "SELECT count(*) FROM genres g WHERE ",
                has_a_visible_track!(
                    "JOIN track_genres tg ON tg.track_id = t.id WHERE tg.genre_id = g.id"
                )
            ));
            if let Some(text) = plain(filter) {
                builder.push(" AND g.name LIKE ");
                builder.push_bind(format!("%{text}%"));
            }
        }
    }

    builder
        .build_query_scalar()
        .fetch_one(pool)
        .await
        .map_err(|e| ApiError::internal(e, "counting a listing"))
}

/// The conditions a track listing takes on, appended to a statement that has
/// already opened its `WHERE`.
fn narrow(builder: &mut QueryBuilder<Sqlite>, filter: &Filter) {
    if let Some(matching) = searching(filter) {
        builder.push(" AND t.id IN (SELECT f.rowid FROM tracks_fts f WHERE tracks_fts MATCH ");
        builder.push_bind(matching);
        builder.push(")");
    }

    if let Some(album) = &filter.album {
        builder.push(" AND al.public_id = ");
        builder.push_bind(album.clone());
    }

    // Credited on the track, or on the album it belongs to. Somebody asking for
    // an artist means the songs they would say are theirs, which includes the
    // ones on their records that credit only the band.
    if let Some(artist) = &filter.artist {
        builder.push(
            " AND (EXISTS (SELECT 1 FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
                            WHERE ta.track_id = t.id AND a.public_id = ",
        );
        builder.push_bind(artist.clone());
        builder.push(
            ") OR EXISTS (SELECT 1 FROM album_artists aa JOIN artists a ON a.id = aa.artist_id
                           WHERE aa.album_id = t.album_id AND a.public_id = ",
        );
        builder.push_bind(artist.clone());
        builder.push("))");
    }

    if let Some(genre) = &filter.genre {
        builder.push(
            " AND EXISTS (SELECT 1 FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                           WHERE tg.track_id = t.id AND g.name = ",
        );
        builder.push_bind(genre.clone());
        builder.push(")");
    }
}

/// The same, for a listing of albums. No album filter, since an album is not in
/// an album.
fn narrow_albums(builder: &mut QueryBuilder<Sqlite>, filter: &Filter) {
    if let Some(matching) = searching(filter) {
        builder.push(" AND al.id IN (SELECT f.rowid FROM albums_fts f WHERE albums_fts MATCH ");
        builder.push_bind(matching);
        builder.push(")");
    }

    if let Some(artist) = &filter.artist {
        builder.push(
            " AND EXISTS (SELECT 1 FROM album_artists aa JOIN artists a ON a.id = aa.artist_id
                           WHERE aa.album_id = al.id AND a.public_id = ",
        );
        builder.push_bind(artist.clone());
        builder.push(")");
    }

    if let Some(genre) = &filter.genre {
        builder.push(
            " AND EXISTS (SELECT 1 FROM tracks t
                            JOIN track_genres tg ON tg.track_id = t.id
                            JOIN genres g ON g.id = tg.genre_id
                           WHERE t.album_id = al.id AND g.name = ",
        );
        builder.push_bind(genre.clone());
        builder.push(")");
    }
}

/// What was typed, as something FTS5 will take, or nothing when it amounts to no
/// search at all.
fn searching(filter: &Filter) -> Option<String> {
    crate::search::wanted(filter.search.as_deref()?)
}

/// What was typed, for the one listing that compares it as text.
fn plain(filter: &Filter) -> Option<String> {
    let text = filter.search.as_deref()?.trim();

    // `like` treats these as wildcards, so a search for "rock%" would find every
    // rock there is rather than nothing.
    (!text.is_empty()).then(|| text.replace('\\', "\\\\").replace(['%', '_'], ""))
}

#[derive(sqlx::FromRow)]
struct TrackRow {
    public_id: String,
    title: String,
    artists: Option<String>,
    album: Option<String>,
    album_id: Option<String>,
    genre: Option<String>,
    duration_ms: Option<i64>,
    missing: bool,
}

impl From<TrackRow> for Track {
    fn from(row: TrackRow) -> Self {
        Self {
            id: row.public_id,
            title: row.title,
            artists: row.artists,
            album: row.album,
            album_id: row.album_id,
            genre: row.genre,
            // Milliseconds in the row and seconds in the answer, like every other
            // length this API reports.
            duration: row.duration_ms.map(|ms| ms / 1000),
            missing: row.missing,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AlbumRow {
    public_id: String,
    name: String,
    artist: Option<String>,
    year: Option<i64>,
    tracks: i64,
    cover: bool,
}

impl From<AlbumRow> for Album {
    fn from(row: AlbumRow) -> Self {
        Self {
            id: row.public_id,
            name: row.name,
            artist: row.artist,
            year: row.year,
            tracks: row.tracks,
            cover: row.cover,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ArtistRow {
    public_id: String,
    name: String,
    image: bool,
    albums: i64,
    tracks: i64,
}

impl From<ArtistRow> for Artist {
    fn from(row: ArtistRow) -> Self {
        Self {
            id: row.public_id,
            name: row.name,
            albums: row.albums,
            tracks: row.tracks,
            image: row.image,
        }
    }
}

#[derive(sqlx::FromRow)]
struct GenreRow {
    name: String,
    albums: i64,
    tracks: i64,
}

impl From<GenreRow> for Genre {
    fn from(row: GenreRow) -> Self {
        Self {
            name: row.name,
            albums: row.albums,
            tracks: row.tracks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::user::User;

    /// Two libraries, an artist with a record in each, and one track whose file
    /// has gone. Enough to tell every rule here apart from the others.
    async fn a_collection() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        for (id, name) in [(1, "kept"), (2, "walled")] {
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
                 VALUES (?, ?, ?, 'root', '', 1)",
            )
            .bind(id)
            .bind(format!("f{id}"))
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::query(
            "INSERT INTO artists (id, public_id, name, sort_name, created_at, updated_at)
             VALUES (1, 'ar1', 'Triana', 'Triana', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO artists_fts (rowid, name) VALUES (1, 'Triana')")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("INSERT INTO genres (id, name) VALUES (1, 'Flamenco')")
            .execute(&pool)
            .await
            .unwrap();

        // One album per library, so a restriction has something to hide.
        for (id, library, name) in [(1i64, 1i64, "El Patio"), (2, 2, "Hijos del Agobio")] {
            sqlx::query(
                "INSERT INTO albums (id, public_id, name, year, created_at, updated_at)
                 VALUES (?, ?, ?, 1975, ?, ?)",
            )
            .bind(id)
            .bind(format!("al{id}"))
            .bind(name)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO album_artists (album_id, artist_id, role) VALUES (?, 1, 'main')",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO albums_fts (rowid, name, artists) VALUES (?, ?, 'Triana')")
                .bind(id)
                .bind(name)
                .execute(&pool)
                .await
                .unwrap();

            // Two tracks each, and in the first library one of them is gone.
            for track in 0..2 {
                let track_id = id * 10 + track;
                let title = format!("{name} {track}");
                let missing = (library == 1 && track == 1).then(|| at.clone());

                sqlx::query(
                    "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                         file_size, file_modified_at, content_type, suffix, title,
                                         track_number, disc_number, duration_ms, missing_since,
                                         last_seen_scan, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, 1, ?, 'audio/flac', 'flac', ?, ?, 1, 180000, ?, 1, ?, ?)",
                )
                .bind(track_id)
                .bind(format!("t{track_id}"))
                .bind(library)
                .bind(library)
                .bind(id)
                .bind(format!("{track_id}.flac"))
                .bind(&at)
                .bind(&title)
                .bind(track)
                .bind(missing)
                .bind(&at)
                .bind(&at)
                .execute(&pool)
                .await
                .unwrap();

                sqlx::query(
                    "INSERT INTO track_artists (track_id, artist_id, role, position)
                     VALUES (?, 1, 'main', 0)",
                )
                .bind(track_id)
                .execute(&pool)
                .await
                .unwrap();
                sqlx::query("INSERT INTO track_genres (track_id, genre_id) VALUES (?, 1)")
                    .bind(track_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query(
                    "INSERT INTO tracks_fts (rowid, title, album, artists)
                     VALUES (?, ?, ?, 'Triana')",
                )
                .bind(track_id)
                .bind(&title)
                .bind(name)
                .execute(&pool)
                .await
                .unwrap();
            }
        }

        pool
    }

    /// An account, and whether it is walled off from the second library.
    async fn somebody(pool: &SqlitePool, restricted: bool) -> Panel {
        let at = db::now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&at)
        .bind(&at)
        .fetch_one(pool)
        .await
        .unwrap();

        if restricted {
            sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, 1)")
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
        }

        Panel {
            id: 1,
            user: User {
                id,
                username: "ana".to_string(),
                is_admin: false,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        }
    }

    /// The same session again, for a test that makes more than one call: a
    /// `Panel` is consumed by the handler that takes it.
    fn again(panel: &Panel) -> Panel {
        Panel {
            id: panel.id,
            user: panel.user.clone(),
            expires_at: panel.expires_at.clone(),
        }
    }

    fn nothing() -> Query<Filter> {
        Query(Filter::default())
    }

    fn all_of_it() -> Query<Paging> {
        Query(Paging {
            offset: 0,
            limit: Some(MOST),
        })
    }

    /// The rule every one of these listings answers to first. A restriction is
    /// not a filter somebody can drop: it decides what the listing is.
    #[tokio::test]
    async fn a_restricted_account_is_shown_only_its_own_libraries() {
        let pool = a_collection().await;
        let walled = somebody(&pool, true).await;
        let walled_again = again(&walled);

        let Json(tracks) = tracks(walled, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        assert_eq!(tracks.total, 2, "the two in the library it may see");
        assert!(
            tracks
                .tracks
                .iter()
                .all(|t| t.album.as_deref() == Some("El Patio")),
            "nothing from the other library"
        );

        let Json(albums) = albums(walled_again, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        assert_eq!(albums.total, 1);
        assert_eq!(albums.albums[0].name, "El Patio");
    }

    /// And the same account with no restriction sees the lot, so the test above
    /// is measuring the restriction rather than a mistake in the fixture.
    #[tokio::test]
    async fn an_unrestricted_account_is_shown_everything() {
        let pool = a_collection().await;

        let Json(listed) = tracks(
            somebody(&pool, false).await,
            State(pool.clone()),
            nothing(),
            all_of_it(),
        )
        .await
        .unwrap();

        assert_eq!(listed.total, 4);
    }

    /// A track whose file is gone stays in the listing and says so — the Overview
    /// counts it, so a listing that dropped it would disagree with the Overview —
    /// and never goes in the queue, because there is nothing to play.
    #[tokio::test]
    async fn what_is_missing_is_listed_and_not_played() {
        let pool = a_collection().await;
        let ana = somebody(&pool, false).await;
        let ana_again = again(&ana);

        let Json(listed) = tracks(ana, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        assert_eq!(listed.tracks.iter().filter(|t| t.missing).count(), 1);

        let Json(playing) = queue(
            ana_again,
            State(pool.clone()),
            nothing(),
            Query(Playing::default()),
        )
        .await
        .unwrap();

        assert_eq!(playing.tracks.len(), 3, "the fourth has no file");
    }

    /// The total has to be counted through the same filter as the page, or an
    /// endless list asks for a page that does not exist — or stops before the
    /// end.
    #[tokio::test]
    async fn the_total_counts_what_the_page_was_filtered_by() {
        let pool = a_collection().await;
        let searching = || {
            Query(Filter {
                search: Some("patio".to_string()),
                ..Filter::default()
            })
        };

        let Json(listed) = tracks(
            somebody(&pool, false).await,
            State(pool.clone()),
            searching(),
            Query(Paging {
                offset: 0,
                limit: Some(1),
            }),
        )
        .await
        .unwrap();

        assert_eq!(listed.tracks.len(), 1, "one page");
        assert_eq!(listed.total, 2, "of two");
    }

    /// Typed into a search box, "tri" is somebody halfway through "Triana"
    /// rather than somebody looking for a word that is only "tri".
    #[tokio::test]
    async fn a_half_typed_word_finds_what_it_starts() {
        let pool = a_collection().await;

        let Json(found) = artists(
            somebody(&pool, false).await,
            State(pool.clone()),
            Query(Filter {
                search: Some("tri".to_string()),
                ..Filter::default()
            }),
            all_of_it(),
        )
        .await
        .unwrap();

        assert_eq!(found.total, 1);
        assert_eq!(found.artists[0].name, "Triana");
        assert_eq!(found.artists[0].albums, 2);
    }

    /// Asking for an artist means the songs somebody would say are theirs, which
    /// includes the ones on their records that credit only the band.
    #[tokio::test]
    async fn an_artist_filter_reaches_their_records() {
        let pool = a_collection().await;

        // A track on Triana's album crediting nobody in particular.
        let at = db::now();
        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path, file_size,
                                 file_modified_at, content_type, suffix, title, track_number,
                                 disc_number, duration_ms, last_seen_scan, created_at, updated_at)
             VALUES (99, 't99', 1, 1, 1, '99.flac', 1, ?, 'audio/flac', 'flac', 'Instrumental',
                     9, 1, 60000, 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let Json(theirs) = queue(
            somebody(&pool, false).await,
            State(pool.clone()),
            Query(Filter {
                artist: Some("ar1".to_string()),
                ..Filter::default()
            }),
            Query(Playing::default()),
        )
        .await
        .unwrap();

        assert!(
            theirs.tracks.iter().any(|id| id == "t99"),
            "credited to the album, so it is theirs"
        );
    }

    /// A listing is for reading. Asking for the whole collection in one page is
    /// what the queue is for, and it comes back in identifiers.
    #[tokio::test]
    async fn a_page_is_never_larger_than_a_page() {
        let asked = Paging {
            offset: -5,
            limit: Some(100_000),
        };

        assert_eq!(asked.window(), (MOST, 0));
    }

    /// A search of nothing but punctuation is not a search.
    #[tokio::test]
    async fn a_search_that_says_nothing_narrows_nothing() {
        let pool = a_collection().await;
        let empty = || {
            Query(Filter {
                search: Some("   ".to_string()),
                ..Filter::default()
            })
        };

        let Json(listed) = tracks(
            somebody(&pool, false).await,
            State(pool.clone()),
            empty(),
            all_of_it(),
        )
        .await
        .unwrap();

        assert_eq!(listed.total, 4, "all of them");
    }

    /// How many is enough is the caller's to say, and asking for none of it is
    /// not a way of asking for all of it.
    #[tokio::test]
    async fn a_queue_comes_back_as_long_as_it_was_asked_for() {
        let pool = a_collection().await;
        let ana = somebody(&pool, false).await;
        let ana_again = again(&ana);

        let Json(some) = queue(
            ana,
            State(pool.clone()),
            nothing(),
            Query(Playing {
                shuffle: false,
                limit: Some(2),
            }),
        )
        .await
        .unwrap();

        assert_eq!(some.tracks.len(), 2);

        let Json(all) = queue(
            ana_again,
            State(pool.clone()),
            nothing(),
            Query(Playing::default()),
        )
        .await
        .unwrap();

        assert_eq!(all.tracks.len(), 3, "everything with a file");
    }

    /// The counts on the Overview, on a Profile and in what a purge would cost
    /// all read the same rows, so a play from the panel has to land in them the
    /// same way a play from a phone does.
    #[tokio::test]
    async fn a_play_is_counted_for_the_track_and_its_album() {
        let pool = a_collection().await;
        let ana = somebody(&pool, false).await;
        let who = ana.user.id;

        played(ana, State(pool.clone()), UrlPath("t10".to_string()))
            .await
            .unwrap();

        let track: (i64,) = sqlx::query_as(
            "SELECT play_count FROM user_track_stats WHERE user_id = ? AND track_id = 10",
        )
        .bind(who)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(track.0, 1);

        let album: (i64,) = sqlx::query_as(
            "SELECT play_count FROM user_album_stats WHERE user_id = ? AND album_id = 1",
        )
        .bind(who)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(album.0, 1, "the record it is on, so albums can be ranked");
    }

    /// A track in a library somebody may not see is not theirs to have heard.
    /// Nothing is refused — a play is not a question — but nothing is written
    /// either, so their own figures stay figures they can account for.
    #[tokio::test]
    async fn a_play_of_what_you_cannot_see_is_not_counted() {
        let pool = a_collection().await;
        let walled = somebody(&pool, true).await;
        let who = walled.user.id;

        // t20 is in the second library, which this account is walled off from.
        played(walled, State(pool.clone()), UrlPath("t20".to_string()))
            .await
            .unwrap();

        let counted: Option<(i64,)> =
            sqlx::query_as("SELECT play_count FROM user_track_stats WHERE user_id = ?")
                .bind(who)
                .fetch_optional(&pool)
                .await
                .unwrap();

        assert!(counted.is_none());
    }
}
