// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Browsing the catalogue by tags: artists, their albums, their songs.
//!
//! Every query here filters out what the scanner marked as absent. A track
//! whose file is gone keeps its row so the user's data survives, but it has no
//! business appearing in a listing.

use super::auth::Authenticated;
use super::error::ApiError;
use super::models::{
    AlbumId3, ArtistId3, Child, DiscTitle, ItemGenre, NamedEntry, ReplayGain, seconds,
};
use super::response;
use crate::settings;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use tracing::error;

#[derive(Debug, Deserialize)]
pub(super) struct IdQuery {
    pub id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MusicFoldersBody {
    music_folders: MusicFolders,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MusicFolders {
    music_folder: Vec<MusicFolder>,
}

#[derive(Serialize)]
struct MusicFolder {
    id: i64,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistsBody {
    artists: ArtistsIndex,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistsIndex {
    ignored_articles: String,
    index: Vec<IndexGroup>,
}

#[derive(Serialize)]
struct IndexGroup {
    name: String,
    artist: Vec<ArtistId3>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistBody {
    artist: ArtistWithAlbums,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistWithAlbums {
    #[serde(flatten)]
    artist: ArtistId3,
    album: Vec<AlbumId3>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumBody {
    album: AlbumWithSongs,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumWithSongs {
    #[serde(flatten)]
    album: AlbumId3,
    song: Vec<Child>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SongBody {
    song: Child,
}

pub async fn get_music_folders(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let rows: Result<Vec<(i64, String)>, _> = sqlx::query_as(concat!(
        visible_libraries!(),
        " SELECT id, name FROM libraries
           WHERE id IN (SELECT id FROM visible_libraries)
           ORDER BY name"
    ))
    .bind(auth.user.id)
    .fetch_all(&pool)
    .await;

    let folders = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|(id, name)| MusicFolder { id, name })
            .collect(),
        Err(e) => return internal(e, auth.format, "listing music folders"),
    };

    response::ok(
        auth.format,
        MusicFoldersBody {
            music_folders: MusicFolders {
                music_folder: folders,
            },
        },
    )
}

pub async fn get_artists(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let artists = match load_artists(&pool, auth.user.id).await {
        Ok(artists) => artists,
        Err(e) => return internal(e, auth.format, "listing artists"),
    };

    let articles = match settings::load(&pool).await {
        Ok(settings) => settings.ignored_articles,
        Err(e) => {
            error!("reading the settings: {e:#}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };
    let articles = articles.as_slice();

    let groups = by_letter(artists, articles, |artist| {
        artist.sort_name.as_deref().unwrap_or(&artist.name)
    })
    .into_iter()
    .map(|(name, artist)| IndexGroup { name, artist })
    .collect();

    response::ok(
        auth.format,
        ArtistsBody {
            artists: ArtistsIndex {
                ignored_articles: articles.join(" "),
                index: groups,
            },
        },
    )
}

pub async fn get_artist(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
) -> Response {
    let artist = match load_artist(&pool, auth.user.id, &query.id).await {
        Ok(Some(artist)) => artist,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => return internal(e, auth.format, "loading an artist"),
    };

    let albums = match load_albums_of_artist(&pool, auth.user.id, &query.id).await {
        Ok(albums) => albums,
        Err(e) => return internal(e, auth.format, "loading the albums of an artist"),
    };

    response::ok(
        auth.format,
        ArtistBody {
            artist: ArtistWithAlbums {
                artist,
                album: albums,
            },
        },
    )
}

pub async fn get_album(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
) -> Response {
    let album = match load_album(&pool, auth.user.id, &query.id).await {
        Ok(Some(album)) => album,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => return internal(e, auth.format, "loading an album"),
    };

    let songs = match load_songs_of_album(&pool, auth.user.id, &query.id).await {
        Ok(songs) => songs,
        Err(e) => return internal(e, auth.format, "loading the songs of an album"),
    };

    response::ok(
        auth.format,
        AlbumBody {
            album: AlbumWithSongs { album, song: songs },
        },
    )
}

pub async fn get_song(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
) -> Response {
    match load_song(&pool, auth.user.id, &query.id).await {
        Ok(Some(song)) => response::ok(auth.format, SongBody { song }),
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "loading a song"),
    }
}

/// A database failure is ours, not the client's: log the detail and return the
/// generic error, without leaking SQL to whoever asked.
fn internal(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}

/// A name with its leading article dropped, so "The Beatles" files under B.
///
/// Compares whole words rather than slicing off a prefix. Slicing a string by
/// byte length panics when the cut lands inside a character, and "Björk" against
/// the article "La " does exactly that. Splitting on the space is both safe and
/// more correct: "Theatre of Tragedy" keeps its T because its first word is not
/// an article.
fn without_article<'a>(name: &'a str, articles: &[String]) -> &'a str {
    let name = name.trim();

    match name.split_once(' ') {
        Some((first, rest)) if articles.iter().any(|a| a.eq_ignore_ascii_case(first)) => {
            rest.trim_start()
        }
        _ => name,
    }
}

/// First letter for the alphabetical index.
fn index_letter(name: &str, articles: &[String]) -> String {
    match without_article(name, articles).chars().next() {
        Some(first) if first.is_alphabetic() => first.to_uppercase().to_string(),
        // Digits, punctuation and everything else share one bucket, which is
        // what clients expect to see at the top of the list.
        _ => "#".to_string(),
    }
}

/// Groups a list into the runs of one letter that an index is made of.
///
/// The sort happens here, on the same name the letter comes from, and that is
/// the point of the function. The database orders by the whole name, article
/// and all, so "The Beatles" arrives among the Ts while belonging under B —
/// and since a group is a run of equal letters, it would open a second B group
/// further down. Two of the same letter in one index, and the artist filed
/// where nobody would look.
///
/// Sorting is stable, so whatever order the database sent still decides ties.
fn by_letter<T, F>(mut items: Vec<T>, articles: &[String], name_of: F) -> Vec<(String, Vec<T>)>
where
    F: Fn(&T) -> &str,
{
    items.sort_by_cached_key(|item| without_article(name_of(item), articles).to_lowercase());

    let mut groups: Vec<(String, Vec<T>)> = Vec::new();

    for item in items {
        let letter = index_letter(name_of(&item), articles);

        match groups.last_mut() {
            Some((name, run)) if *name == letter => run.push(item),
            _ => groups.push((letter, vec![item])),
        }
    }

    groups
}

#[derive(sqlx::FromRow)]
struct ArtistRow {
    id: i64,
    public_id: String,
    name: String,
    sort_name: Option<String>,
    mbid: Option<String>,
    album_count: i64,
    starred_at: Option<String>,
}

impl From<ArtistRow> for ArtistId3 {
    fn from(row: ArtistRow) -> Self {
        Self {
            id: row.public_id,
            name: row.name,
            cover_art: None,
            album_count: Some(row.album_count),
            starred: row.starred_at,
            music_brainz_id: row.mbid,
            sort_name: row.sort_name,
        }
    }
}

/// Artists worth listing, with how many albums they have.
///
/// An artist counts as present two ways: crediting a track that is still there,
/// or being the album artist of an album that still has tracks. Only the first
/// would drop "Various Artists" off the list entirely, and would report zero
/// albums for everyone who only ever appears on a compilation — while getArtist
/// happily returned those same albums. The three endpoints have to agree, so
/// they all use this pair of conditions.
///
/// A macro rather than a constant so `concat!` can build each full statement at
/// compile time: sqlx will not take SQL assembled at runtime, and it is right
/// not to.
macro_rules! artist_columns_head {
    () => {
        concat!(
            "
    SELECT a.id, a.public_id, a.name, a.sort_name, a.mbid,
           (SELECT count(*) FROM albums al
             WHERE (EXISTS (
                        SELECT 1 FROM album_artists aa
                         WHERE aa.album_id = al.id AND aa.artist_id = a.id
                    )
                 OR EXISTS (
                        SELECT 1 FROM track_artists ta
                          JOIN tracks t ON t.id = ta.track_id
                         WHERE t.album_id = al.id AND ta.artist_id = a.id
                    ))
               AND ",
            album_is_visible!("al.id"),
            ") AS album_count,
           s.starred_at
      FROM artists a
      LEFT JOIN user_artist_stats s ON s.artist_id = a.id AND s.user_id = "
        )
    };
}

macro_rules! artist_columns_tail {
    () => {
        "
     WHERE (EXISTS (
                SELECT 1 FROM track_artists ta
                  JOIN tracks t ON t.id = ta.track_id
                 WHERE ta.artist_id = a.id AND t.missing_since IS NULL
                   AND t.library_id IN (SELECT id FROM visible_libraries)
            )
         OR EXISTS (
                SELECT 1 FROM album_artists aa
                  JOIN tracks t ON t.album_id = aa.album_id
                 WHERE aa.artist_id = a.id AND t.missing_since IS NULL
                   AND t.library_id IN (SELECT id FROM visible_libraries)
            ))"
    };
}

/// The whole statement, for the callers that bind the user themselves.
macro_rules! artist_columns {
    () => {
        concat!(
            visible_libraries!(),
            artist_columns_head!(),
            "?",
            artist_columns_tail!()
        )
    };
}

async fn load_artists(pool: &SqlitePool, user_id: i64) -> Result<Vec<ArtistId3>, sqlx::Error> {
    let rows: Vec<ArtistRow> = sqlx::query_as(concat!(
        artist_columns!(),
        " ORDER BY coalesce(a.sort_name, a.name) COLLATE NOCASE"
    ))
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(ArtistId3::from).collect())
}

async fn load_artist(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
) -> Result<Option<ArtistId3>, sqlx::Error> {
    let row: Option<ArtistRow> = sqlx::query_as(concat!(artist_columns!(), " AND a.public_id = ?"))
        .bind(user_id)
        .bind(user_id)
        .bind(public_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(ArtistId3::from))
}

#[derive(sqlx::FromRow)]
struct AlbumRow {
    id: i64,
    public_id: String,
    name: String,
    sort_name: Option<String>,
    year: Option<i64>,
    is_compilation: bool,
    mbid_release: Option<String>,
    created_at: String,
    song_count: i64,
    duration_ms: Option<i64>,
    play_count: Option<i64>,
    starred_at: Option<String>,
    rating: Option<i64>,
}

/// Albums with their aggregates. The song count and duration come from the
/// tracks still present, so an album whose files are gone reports zero rather
/// than lying about what can be played.
macro_rules! album_columns_head {
    () => {
        concat!(
            "
    SELECT al.id, al.public_id, al.name, al.sort_name, al.year, al.is_compilation,
           al.mbid_release, al.created_at,
           (SELECT count(*) FROM tracks t
             WHERE t.album_id = al.id AND t.missing_since IS NULL
               AND t.library_id IN (SELECT id FROM visible_libraries)) AS song_count,
           (SELECT sum(t.duration_ms) FROM tracks t
             WHERE t.album_id = al.id AND t.missing_since IS NULL
               AND t.library_id IN (SELECT id FROM visible_libraries)) AS duration_ms,
           s.play_count, s.starred_at, s.rating
      FROM albums al
      LEFT JOIN user_album_stats s ON s.album_id = al.id AND s.user_id = "
        )
    };
}

macro_rules! album_columns {
    () => {
        concat!(visible_libraries!(), album_columns_head!(), "?")
    };
}

async fn load_albums_of_artist(
    pool: &SqlitePool,
    user_id: i64,
    artist_public_id: &str,
) -> Result<Vec<AlbumId3>, sqlx::Error> {
    // Credited as album artist, or simply holding tracks by them: an album with
    // no album artist tagged would otherwise be invisible from its artist.
    let rows: Vec<AlbumRow> = sqlx::query_as(concat!(
        album_columns!(),
        "
         WHERE EXISTS (
                   SELECT 1 FROM album_artists aa
                     JOIN artists ar ON ar.id = aa.artist_id
                    WHERE aa.album_id = al.id AND ar.public_id = ?
               )
            OR EXISTS (
                   SELECT 1 FROM tracks t
                     JOIN track_artists ta ON ta.track_id = t.id
                     JOIN artists ar ON ar.id = ta.artist_id
                    WHERE t.album_id = al.id AND t.missing_since IS NULL
                      AND t.library_id IN (SELECT id FROM visible_libraries)
                      AND ar.public_id = ?
               )
         ORDER BY al.year, coalesce(al.sort_name, al.name) COLLATE NOCASE"
    ))
    .bind(user_id)
    .bind(user_id)
    .bind(artist_public_id)
    .bind(artist_public_id)
    .fetch_all(pool)
    .await?;

    let mut albums = Vec::with_capacity(rows.len());
    for row in rows {
        albums.push(build_album(pool, row).await?);
    }

    Ok(albums)
}

async fn load_album(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
) -> Result<Option<AlbumId3>, sqlx::Error> {
    let row: Option<AlbumRow> =
        // The visibility condition belongs here rather than only in the counts:
        // without it, naming an album in a library you may not see answers with
        // its title and an empty track list instead of saying it is not there.
        sqlx::query_as(concat!(
            album_columns!(),
            " WHERE al.public_id = ? AND ",
            album_is_visible!("al.id")
        ))
            .bind(user_id)
            .bind(user_id)
            .bind(public_id)
            .fetch_optional(pool)
            .await?;

    match row {
        Some(row) => Ok(Some(build_album(pool, row).await?)),
        None => Ok(None),
    }
}

/// Fills in what needs its own queries: the credited artists, the genres and
/// the disc titles.
async fn build_album(pool: &SqlitePool, row: AlbumRow) -> Result<AlbumId3, sqlx::Error> {
    let artists: Vec<(String, String)> = sqlx::query_as(
        "SELECT ar.public_id, ar.name
           FROM album_artists aa
           JOIN artists ar ON ar.id = aa.artist_id
           JOIN albums al ON al.id = aa.album_id
          WHERE al.public_id = ? AND aa.role = 'albumartist'
          ORDER BY aa.position",
    )
    .bind(&row.public_id)
    .fetch_all(pool)
    .await?;

    let genres: Vec<String> = sqlx::query_scalar(
        "SELECT g.name
           FROM album_genres ag
           JOIN genres g ON g.id = ag.genre_id
           JOIN albums al ON al.id = ag.album_id
          WHERE al.public_id = ?
          ORDER BY g.name",
    )
    .bind(&row.public_id)
    .fetch_all(pool)
    .await?;

    let disc_titles: Vec<(i64, String)> = sqlx::query_as(
        "SELECT ad.disc_number, ad.title
           FROM album_discs ad
           JOIN albums al ON al.id = ad.album_id
          WHERE al.public_id = ? AND ad.title IS NOT NULL
          ORDER BY ad.disc_number",
    )
    .bind(&row.public_id)
    .fetch_all(pool)
    .await?;

    let display_artist = display_names(&artists);
    let first_artist = artists.first().cloned();

    Ok(AlbumId3 {
        id: row.public_id.clone(),
        name: row.name,
        song_count: row.song_count,
        duration: seconds(row.duration_ms).unwrap_or(0),
        created: row.created_at,
        artist: display_artist.clone(),
        artist_id: first_artist.map(|(id, _)| id),
        // The album's own id doubles as its cover art id, which is how the API
        // has always worked and what lets the cover be found without having
        // extracted anything yet.
        cover_art: Some(row.public_id.clone()),
        play_count: row.play_count,
        starred: row.starred_at,
        year: row.year,
        genre: genres.first().cloned(),
        user_rating: row.rating,
        music_brainz_id: row.mbid_release,
        sort_name: row.sort_name,
        is_compilation: Some(row.is_compilation),
        genres: genres.into_iter().map(|name| ItemGenre { name }).collect(),
        artists: artists
            .into_iter()
            .map(|(id, name)| ArtistId3::named(id, name))
            .collect(),
        display_artist,
        disc_titles: disc_titles
            .into_iter()
            .map(|(disc, title)| DiscTitle { disc, title })
            .collect(),
    })
}

#[derive(sqlx::FromRow)]
struct TrackRow {
    id: i64,
    public_id: String,
    title: String,
    sort_title: Option<String>,
    track_number: Option<i64>,
    disc_number: Option<i64>,
    year: Option<i64>,
    duration_ms: Option<i64>,
    bit_rate: Option<i64>,
    bit_depth: Option<i64>,
    sampling_rate: Option<i64>,
    channel_count: Option<i64>,
    bpm: Option<i64>,
    comment: Option<String>,
    mbid_recording: Option<String>,
    isrc: Option<String>,
    rg_track_gain: Option<f64>,
    rg_track_peak: Option<f64>,
    file_size: i64,
    content_type: String,
    suffix: String,
    created_at: String,
    album_public_id: Option<String>,
    album_name: Option<String>,
    rg_album_gain: Option<f64>,
    rg_album_peak: Option<f64>,
    folder_public_id: String,
    play_count: Option<i64>,
    last_played: Option<String>,
    starred_at: Option<String>,
    rating: Option<i64>,
}

macro_rules! track_columns_head {
    () => {
        concat!(
            "
    SELECT t.id, t.public_id, t.title, t.sort_title, t.track_number, t.disc_number,
           t.year, t.duration_ms, t.bit_rate, t.bit_depth, t.sampling_rate,
           t.channel_count, t.bpm, t.comment, t.mbid_recording, t.isrc,
           t.rg_track_gain, t.rg_track_peak, t.file_size, t.content_type,
           t.suffix, t.created_at,
           al.public_id AS album_public_id, al.name AS album_name,
           al.rg_album_gain, al.rg_album_peak,
           f.public_id AS folder_public_id,
           s.play_count, s.last_played, s.starred_at, s.rating
      FROM tracks t
      JOIN folders f ON f.id = t.folder_id
      LEFT JOIN albums al ON al.id = t.album_id
      LEFT JOIN user_track_stats s ON s.track_id = t.id AND s.user_id = "
        )
    };
}

macro_rules! track_columns_tail {
    () => {
        "
     WHERE t.missing_since IS NULL
       AND t.library_id IN (SELECT id FROM visible_libraries)"
    };
}

macro_rules! track_columns {
    () => {
        concat!(
            visible_libraries!(),
            track_columns_head!(),
            "?",
            track_columns_tail!()
        )
    };
}

async fn load_songs_of_album(
    pool: &SqlitePool,
    user_id: i64,
    album_public_id: &str,
) -> Result<Vec<Child>, sqlx::Error> {
    let rows: Vec<TrackRow> = sqlx::query_as(concat!(
        track_columns!(),
        " AND al.public_id = ?
         ORDER BY t.disc_number, t.track_number, t.title COLLATE NOCASE"
    ))
    .bind(user_id)
    .bind(user_id)
    .bind(album_public_id)
    .fetch_all(pool)
    .await?;

    build_children(pool, rows).await
}

async fn load_song(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
) -> Result<Option<Child>, sqlx::Error> {
    let row: Option<TrackRow> = sqlx::query_as(concat!(track_columns!(), " AND t.public_id = ?"))
        .bind(user_id)
        .bind(user_id)
        .bind(public_id)
        .fetch_optional(pool)
        .await?;

    match row {
        Some(row) => Ok(build_children(pool, vec![row]).await?.pop()),
        None => Ok(None),
    }
}

/// Attaches artists and genres to a batch of tracks.
///
/// Two queries for the whole batch rather than two per track: the credits of a
/// fifty track album are one round trip, not a hundred.
async fn build_children(pool: &SqlitePool, rows: Vec<TrackRow>) -> Result<Vec<Child>, sqlx::Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<&str> = rows.iter().map(|row| row.public_id.as_str()).collect();
    let mut artists = artists_by_track(pool, &ids).await?;
    let mut genres = genres_by_track(pool, &ids).await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let track_artists = artists.remove(&row.public_id).unwrap_or_default();
            let track_genres = genres.remove(&row.public_id).unwrap_or_default();
            build_child(row, track_artists, track_genres)
        })
        .collect())
}

async fn artists_by_track(
    pool: &SqlitePool,
    track_ids: &[&str],
) -> Result<HashMap<String, Vec<(String, String)>>, sqlx::Error> {
    let mut builder = sqlx::QueryBuilder::new(
        "SELECT t.public_id, ar.public_id, ar.name
           FROM track_artists ta
           JOIN tracks t ON t.id = ta.track_id
           JOIN artists ar ON ar.id = ta.artist_id
          WHERE ta.role = 'artist' AND t.public_id IN (",
    );
    let mut separated = builder.separated(", ");
    for id in track_ids {
        separated.push_bind(*id);
    }
    builder.push(") ORDER BY t.public_id, ta.position");

    let rows: Vec<(String, String, String)> = builder.build_query_as().fetch_all(pool).await?;

    let mut grouped: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (track, artist_id, name) in rows {
        grouped.entry(track).or_default().push((artist_id, name));
    }

    Ok(grouped)
}

async fn genres_by_track(
    pool: &SqlitePool,
    track_ids: &[&str],
) -> Result<HashMap<String, Vec<String>>, sqlx::Error> {
    let mut builder = sqlx::QueryBuilder::new(
        "SELECT t.public_id, g.name
           FROM track_genres tg
           JOIN tracks t ON t.id = tg.track_id
           JOIN genres g ON g.id = tg.genre_id
          WHERE t.public_id IN (",
    );
    let mut separated = builder.separated(", ");
    for id in track_ids {
        separated.push_bind(*id);
    }
    builder.push(") ORDER BY t.public_id, g.name");

    let rows: Vec<(String, String)> = builder.build_query_as().fetch_all(pool).await?;

    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    for (track, genre) in rows {
        grouped.entry(track).or_default().push(genre);
    }

    Ok(grouped)
}

fn build_child(row: TrackRow, artists: Vec<(String, String)>, genres: Vec<String>) -> Child {
    let display_artist = display_names(&artists);
    let first_artist = artists.first().cloned();

    Child {
        id: row.public_id,
        is_dir: false,
        title: row.title,
        parent: Some(row.folder_public_id),
        album: row.album_name,
        artist: display_artist.clone(),
        track: row.track_number,
        year: row.year,
        genre: genres.first().cloned(),
        // A song's cover is its album's, and the album's id is the handle for it.
        cover_art: row.album_public_id.clone(),
        size: Some(row.file_size),
        content_type: Some(row.content_type),
        suffix: Some(row.suffix),
        duration: seconds(row.duration_ms),
        bit_rate: row.bit_rate,
        bit_depth: row.bit_depth,
        sampling_rate: row.sampling_rate,
        channel_count: row.channel_count,
        disc_number: row.disc_number,
        created: Some(row.created_at),
        starred: row.starred_at,
        user_rating: row.rating,
        play_count: row.play_count,
        played: row.last_played,
        album_id: row.album_public_id,
        artist_id: first_artist.map(|(id, _)| id),
        is_video: false,
        r#type: "music",
        media_type: "song",
        bpm: row.bpm,
        comment: row.comment,
        sort_name: row.sort_title,
        music_brainz_id: row.mbid_recording,
        isrc: row.isrc.into_iter().collect(),
        genres: genres.into_iter().map(|name| ItemGenre { name }).collect(),
        artists: artists
            .into_iter()
            .map(|(id, name)| ArtistId3::named(id, name))
            .collect(),
        display_artist,
        album_artists: Vec::new(),
        display_album_artist: None,
        replay_gain: ReplayGain::of(
            row.rg_track_gain,
            row.rg_track_peak,
            row.rg_album_gain,
            row.rg_album_peak,
        ),
    }
}

/// The flat string the older fields want, built from the structured list so the
/// two never disagree.
fn display_names(artists: &[(String, String)]) -> Option<String> {
    if artists.is_empty() {
        return None;
    }

    Some(
        artists
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn articles() -> Vec<String> {
        ["The", "El", "La", "Los", "Las", "Le", "Les"]
            .iter()
            .map(|a| a.to_string())
            .collect()
    }

    #[test]
    fn a_leading_article_does_not_decide_the_letter() {
        let a = articles();
        assert_eq!(index_letter("The Beatles", &a), "B");
        assert_eq!(index_letter("the beatles", &a), "B");
        assert_eq!(index_letter("Los Planetas", &a), "P");
        assert_eq!(index_letter("La Oreja de Van Gogh", &a), "O");
    }

    /// Names in the order the database sends them, which is by the whole name,
    /// article included.
    fn as_the_database_sends_them() -> Vec<String> {
        ["Beach Boys", "Bee Gees", "Cure", "The Beatles"]
            .iter()
            .map(|n| n.to_string())
            .collect()
    }

    /// The failure this guards against: grouping drops the article but the
    /// database's order does not, so "The Beatles" arrives after "Cure" and
    /// starts a second B group of its own.
    #[test]
    fn a_letter_never_opens_twice() {
        let groups = by_letter(as_the_database_sends_them(), &articles(), |n| n.as_str());
        let letters: Vec<&str> = groups.iter().map(|(name, _)| name.as_str()).collect();

        assert_eq!(letters, ["B", "C"]);
    }

    /// And it lands in its place within the group, not merely inside it: an
    /// artist filed under B is looked for where its name without the article
    /// says, between "Beach Boys" and "Bee Gees".
    #[test]
    fn the_article_is_gone_from_the_order_too() {
        let groups = by_letter(as_the_database_sends_them(), &articles(), |n| n.as_str());

        assert_eq!(groups[0].1, ["Beach Boys", "The Beatles", "Bee Gees"]);
    }

    #[test]
    fn an_article_that_is_the_whole_name_stays_put() {
        let a = articles();
        assert_eq!(index_letter("The", &a), "T");
        assert_eq!(index_letter("La", &a), "L");
    }

    #[test]
    fn a_word_that_merely_starts_like_an_article_is_untouched() {
        let a = articles();
        assert_eq!(index_letter("Theatre of Tragedy", &a), "T");
        assert_eq!(index_letter("Element of Crime", &a), "E");
    }

    #[test]
    fn everything_that_is_not_a_letter_shares_a_bucket() {
        let a = articles();
        assert_eq!(index_letter("2Pac", &a), "#");
        assert_eq!(index_letter("!!!", &a), "#");
        assert_eq!(index_letter("", &a), "#");
    }

    #[test]
    fn accented_letters_keep_their_own_group() {
        let a = articles();
        assert_eq!(index_letter("Ábaco", &a), "Á");
        assert_eq!(index_letter("Ñu", &a), "Ñ");
    }

    /// Slicing by byte length used to panic here: the third byte of "Björk"
    /// falls inside the 'ö', which is where the article "La " would have cut.
    #[test]
    fn a_multibyte_character_at_the_cut_does_not_panic() {
        let a = articles();
        assert_eq!(index_letter("Björk", &a), "B");
        assert_eq!(index_letter("Sigur Rós", &a), "S");
        assert_eq!(index_letter("Émile", &a), "É");
        assert_eq!(index_letter("日本", &a), "日");
        // And with an article actually present in front of one.
        assert_eq!(index_letter("La Björk", &a), "B");
    }

    #[test]
    fn a_display_name_joins_the_structured_list() {
        let artists = vec![
            ("1".to_string(), "David Bowie".to_string()),
            ("2".to_string(), "Queen".to_string()),
        ];
        assert_eq!(
            display_names(&artists).as_deref(),
            Some("David Bowie, Queen")
        );
        assert_eq!(display_names(&[]), None);
    }
}

// ---------------------------------------------------------------------------
// Browsing by folder
// ---------------------------------------------------------------------------
//
// The other tree. Clients that browse by directory need this, and so does
// anybody whose library is well organised but poorly tagged.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexesQuery {
    music_folder_id: Option<i64>,
    /// Milliseconds since the epoch. A client that already has the tree asks
    /// for it again with this set, and gets an empty answer if nothing moved.
    if_modified_since: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexesBody {
    indexes: Indexes,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Indexes {
    last_modified: i64,
    ignored_articles: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    index: Vec<FolderIndexGroup>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    child: Vec<Child>,
}

#[derive(Serialize)]
struct FolderIndexGroup {
    name: String,
    artist: Vec<NamedEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryBody {
    directory: Directory,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Directory {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    child: Vec<Child>,
}

pub async fn get_indexes(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IndexesQuery>,
) -> Response {
    let last_modified = match load_last_modified(&pool, auth.user.id, query.music_folder_id).await {
        Ok(value) => value,
        Err(e) => return internal(e, auth.format, "reading when the tree last changed"),
    };

    let articles = match settings::load(&pool).await {
        Ok(settings) => settings.ignored_articles,
        Err(e) => {
            error!("reading the settings: {e:#}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };
    let articles = articles.as_slice();

    // Nothing has moved since the client last asked, so say so with an empty
    // body rather than sending the whole tree again.
    if query
        .if_modified_since
        .is_some_and(|since| last_modified <= since)
    {
        return response::ok(
            auth.format,
            IndexesBody {
                indexes: Indexes {
                    last_modified,
                    ignored_articles: articles.join(" "),
                    index: Vec::new(),
                    child: Vec::new(),
                },
            },
        );
    }

    let roots = match load_root_folders(&pool, auth.user.id, query.music_folder_id).await {
        Ok(roots) => roots,
        Err(e) => return internal(e, auth.format, "listing the top level folders"),
    };

    let loose = match load_loose_songs(&pool, auth.user.id, query.music_folder_id).await {
        Ok(songs) => songs,
        Err(e) => return internal(e, auth.format, "listing songs outside any folder"),
    };

    let entries = roots
        .into_iter()
        .map(|(id, name)| NamedEntry { id, name })
        .collect();

    let groups = by_letter(entries, articles, |entry| &entry.name)
        .into_iter()
        .map(|(name, artist)| FolderIndexGroup { name, artist })
        .collect();

    response::ok(
        auth.format,
        IndexesBody {
            indexes: Indexes {
                last_modified,
                ignored_articles: articles.join(" "),
                index: groups,
                child: loose,
            },
        },
    )
}

pub async fn get_music_directory(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
) -> Response {
    let folder: Option<(String, String, Option<String>)> = match sqlx::query_as(concat!(
        visible_libraries!(),
        " SELECT f.public_id, f.name, parent.public_id
            FROM folders f
            LEFT JOIN folders parent ON parent.id = f.parent_id
           WHERE f.public_id = ? AND f.missing_since IS NULL
             AND f.library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(auth.user.id)
    .bind(&query.id)
    .fetch_optional(&pool)
    .await
    {
        Ok(folder) => folder,
        Err(e) => return internal(e, auth.format, "loading a directory"),
    };

    let Some((id, name, parent)) = folder else {
        return ApiError::NotFound.in_format(auth.format).into_response();
    };

    // Subdirectories first, then the songs, which is the order clients expect
    // and the reason the response mixes both in one array.
    let mut children = match load_child_folders(&pool, auth.user.id, &query.id).await {
        Ok(folders) => folders,
        Err(e) => return internal(e, auth.format, "listing subdirectories"),
    };

    match load_songs_of_folder(&pool, auth.user.id, &query.id).await {
        Ok(songs) => children.extend(songs),
        Err(e) => return internal(e, auth.format, "listing the songs of a directory"),
    }

    response::ok(
        auth.format,
        DirectoryBody {
            directory: Directory {
                id,
                name,
                parent,
                child: children,
            },
        },
    )
}

/// Most recent folder modification time, in the milliseconds this endpoint
/// reports. Zero when the library is empty, which is a truthful "never".
async fn load_last_modified(
    pool: &SqlitePool,
    user_id: i64,
    library_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let newest: Option<String> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        " SELECT max(modified_at) FROM folders
           WHERE missing_since IS NULL AND (? IS NULL OR library_id = ?)
             AND library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(user_id)
    .bind(library_id)
    .bind(library_id)
    .fetch_optional(pool)
    .await?
    .flatten();

    Ok(newest
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|parsed| parsed.timestamp_millis())
        .unwrap_or(0))
}

/// The top level a client sees: the children of each library root.
///
/// The root folder itself is a row with no parent, and it is not what anyone
/// wants to see in an index — a library called "music" would show up as a
/// single artist containing everything.
async fn load_root_folders(
    pool: &SqlitePool,
    user_id: i64,
    library_id: Option<i64>,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as(concat!(
        visible_libraries!(),
        " SELECT f.public_id, f.name
            FROM folders f
            JOIN folders root ON root.id = f.parent_id
           WHERE root.parent_id IS NULL AND f.missing_since IS NULL
             AND f.library_id IN (SELECT id FROM visible_libraries)
             AND (? IS NULL OR f.library_id = ?)
           ORDER BY f.name COLLATE NOCASE"
    ))
    .bind(user_id)
    .bind(library_id)
    .bind(library_id)
    .fetch_all(pool)
    .await
}

async fn load_child_folders(
    pool: &SqlitePool,
    user_id: i64,
    parent_public_id: &str,
) -> Result<Vec<Child>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(concat!(
        visible_libraries!(),
        " SELECT f.public_id, f.name
            FROM folders f
            JOIN folders parent ON parent.id = f.parent_id
           WHERE parent.public_id = ? AND f.missing_since IS NULL
             AND f.library_id IN (SELECT id FROM visible_libraries)
           ORDER BY f.name COLLATE NOCASE"
    ))
    .bind(user_id)
    .bind(parent_public_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, name)| Child::directory(id, name, Some(parent_public_id.to_string())))
        .collect())
}

async fn load_songs_of_folder(
    pool: &SqlitePool,
    user_id: i64,
    folder_public_id: &str,
) -> Result<Vec<Child>, sqlx::Error> {
    let rows: Vec<TrackRow> = sqlx::query_as(concat!(
        track_columns!(),
        " AND f.public_id = ?
          ORDER BY t.disc_number, t.track_number, t.title COLLATE NOCASE"
    ))
    .bind(user_id)
    .bind(user_id)
    .bind(folder_public_id)
    .fetch_all(pool)
    .await?;

    build_children(pool, rows).await
}

/// Songs sitting directly in a library root, which `getIndexes` reports beside
/// the folder list because there is no folder a client would open to find them.
async fn load_loose_songs(
    pool: &SqlitePool,
    user_id: i64,
    library_id: Option<i64>,
) -> Result<Vec<Child>, sqlx::Error> {
    let rows: Vec<TrackRow> = sqlx::query_as(concat!(
        track_columns!(),
        " AND f.parent_id IS NULL AND (? IS NULL OR t.library_id = ?)
          ORDER BY t.title COLLATE NOCASE"
    ))
    .bind(user_id)
    .bind(user_id)
    .bind(library_id)
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    build_children(pool, rows).await
}

// ---------------------------------------------------------------------------
// Loading by internal id, for the search module
// ---------------------------------------------------------------------------
//
// Callers resolve what they want into internal ids and then ask for the entities
// themselves, so the shape of a response is built in one place.
//
// These return exactly one entry per id asked for, in the order asked for, and
// that is a contract rather than a detail. An IN clause gives no order at all,
// and gives each matching row only once — so a playlist holding the same song
// twice came back a song short while its own count said otherwise. Both the
// order and the repeats are restored here, which is why the row types carry the
// internal id.

pub(super) async fn load_artists_by_ids(
    pool: &SqlitePool,
    user_id: i64,
    ids: &[i64],
) -> Result<Vec<ArtistId3>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // The expression, its parameter, and then the columns: a QueryBuilder writes a
    // `?` for every argument it takes, so the statement is handed over in pieces
    // either side of each one.
    let mut builder = sqlx::QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(user_id);
    builder.push(concat!(visible_libraries_tail!(), artist_columns_head!()));
    builder.push_bind(user_id);
    builder.push(concat!(artist_columns_tail!(), " AND a.id IN ("));
    push_ids(&mut builder, ids);
    builder.push(")");

    let rows: Vec<ArtistRow> = builder.build_query_as().fetch_all(pool).await?;
    let found: HashMap<i64, ArtistId3> = rows
        .into_iter()
        .map(|row| (row.id, ArtistId3::from(row)))
        .collect();

    Ok(in_requested_order(ids, &found))
}

pub(super) async fn load_albums_by_ids(
    pool: &SqlitePool,
    user_id: i64,
    ids: &[i64],
) -> Result<Vec<AlbumId3>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // The expression, its parameter, and then the columns: a QueryBuilder writes a
    // `?` for every argument it takes, so the statement is handed over in pieces
    // either side of each one.
    let mut builder = sqlx::QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(user_id);
    builder.push(concat!(visible_libraries_tail!(), album_columns_head!()));
    builder.push_bind(user_id);
    // The albums the caller named, minus any it may not see. The callers all
    // choose their identifiers from a filtered query already, but a loader that
    // depends on that is a loader that leaks the day one of them forgets.
    builder.push(concat!(
        " WHERE ",
        album_is_visible!("al.id"),
        " AND al.id IN ("
    ));
    push_ids(&mut builder, ids);
    builder.push(")");

    let rows: Vec<AlbumRow> = builder.build_query_as().fetch_all(pool).await?;

    let mut found = HashMap::with_capacity(rows.len());
    for row in rows {
        let id = row.id;
        found.insert(id, build_album(pool, row).await?);
    }

    Ok(in_requested_order(ids, &found))
}

pub(super) async fn load_tracks_by_ids(
    pool: &SqlitePool,
    user_id: i64,
    ids: &[i64],
) -> Result<Vec<Child>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // The expression, its parameter, and then the columns: a QueryBuilder writes a
    // `?` for every argument it takes, so the statement is handed over in pieces
    // either side of each one.
    let mut builder = sqlx::QueryBuilder::new(visible_libraries_head!());
    builder.push_bind(user_id);
    builder.push(concat!(visible_libraries_tail!(), track_columns_head!()));
    builder.push_bind(user_id);
    builder.push(concat!(track_columns_tail!(), " AND t.id IN ("));
    push_ids(&mut builder, ids);
    builder.push(")");

    let rows: Vec<TrackRow> = builder.build_query_as().fetch_all(pool).await?;
    let row_ids: Vec<i64> = rows.iter().map(|row| row.id).collect();

    let children = build_children(pool, rows).await?;
    let found: HashMap<i64, Child> = row_ids.into_iter().zip(children).collect();

    Ok(in_requested_order(ids, &found))
}

/// One entry per id asked for, in that order, repeats included.
///
/// An id the query did not return simply drops out, which is what should happen
/// to something that no longer resolves. An id given twice appears twice, which
/// is what a playlist holding the same song twice needs.
fn in_requested_order<T: Clone>(ids: &[i64], found: &HashMap<i64, T>) -> Vec<T> {
    ids.iter().filter_map(|id| found.get(id).cloned()).collect()
}

fn push_ids(builder: &mut sqlx::QueryBuilder<sqlx::Sqlite>, ids: &[i64]) {
    let mut separated = builder.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    use crate::db;

    /// Two libraries with an album each, and two people: one who may see
    /// everything and one restricted to the first.
    async fn two_libraries() -> (SqlitePool, i64, i64, Vec<i64>) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        for (id, name) in [(1, "a"), (2, "b")] {
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
                 VALUES (?, ?, ?, ?, ?, 1)",
            )
            .bind(id)
            .bind(format!("f{id}"))
            .bind(id)
            .bind(name)
            .bind(format!("/{name}"))
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO albums (id, public_id, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("alb{id}"))
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

        let mut users = Vec::new();
        for name in ["everybody", "restricted"] {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
                 VALUES (?, 'x', 0, ?, ?) RETURNING id",
            )
            .bind(name)
            .bind(&at)
            .bind(&at)
            .fetch_one(&pool)
            .await
            .unwrap();
            users.push(id);
        }

        // The second one may see the first library only.
        sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, 1)")
            .bind(users[1])
            .execute(&pool)
            .await
            .unwrap();

        (pool, users[0], users[1], vec![1, 2])
    }

    /// The loaders that take a list of identifiers are assembled by a
    /// `QueryBuilder`, which writes a `?` for every argument it is given. That
    /// makes their parameter order the easiest thing here to get wrong, and a
    /// wrong one is quiet: the answer comes back short rather than failing.
    #[tokio::test]
    async fn loading_by_ids_leaves_out_what_may_not_be_seen() {
        let (pool, unrestricted, restricted, ids) = two_libraries().await;

        let albums = load_albums_by_ids(&pool, unrestricted, &ids).await.unwrap();
        assert_eq!(albums.len(), 2, "no restriction means every album");

        let albums = load_albums_by_ids(&pool, restricted, &ids).await.unwrap();
        assert_eq!(albums.len(), 1, "only the album of the allowed library");
        assert_eq!(albums[0].name, "Album a");

        let tracks = load_tracks_by_ids(&pool, unrestricted, &ids).await.unwrap();
        assert_eq!(tracks.len(), 2);

        let tracks = load_tracks_by_ids(&pool, restricted, &ids).await.unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Song a");

        let artists = load_artists_by_ids(&pool, unrestricted, &ids)
            .await
            .unwrap();
        assert_eq!(artists.len(), 2);

        let artists = load_artists_by_ids(&pool, restricted, &ids).await.unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, "Artist a");
    }

    #[tokio::test]
    async fn naming_a_hidden_album_is_a_miss_not_an_empty_shell() {
        let (pool, unrestricted, restricted, _) = two_libraries().await;

        assert!(
            load_album(&pool, unrestricted, "alb2")
                .await
                .unwrap()
                .is_some(),
            "visible to somebody with no restriction"
        );
        assert!(
            load_album(&pool, restricted, "alb2")
                .await
                .unwrap()
                .is_none(),
            "not there as far as this account is concerned"
        );
        assert!(
            load_album(&pool, restricted, "alb1")
                .await
                .unwrap()
                .is_some(),
            "and the one it may see is still there"
        );
    }

    #[tokio::test]
    async fn a_disabled_library_is_hidden_from_everybody() {
        let (pool, unrestricted, _, ids) = two_libraries().await;

        sqlx::query("UPDATE libraries SET enabled = 0 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        let albums = load_albums_by_ids(&pool, unrestricted, &ids).await.unwrap();
        assert_eq!(albums.len(), 1, "switched off, so nobody sees it");
    }
}
