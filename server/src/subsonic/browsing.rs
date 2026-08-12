// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Browsing the catalogue by tags: artists, their albums, their songs.
//!
//! Every query here filters out what the scanner marked as absent. A track
//! whose file is gone keeps its row so the user's data survives, but it has no
//! business appearing in a listing.

use super::asked::Asked;
use super::auth::Authenticated;
use super::error::ApiError;
use super::models::{
    AlbumId3, ArtistId3, Child, DiscTitle, ItemGenre, NamedEntry, ReplayGain, seconds,
};
use super::response;
use crate::settings;
use axum::extract::State;
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

/// What this server holds of the three "info" calls, which is one field.
///
/// The rest of what they were made to carry is what a scrobbling service knew and
/// this one does not: a biography, a page on the web, and three sizes of picture
/// hosted elsewhere. None of them is invented here.
///
/// The picture in particular is not missing, it is somewhere better: an artist and
/// a record both come with `coverArt` on them already, which is an id for
/// getCoverArt and needs nobody to guess this server's address from the outside.
/// The three `*ImageUrl` fields are absolute URLs, and behind a proxy an absolute
/// URL made up by the server is a guess handed to the client as a fact.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    #[serde(skip_serializing_if = "Option::is_none")]
    music_brainz_id: Option<String>,
}

/// `getArtistInfo` and `getArtistInfo2` differ in the element they answer in and
/// in nothing else here — the similar artists they would also carry are what this
/// server does not know.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistInfoBody {
    artist_info: Info,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistInfo2Body {
    artist_info2: Info,
}

/// Both album calls answer in `albumInfo`. Not a slip: the specification names
/// getAlbumInfo2 for its argument, an ID3 album id, and leaves the element alone.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumInfoBody {
    album_info: Info,
}

/// What getTopSongs asks with: a name rather than an id, which is the call's own
/// oddity and not ours — it predates the identified artists by several versions.
///
/// Which is what the `topSongsByArtistId` extension is for, and this server
/// declares it: an `id` may come instead, and where both come the id wins. So
/// neither is required on its own and one of the two still is — a request naming
/// no artist at all is answered as the missing parameter it is.
#[derive(Debug, Deserialize)]
pub(super) struct TopSongsQuery {
    id: Option<String>,
    artist: Option<String>,
    count: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TopSongsBody {
    top_songs: TopSongs,
}

#[derive(Serialize)]
struct TopSongs {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    song: Vec<Child>,
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
    Asked(query): Asked<IdQuery>,
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
    Asked(query): Asked<IdQuery>,
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
    Asked(query): Asked<IdQuery>,
) -> Response {
    match load_song(&pool, auth.user.id, &query.id).await {
        Ok(Some(song)) => response::ok(auth.format, SongBody { song }),
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "loading a song"),
    }
}

/// What is known about an artist beyond the artist itself.
///
/// Both spellings of the call are answered from the same place, because the id
/// they take is the same id: an artist as this server hands it out. A client
/// browsing by folder passes a directory id instead and gets a 70, which is the
/// truth — that id names a folder, and there is no artist by it.
pub async fn get_artist_info(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<IdQuery>,
) -> Response {
    match load_artist(&pool, auth.user.id, &query.id).await {
        Ok(Some(artist)) => response::ok(
            auth.format,
            ArtistInfoBody {
                artist_info: Info {
                    music_brainz_id: artist.music_brainz_id,
                },
            },
        ),
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "loading what is known of an artist"),
    }
}

pub async fn get_artist_info2(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<IdQuery>,
) -> Response {
    match load_artist(&pool, auth.user.id, &query.id).await {
        Ok(Some(artist)) => response::ok(
            auth.format,
            ArtistInfo2Body {
                artist_info2: Info {
                    music_brainz_id: artist.music_brainz_id,
                },
            },
        ),
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "loading what is known of an artist"),
    }
}

/// The same for a record, and for both spellings of that call too.
pub async fn get_album_info(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<IdQuery>,
) -> Response {
    match load_album(&pool, auth.user.id, &query.id).await {
        Ok(Some(album)) => response::ok(
            auth.format,
            AlbumInfoBody {
                album_info: Info {
                    music_brainz_id: album.music_brainz_id,
                },
            },
        ),
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "loading what is known of a record"),
    }
}

/// Songs returned when a client does not say how many, and the ceiling on what it
/// may ask for. Fifty is the number the call itself names as its default.
const DEFAULT_TOP_SONGS: i64 = 50;
const MAX_TOP_SONGS: i64 = 500;

/// An artist's songs, most played first.
///
/// The call was invented for what a scrobbling service knows and this server does
/// not: what the world plays most of an artist. What it does know is what *this
/// listener* has played, which is the only honest answer available here and the
/// more useful one on a shelf somebody chose themselves.
///
/// Songs nobody has played yet come last rather than being left out — `DESC` puts
/// the nulls at the end, which is the behaviour wanted and not an accident. A
/// server on its first day has no plays at all, and an artist's page with nothing
/// under "top songs" tells the listener less than the same page with the artist's
/// songs in it; as the counts arrive, the list becomes what it says it is.
///
/// Any credit counts — signed or played on — the way it does everywhere else an
/// artist is looked up.
///
/// By id where the client sends one, which is the `topSongsByArtistId` extension
/// and the better of the two: a name is not an identity, and two people can share
/// one. An id nobody answers to is a 70, the way it is on every other call that
/// names a particular thing — where a *name* nobody answers to is an empty list,
/// because a name that matches nothing is a search that found nothing.
pub async fn get_top_songs(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<TopSongsQuery>,
) -> Response {
    let count = query
        .count
        .filter(|count| *count > 0)
        .unwrap_or(DEFAULT_TOP_SONGS)
        .min(MAX_TOP_SONGS);

    // The id wins where both come, which the extension is explicit about, and it
    // travels to the statement as an id: resolved to a name and matched as one, it
    // would hand back the songs of everybody who happens to share it, which is the
    // very ambiguity the extension exists to remove.
    //
    // Looked up first all the same, for two reasons. An id nobody answers to is a
    // 70 rather than an empty list, and the lookup is what keeps the wall around a
    // library standing: an id this account may not see is not there at all.
    let credited = match (&query.id, &query.artist) {
        (Some(id), _) => match load_artist(&pool, auth.user.id, id).await {
            Ok(Some(_)) => Credited::Id(id),
            Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
            Err(e) => return internal(e, auth.format, "looking up an artist by id"),
        },
        (None, Some(name)) => Credited::Named(name),
        (None, None) => {
            return ApiError::MissingParameter("artist")
                .in_format(auth.format)
                .into_response();
        }
    };

    let ids = match top_song_ids(&pool, auth.user.id, credited, count).await {
        Ok(ids) => ids,
        Err(e) => return internal(e, auth.format, "picking an artist's most played songs"),
    };

    match load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(songs) => response::ok(
            auth.format,
            TopSongsBody {
                top_songs: TopSongs { song: songs },
            },
        ),
        Err(e) => internal(e, auth.format, "loading an artist's most played songs"),
    }
}

/// Which artist the songs are wanted of, and how they were asked for.
#[derive(Debug, Clone, Copy)]
enum Credited<'a> {
    /// By the id this server handed out, which names one artist and no other.
    Id(&'a str),
    /// By the name the client typed or read off a song, which may name more.
    Named(&'a str),
}

/// The tracks credited to an artist, most played by this listener first.
///
/// Apart from the handler so that the order and the wall around a library can be
/// asserted without standing up a router: what a client gets out of this is one
/// `ORDER BY` and one `EXISTS`, and both are worth a test of their own.
///
/// One statement for both ways of asking rather than two nearly identical ones:
/// whichever half is not being used is bound as null and the clause stands aside.
async fn top_song_ids(
    pool: &SqlitePool,
    user_id: i64,
    credited: Credited<'_>,
    count: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let (id, name) = match credited {
        Credited::Id(id) => (Some(id), None),
        Credited::Named(name) => (None, Some(name)),
    };

    sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT t.id
           FROM tracks t
           LEFT JOIN user_track_stats s ON s.track_id = t.id AND s.user_id = ?
          WHERE t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
            AND EXISTS (
                    SELECT 1 FROM track_artists ta
                      JOIN artists ar ON ar.id = ta.artist_id
                     WHERE ta.track_id = t.id
                       AND (? IS NULL OR ar.public_id = ?)
                       AND (? IS NULL OR ar.name = ?)
                )
          ORDER BY s.play_count DESC, t.title COLLATE NOCASE
          LIMIT ?"
    ))
    .bind(user_id)
    .bind(user_id)
    .bind(id)
    .bind(id)
    .bind(name)
    .bind(name)
    .bind(count)
    .fetch_all(pool)
    .await
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
            // Their own id doubles as the handle for their picture, the way an
            // album's does. Said whether or not one has been found yet, because
            // finding it is what the asking sets off: it is looked for on disk the
            // first time somebody wants it, and an artist who never announced a
            // picture would never be asked about and so never looked at. A refusal
            // is the answer where there is none, and it is remembered.
            cover_art: Some(row.public_id.clone()),
            id: row.public_id,
            name: row.name,
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
///
/// The count gathers the albums the artist is named on and then asks which of
/// those are visible, rather than walking the albums asking which are theirs.
/// Same answer, and the difference is everything: written the other way round the
/// filter sits in a table other than the one being walked, so no index can serve
/// it and every album is examined once per artist — 736 artists over 1078 albums
/// measured at ten seconds on a slow machine, for the first call every client
/// makes. Read this way each branch of the union is an index lookup on the
/// artist.
/// The records that are an artist's, as a set gathered from the artist.
///
/// One expression because there is one answer, and it is asked for twice: once
/// as a figure beside the name in a listing, once as the records themselves when
/// somebody opens that name. Written twice they drifted, and drifted quietly —
/// the figure said one record and the discography under it held four, with no
/// way for a client to tell which of the two was lying.
///
/// Two ways of being theirs, and they ask different things about what is left to
/// play. A record they **sign** is theirs however its files are doing, which is
/// what keeps a discography on screen when a disk is unmounted. A record they
/// merely **play on** needs a track of theirs still there to be played.
///
/// What they do not differ on is the wall. Both ask that the record be somewhere
/// this person may look — a file that is away is the disk's doing, and a library
/// walled off is an administrator's decision about what an account may know.
///
/// `album_id` is nullable, and a track filed under no record must not be counted
/// as one, so the branch that reaches records through tracks says so. As a
/// condition on `al.id` a null simply never matched; as a row to count it would
/// have been a record that is not there.
///
/// `$artist` is an expression for the artist's row id, named twice.
macro_rules! records_of_artist {
    ($artist:literal) => {
        concat!(
            "SELECT signed.album_id AS id FROM album_artists signed
              WHERE signed.artist_id = ",
            $artist,
            "
                AND EXISTS (SELECT 1 FROM tracks t
                             WHERE t.album_id = signed.album_id
                               AND t.library_id IN (SELECT id FROM visible_libraries))
             UNION
             SELECT t.album_id FROM tracks t
               JOIN track_artists ta ON ta.track_id = t.id
              WHERE ta.artist_id = ",
            $artist,
            "
                AND t.album_id IS NOT NULL
                AND t.missing_since IS NULL
                AND t.library_id IN (SELECT id FROM visible_libraries)"
        )
    };
}

macro_rules! artist_columns_head {
    () => {
        concat!(
            "
    SELECT a.id, a.public_id, a.name, a.sort_name, a.mbid,
           (SELECT count(*) FROM (",
            records_of_artist!("a.id"),
            ")) AS album_count,
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
    // The same set the figure beside the name is counted from, said once — see
    // `records_of_artist!` for which records are hers and why the two ways of
    // being hers ask different things.
    //
    // The name is turned into a row id by the set rather than joined to on every
    // candidate, which is what lets an index reach it. Asked of each album
    // instead — `WHERE EXISTS (…this album is hers…)` — the catalogue is walked
    // record by record and her tracks are looked up again for every one of them:
    // four hundred thousand page reads to answer with five records, on a
    // collection of three and a half thousand. Gathered from her it is nine
    // hundred, and nothing is walked.
    let rows: Vec<AlbumRow> = sqlx::query_as(concat!(
        album_columns!(),
        " WHERE al.id IN (",
        records_of_artist!("(SELECT id FROM artists WHERE public_id = ?)"),
        ") ORDER BY al.year, coalesce(al.sort_name, al.name) COLLATE NOCASE"
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
    /// How the file credits whoever is on the track, whole. Where it is null the
    /// names are the credit, and joining them says the same thing.
    artist_credit: Option<String>,
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
    SELECT t.id, t.public_id, t.title, t.sort_title, t.artist_credit,
           t.track_number, t.disc_number,
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

/// Attaches artists, genres and the record's own credit to a batch of tracks.
///
/// Three queries for the whole batch rather than three per track: the credits of
/// a fifty track album are one round trip, not a hundred and fifty.
async fn build_children(pool: &SqlitePool, rows: Vec<TrackRow>) -> Result<Vec<Child>, sqlx::Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<&str> = rows.iter().map(|row| row.public_id.as_str()).collect();
    let mut artists = artists_by_track(pool, &ids).await?;
    let mut genres = genres_by_track(pool, &ids).await?;

    // Asked for by album and not by track, because that is where the credit
    // lives: a batch is usually one record's worth of songs, and asking each of
    // them who the record is by would be asking the same question fifty times.
    let mut albums: Vec<&str> = rows
        .iter()
        .filter_map(|row| row.album_public_id.as_deref())
        .collect();
    albums.sort_unstable();
    albums.dedup();
    let credits = album_artists_by_album(pool, &albums).await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let track_artists = artists.remove(&row.public_id).unwrap_or_default();
            let track_genres = genres.remove(&row.public_id).unwrap_or_default();
            // Cloned rather than taken: the songs of one record all point at the
            // same credit, and the second one to ask must find it still there.
            let credited = row
                .album_public_id
                .as_deref()
                .and_then(|album| credits.get(album))
                .cloned()
                .unwrap_or_default();

            build_child(row, track_artists, credited, track_genres)
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

/// Who the record a batch of songs comes from is credited to.
///
/// A song carries this as well as its own artist, and the two are different
/// questions — the album artist is how a client groups a record whose tracks are
/// each by somebody else, and how it files a song heard on its own under the
/// record it belongs to.
async fn album_artists_by_album(
    pool: &SqlitePool,
    album_ids: &[&str],
) -> Result<HashMap<String, Vec<(String, String)>>, sqlx::Error> {
    if album_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut builder = sqlx::QueryBuilder::new(
        "SELECT al.public_id, ar.public_id, ar.name
           FROM album_artists aa
           JOIN albums al ON al.id = aa.album_id
           JOIN artists ar ON ar.id = aa.artist_id
          WHERE aa.role = 'albumartist' AND al.public_id IN (",
    );
    let mut separated = builder.separated(", ");
    for id in album_ids {
        separated.push_bind(*id);
    }
    builder.push(") ORDER BY al.public_id, aa.position");

    let rows: Vec<(String, String, String)> = builder.build_query_as().fetch_all(pool).await?;

    let mut grouped: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (album, artist_id, name) in rows {
        grouped.entry(album).or_default().push((artist_id, name));
    }

    Ok(grouped)
}

fn build_child(
    row: TrackRow,
    artists: Vec<(String, String)>,
    album_artists: Vec<(String, String)>,
    genres: Vec<String>,
) -> Child {
    // The file's own credit where it wrote one, and the names joined where it did
    // not. A record that says "A feat. B" says something a list cannot: joining the
    // two names back up gives "A, B", which is the same people and not the same
    // sentence.
    let display_artist = row
        .artist_credit
        .clone()
        .or_else(|| display_names(&artists));
    let display_album_artist = display_names(&album_artists);
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
        album_artists: album_artists
            .into_iter()
            .map(|(id, name)| ArtistId3::named(id, name))
            .collect(),
        display_album_artist,
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
    Asked(query): Asked<IndexesQuery>,
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
    Asked(query): Asked<IdQuery>,
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
                "INSERT INTO albums (id, public_id, grouping_key, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("alb{id}"))
            .bind(format!("album {name}"))
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

    /// Both ways an album is an artist's, since the count gathers them as two
    /// branches and losing either is quiet: the name stays in the listing and
    /// reports nothing behind it, which is what a client shows as an empty shelf.
    #[tokio::test]
    async fn an_album_counts_for_who_signs_it_and_for_who_plays_on_it() {
        let (pool, everybody, _, _) = two_libraries().await;
        let at = db::now();

        // A name that only ever signs: album artist of the first album, credited
        // on no track of it — which is how a file writes a compilation.
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (3, 'art3', 'Various Artists', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO album_artists (album_id, artist_id, role)
             VALUES (1, 3, 'albumartist')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // And a second record of theirs with nothing left in it, so the count has
        // something it must leave out.
        sqlx::query(
            "INSERT INTO albums (id, public_id, grouping_key, name, created_at, updated_at)
             VALUES (3, 'alb3', 'album gone', 'Album Gone', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO album_artists (album_id, artist_id, role)
             VALUES (3, 3, 'albumartist')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                 file_size, file_modified_at, content_type, suffix, title,
                                 missing_since, last_seen_scan, created_at, updated_at)
             VALUES (3, 'trk3', 1, 1, 3, '/a/gone.wav', 1, ?, 'audio/wav', 'wav', 'Gone',
                     '2026-08-01T09:00:00Z', 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let artists = load_artists(&pool, everybody).await.unwrap();
        let of = |name: &str| {
            artists
                .iter()
                .find(|a| a.name == name)
                .unwrap_or_else(|| panic!("{name} is missing from the listing"))
                .album_count
        };

        assert_eq!(
            of("Various Artists"),
            Some(2),
            "both records they sign, though they play on neither and one of them \
             has no file left: a record somebody signs is theirs however its files \
             are doing"
        );
        assert_eq!(
            of("Artist a"),
            Some(1),
            "the record they play on, though somebody else signs it"
        );

        // And the figure is the length of the list it stands for. They are one
        // expression precisely so this holds; written twice they drifted, and the
        // drift showed up as a client printing "1 album" over a shelf of four.
        for name in ["Various Artists", "Artist a"] {
            let public_id = &artists.iter().find(|a| a.name == name).unwrap().id;
            let records = load_albums_of_artist(&pool, everybody, public_id)
                .await
                .unwrap();

            assert_eq!(
                of(name),
                Some(records.len() as i64),
                "{name} is counted for {:?} records and opens onto {}",
                of(name),
                records.len()
            );
        }
    }

    /// A discography holds every record that is theirs, and the two ways of being
    /// theirs do not ask the same thing about what is left to play.
    ///
    /// A record they sign is theirs whether or not its files are still there:
    /// that is what keeps a discography on screen when a disk is unmounted, and
    /// dropping it is how a name ends up in the listing with an empty shelf
    /// behind it. A record they only play on has to have a track of theirs that
    /// can still be played.
    ///
    /// What neither branch may do is reach past the wall, and one of them did.
    /// A file that is away is the disk's doing; a library walled off is an
    /// administrator's decision about what this account may know, and answering
    /// with the title and year of a record from one is handing over the thing
    /// that was to be kept back — whatever the album, the cover and the songs go
    /// on to refuse afterwards.
    ///
    /// Worth pinning here because the statement gathers those records from the
    /// artist rather than asking each record about them, and a set gathered
    /// wrongly is quiet — the answer comes back short, never wrong-looking.
    #[tokio::test]
    async fn a_discography_holds_what_they_sign_and_what_they_can_still_be_heard_on() {
        let (pool, everybody, restricted, _) = two_libraries().await;
        let at = db::now();

        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (3, 'art3', 'Her', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        // Six records: what she signs, what she plays on, and the ones that are
        // hers only in a way this person cannot reach.
        for (album, name) in [
            (10, "Signed, Every File Away"),
            (11, "Plays On It"),
            (12, "Plays On It, File Away"),
            (13, "Plays On It, Other Library"),
            (14, "Signed, Other Library"),
            (15, "Signed, No Tracks At All"),
        ] {
            sqlx::query(
                "INSERT INTO albums (id, public_id, grouping_key, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(album)
            .bind(format!("alb{album}"))
            .bind(name)
            .bind(name)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        // She signs three: one in a library everybody may open, one behind the
        // wall, and one with nothing under it at all — which is what removing a
        // library leaves behind until somebody purges. The rest carry somebody
        // else's signature, so they can only be hers through a track.
        for album in [10, 14, 15] {
            sqlx::query(
                "INSERT INTO album_artists (album_id, artist_id, role)
                 VALUES (?, 3, 'albumartist')",
            )
            .bind(album)
            .execute(&pool)
            .await
            .unwrap();
        }
        for album in [11, 12, 13] {
            sqlx::query(
                "INSERT INTO album_artists (album_id, artist_id, role) VALUES (?, 1, 'albumartist')",
            )
            .bind(album)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Album 15 gets none, on purpose.
        for (track, album, library, gone) in [
            (10, 10, 1, true),
            (11, 11, 1, false),
            (12, 12, 1, true),
            (13, 13, 2, false),
            (14, 14, 2, false),
        ] {
            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                     file_size, file_modified_at, content_type, suffix, title,
                                     missing_since, last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, 1, ?, 'audio/wav', 'wav', 'Song', ?, 1, ?, ?)",
            )
            .bind(track)
            .bind(format!("trk{track}"))
            .bind(library)
            .bind(library)
            .bind(album)
            .bind(format!("/{track}.wav"))
            .bind(&at)
            .bind(gone.then(|| at.clone()))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO track_artists (track_id, artist_id, role) VALUES (?, 3, 'artist')",
            )
            .bind(track)
            .execute(&pool)
            .await
            .unwrap();
        }

        let named = |albums: &[AlbumId3]| {
            let mut names: Vec<String> = albums.iter().map(|a| a.name.clone()).collect();
            names.sort();
            names
        };

        assert_eq!(
            named(
                &load_albums_of_artist(&pool, everybody, "art3")
                    .await
                    .unwrap()
            ),
            vec![
                "Plays On It".to_string(),
                "Plays On It, Other Library".to_string(),
                "Signed, Every File Away".to_string(),
                "Signed, Other Library".to_string(),
            ],
            "what she signs stays whatever became of its files; what she only \
             plays on needs a track of hers still there; and a record with no \
             tracks under it at all is nobody's discography"
        );

        assert_eq!(
            named(
                &load_albums_of_artist(&pool, restricted, "art3")
                    .await
                    .unwrap()
            ),
            vec![
                "Plays On It".to_string(),
                "Signed, Every File Away".to_string(),
            ],
            "the wall holds for both ways of being hers: not the one she plays \
             on in the other library, and not the one she signs there either — \
             the title and the year of a record are the thing being kept back"
        );

        assert!(
            load_albums_of_artist(&pool, everybody, "nobody-by-this-name")
                .await
                .unwrap()
                .is_empty(),
            "a name that is not there owns nothing"
        );

        // The figure a listing prints beside the name counts this same set, for
        // whoever is asking — including the one who is walled off, whose figure
        // must come down with her list rather than stay at what somebody else
        // can see.
        for (who, whom) in [("everybody", everybody), ("the restricted one", restricted)] {
            let listed = load_artists(&pool, whom)
                .await
                .unwrap()
                .into_iter()
                .find(|a| a.id == "art3")
                .expect("she is on both listings");
            let records = load_albums_of_artist(&pool, whom, "art3").await.unwrap();

            assert_eq!(
                listed.album_count,
                Some(records.len() as i64),
                "{who} is told {:?} records and opens onto {}",
                listed.album_count,
                records.len()
            );
        }
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

    /// A song says who the record it is on is by, which is not the same question
    /// as who played on the track. Clients read it to file a song heard on its
    /// own under its record, and to group a record whose every track is by
    /// somebody else — and it was going out empty on every song we served.
    #[tokio::test]
    async fn a_song_carries_the_credit_of_the_record_it_is_on() {
        let (pool, everybody, _, ids) = two_libraries().await;
        let at = db::now();

        // Credited to somebody other than whoever is on the track, which is the
        // difference the two fields exist to carry.
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (9, 'art9', 'The Band', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO album_artists (album_id, artist_id, role, position)
             VALUES (1, 9, 'albumartist', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let songs = load_tracks_by_ids(&pool, everybody, &ids).await.unwrap();

        let credited: Vec<&str> = songs[0]
            .album_artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect();
        assert_eq!(credited, ["The Band"]);
        assert_eq!(songs[0].display_album_artist.as_deref(), Some("The Band"));
        assert_eq!(
            songs[0].display_artist.as_deref(),
            Some("Artist a"),
            "and whoever is on the track is still whoever is on the track"
        );

        // The other record is credited to nobody, and nobody else's credit
        // reaches it.
        assert!(songs[1].album_artists.is_empty());
        assert!(songs[1].display_album_artist.is_none());
    }

    /// An artist says where their picture is, and that handle is their own id.
    ///
    /// Without it a client has nothing to ask with, so however many pictures we find
    /// on disk none of them is ever drawn — and worse, none is ever looked for: the
    /// asking is what sets off the search. Said whether or not one has been found
    /// yet, exactly as an album says it.
    #[tokio::test]
    async fn an_artist_says_where_their_picture_is() {
        let (pool, everybody, restricted, ids) = two_libraries().await;

        let artists = load_artists_by_ids(&pool, everybody, &ids).await.unwrap();
        assert_eq!(artists.len(), 2);

        for artist in &artists {
            assert_eq!(
                artist.cover_art.as_deref(),
                Some(artist.id.as_str()),
                "their own id is the handle for their picture"
            );
        }

        // And asking with it is refused for an artist whose music this account may
        // not see — the same wall the rest of the catalogue is behind.
        let hidden = artists
            .iter()
            .find(|artist| artist.name == "Artist b")
            .expect("the artist of the second library");

        assert!(
            crate::media::resolve_artist(&pool, everybody, &hidden.id)
                .await
                .unwrap()
                .is_some(),
            "visible to somebody with no restriction"
        );
        assert!(
            crate::media::resolve_artist(&pool, restricted, &hidden.id)
                .await
                .unwrap()
                .is_none(),
            "and not there at all as far as this account is concerned"
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

    /// The most played first, the unplayed after them, and never one from a
    /// library this account may not see.
    ///
    /// Both halves are quiet when they break. An order that goes the other way is
    /// a list that still looks like a list, and a track reached across the wall
    /// arrives with its title and its record on it — which is the thing that was
    /// to be kept back.
    #[tokio::test]
    async fn the_most_played_songs_of_an_artist_come_first_and_only_from_where_they_may() {
        let (pool, everybody, restricted, _) = two_libraries().await;
        let at = db::now();

        // A second song for the first artist, in the library everybody sees, and
        // the only one with a play behind it.
        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                 file_size, file_modified_at, content_type, suffix, title,
                                 last_seen_scan, created_at, updated_at)
             VALUES (3, 'trk3', 1, 1, 1, '/a/two.wav', 1, ?, 'audio/wav', 'wav', 'Another',
                     1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role) VALUES (3, 1, 'artist')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_track_stats (user_id, track_id, play_count) VALUES (?, 3, 7)",
        )
        .bind(everybody)
        .execute(&pool)
        .await
        .unwrap();

        // And the same artist credited on the song in the walled-off library.
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role) VALUES (2, 1, 'artist')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let top = top_song_ids(&pool, everybody, Credited::Named("Artist a"), 50)
            .await
            .unwrap();
        assert_eq!(
            top,
            [3, 1, 2],
            "the played one first, then the rest by title"
        );

        // Whose plays they are, since the count sits in a row per listener. The
        // other account played the other song, and is walled out of the second
        // library — so both halves show in one answer.
        sqlx::query(
            "INSERT INTO user_track_stats (user_id, track_id, play_count) VALUES (?, 1, 2)",
        )
        .bind(restricted)
        .execute(&pool)
        .await
        .unwrap();

        let theirs = top_song_ids(&pool, restricted, Credited::Named("Artist a"), 50)
            .await
            .unwrap();
        assert_eq!(
            theirs,
            [1, 3],
            "their own plays lead, and nothing from the library barred"
        );

        let asked = top_song_ids(&pool, everybody, Credited::Named("Artist a"), 1)
            .await
            .unwrap();
        assert_eq!(asked, [3], "and the count is a count");
    }

    /// Asked by id, which is what the `topSongsByArtistId` extension adds, and the
    /// reason it is worth having: two people can be called the same thing, and a
    /// name asks for both of them.
    #[tokio::test]
    async fn an_artists_id_names_one_artist_where_their_name_may_name_two() {
        let (pool, everybody, _, _) = two_libraries().await;
        let at = db::now();

        // A second artist by the same name, with a song of their own in the library
        // everybody can see. Which happens for real — two bands, one word — and it
        // is the whole reason a client should be sending the id.
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (3, 'art3', 'Artist a', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                 file_size, file_modified_at, content_type, suffix, title,
                                 last_seen_scan, created_at, updated_at)
             VALUES (3, 'trk3', 1, 1, 1, '/a/other.wav', 1, ?, 'audio/wav', 'wav', 'Namesake',
                     1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role) VALUES (3, 3, 'artist')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let by_name = top_song_ids(&pool, everybody, Credited::Named("Artist a"), 50)
            .await
            .unwrap();
        assert_eq!(by_name, [3, 1], "a name asks for both of them");

        let by_id = top_song_ids(&pool, everybody, Credited::Id("art1"), 50)
            .await
            .unwrap();
        assert_eq!(by_id, [1], "and an id for the one it names");

        let theirs = top_song_ids(&pool, everybody, Credited::Id("art3"), 50)
            .await
            .unwrap();
        assert_eq!(theirs, [3], "each to their own");
    }
}
