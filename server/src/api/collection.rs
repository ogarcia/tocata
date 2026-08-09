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
    Album, AlbumDetail, AlbumTrack, Albums, Artist, ArtistAlbum, ArtistDetail, Artists,
    Attribution, Credit, ErrorBody, Genre, GenreDetail, Genres, LyricLine, LyricSource, Lyrics,
    PlayedTrack, Queue, Tags, Track, TrackDetail, Tracks,
};
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use utoipa::IntoParams;

/// The records that are an artist's, here where a record that has lost its files is
/// something to look at rather than something to hide.
///
/// That is the whole of what makes this different from the set OpenSubsonic answers
/// with: a client is browsing music to play, and this is where somebody comes to find
/// out what is gone. So a record of theirs is one holding a track of theirs in a
/// library this account may look in, present or not — and every record here carries
/// its own count of what is missing, which is the point.
///
/// One expression, used by the figure and by the list under it, because they are one
/// answer. Written twice they drifted: the count asked that the files still be there
/// and the list did not, so unmounting a disk left the panel reading "1 record" above
/// two of them.
///
/// `$artist` is an expression for the artist's row id, named twice by the set inside.
macro_rules! records_of_artist {
    ($artist:literal) => {
        concat!(
            "SELECT t.album_id AS id FROM tracks t
              WHERE ",
            track_is_theirs!("t", $artist),
            "
                AND t.album_id IS NOT NULL
                AND t.library_id IN (SELECT id FROM visible_libraries)"
        )
    };
}

/// How many rows a page holds when nobody says. Enough to fill a tall window
/// once, so the first screenful takes one request.
const PAGE: i64 = 50;

/// What a track's row is read from, up to the `WHERE`.
///
/// Written once because two things read it: the listing, and one track on its own.
/// They have to agree — the second is what the player asks for when it moves to a
/// track the listing never fetched, and a title that came out differently there
/// would be a different song as far as anybody watching the sidebar is concerned.
///
/// The credits are read through their role, and the roles are the ones the scanner
/// writes: `artist` on a track and `albumartist` on a record. Anything else comes
/// back null in silence.
///
/// How the file credits them wins over the names joined by commas, and the names are
/// what answers when it does not say. The two are not the same fact: the credit is
/// where "Above & Beyond feat. Zoë Johnston" is written down, and nothing joining a
/// list back together can invent the word between them. The column is only filled
/// when it says something the names do not, so the `coalesce` needs no condition of
/// its own — see `Metadata::credited_as`.
macro_rules! a_tracks_row {
    () => {
        "SELECT t.public_id, t.title,
                coalesce(t.artist_credit,
                  (SELECT group_concat(a.name, ', ')
                     FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
                    WHERE ta.track_id = t.id AND ta.role = 'artist')) AS artists,
                al.name AS album, al.public_id AS album_id,
                (SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                  WHERE tg.track_id = t.id ORDER BY g.name LIMIT 1) AS genre,
                t.track_number, t.duration_ms, t.suffix, t.bit_rate,
                t.missing_since IS NOT NULL AS missing
           FROM tracks t
           LEFT JOIN albums al ON al.id = t.album_id
           LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.role = 'albumartist'
           LEFT JOIN artists ar ON ar.id = aa.artist_id"
    };
}

/// How many of an artist's songs a panel lists as their most played.
///
/// Five, which is a handful somebody reads rather than a chart they scroll: the point
/// of that section is what this artist amounts to in this house, and a longer list
/// answers a question nobody asked.
///
/// A macro and not a constant because it is pasted into a statement by `concat!`,
/// which takes literals and nothing else — the same reason the column lists here are
/// macros.
macro_rules! most_played {
    () => {
        "5"
    };
}

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
    /// Named tracks and no others, as identifiers separated by commas.
    ///
    /// What a queue is drawn with. A queue is identifiers, so the moment something
    /// wants to show it as rows — titles, who made them, how long they run — it has
    /// a handful of identifiers and nothing to print. This answers that in one
    /// request instead of one per track.
    ///
    /// The order is the listing's own, not the order they were named in: whoever
    /// asked knows what order they wanted, and reordering fifty rows client-side is
    /// nothing next to a second statement here that could not use the index.
    pub ids: Option<String>,
}

impl Filter {
    /// The identifiers asked for, if any, with anything unreasonable dropped.
    ///
    /// Capped at the size of a page: this is for drawing rows, and nobody reads more
    /// rows than that at once. Empty strings go — `"a,,b"` is two identifiers and a
    /// typo, not three — and an empty list is `None`, which narrows nothing rather
    /// than matching nothing.
    fn named(&self) -> Option<Vec<String>> {
        let ids: Vec<String> = self
            .ids
            .as_deref()?
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .take(MOST as usize)
            .map(str::to_string)
            .collect();

        (!ids.is_empty()).then_some(ids)
    }
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
        a_tracks_row!(),
        " WHERE t.library_id IN (SELECT id FROM visible_libraries)"
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

/// One track
///
/// The same row a listing draws, for one track named by its identifier.
///
/// What asks for this is a player moving through a queue. A queue is identifiers —
/// that is what makes it affordable to hold thousands of them — so the moment it
/// steps onto a track that was never on screen, nothing knows what to call it. This
/// is that answer, and it reads the same columns the listing does, so the sidebar and
/// the row it came from cannot disagree about a title.
///
/// A track in a library this account may not see is not a track it may know exists,
/// so that answers the same 404 as one that is not there at all.
#[utoipa::path(
    get,
    path = "/tracks/{id}",
    tag = "collection",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 200, description = "The track", body = Track),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such track, or not one you may see", body = ErrorBody),
    )
)]
pub async fn track(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Track>, ApiError> {
    let row: Option<TrackRow> = sqlx::query_as(concat!(
        visible_libraries!(),
        a_tracks_row!(),
        " WHERE t.public_id = ? AND t.library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(panel.user.id)
    .bind(&id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading a track"))?;

    row.map(|row| Json(Track::from(row)))
        .ok_or(ApiError::NotFound)
}

/// All about one track
///
/// Everything the database holds about it, which is what a track's own panel draws.
/// Wider than the row a listing shows and asked for one track at a time, which is
/// the whole point of it being a second call: nobody pays for these columns fifty
/// rows at a time.
///
/// What this cannot say is what the file says. The scanner keeps the fields the
/// schema has columns for and lets the rest go by, so the credits with nowhere to be
/// kept — composer, producer, whoever engineered it — are in `/tracks/{id}/tags`,
/// which reads the file.
#[utoipa::path(
    get,
    path = "/tracks/{id}/detail",
    tag = "collection",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 200, description = "Everything known about it", body = TrackDetail),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such track, or not one you may see", body = ErrorBody),
    )
)]
pub async fn detail(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<TrackDetail>, ApiError> {
    let row: Option<DetailRow> = sqlx::query_as(concat!(
        visible_libraries!(),
        // The credit over the names where there is one, the same as the listing: the
        // panel that opens over a row must not disagree with the row it opened from.
        "SELECT t.public_id, t.title,
                coalesce(t.artist_credit,
                  (SELECT group_concat(a.name, ', ')
                     FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
                    WHERE ta.track_id = t.id AND ta.role = 'artist')) AS artists,
                al.name AS album, al.public_id AS album_id,
                (SELECT group_concat(a.name, ', ')
                   FROM album_artists aa JOIN artists a ON a.id = aa.artist_id
                  WHERE aa.album_id = al.id AND aa.role = 'albumartist') AS album_artist,
                (SELECT group_concat(g.name, ', ')
                   FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                  WHERE tg.track_id = t.id) AS genres,
                t.track_number,
                -- Counted over what is still there, like every other figure about a
                -- record: 'of 10' has to mean ten you could play.
                (SELECT count(*) FROM tracks o
                  WHERE o.album_id = al.id AND o.missing_since IS NULL) AS album_tracks,
                t.disc_number,
                -- Null rather than nought where nothing on the record numbers a
                -- disc, so 'of 1' is only ever said by a record that said it.
                (SELECT count(DISTINCT o.disc_number) FROM tracks o
                  WHERE o.album_id = al.id AND o.disc_number IS NOT NULL) AS album_discs,
                coalesce(t.year, al.year) AS year,
                t.duration_ms, t.suffix, t.bit_rate, t.sampling_rate, t.bit_depth,
                t.path, l.name AS library, t.file_size, t.updated_at,
                t.isrc, t.mbid_recording, t.comment,
                t.missing_since IS NOT NULL AS missing
           FROM tracks t
           JOIN libraries l ON l.id = t.library_id
           LEFT JOIN albums al ON al.id = t.album_id
          WHERE t.public_id = ? AND t.library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(panel.user.id)
    .bind(&id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading everything about a track"))?;

    let row = row.ok_or(ApiError::NotFound)?;

    // Who the names in that credit are. Asked apart from the rest because it is a
    // list and the row is a row — and only once the track has been found, so a
    // track nobody may see is refused before anything is looked up about it.
    //
    // In the order the file listed them, which is the order they read in.
    let credits: Vec<CreditRow> = sqlx::query_as(
        "SELECT a.public_id, a.name
           FROM tracks t
           JOIN track_artists ta ON ta.track_id = t.id AND ta.role = 'artist'
           JOIN artists a ON a.id = ta.artist_id
          WHERE t.public_id = ?
          ORDER BY ta.position",
    )
    .bind(&id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading who is credited on a track"))?;

    Ok(Json(
        row.about(credits.into_iter().map(Credit::from).collect()),
    ))
}

/// A track's tags, as the file holds them
///
/// Read from the file every time, which is what makes it worth having: it is the
/// answer the database cannot give. The credits a schema has no columns for are here,
/// and so is whatever a tagger wrote that Tocata has no use for.
///
/// Not every byte of the tag. The reader maps the frames it knows onto one set of
/// names and a vendor's own invention does not come through — so this is what could
/// be read rather than what is in there, and it does not claim to count the rest.
///
/// 404 where a track's file is gone, since there is nothing to read, and where the
/// track is one this account may not see. A track with a file and no tag at all
/// answers with an empty list rather than a refusal: nothing to say is not a failure.
#[utoipa::path(
    get,
    path = "/tracks/{id}/tags",
    tag = "collection",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 200, description = "What the file says", body = Tags),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such track, or its file is gone", body = ErrorBody),
        (status = 500, description = "The file is there and could not be read", body = ErrorBody),
    )
)]
pub async fn tags(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Tags>, ApiError> {
    let path = whereabouts(&pool, panel.user.id, &id).await?;

    // Blocking, and it reads the embedded picture: describing the cover is part of
    // the answer, and the size of it is the interesting half.
    let read = tokio::task::spawn_blocking(move || crate::scanner::read_every_tag(&path))
        .await
        .map_err(|e| ApiError::internal(e, "the tag reader gave up"))?;

    read.map(Json)
        .map_err(|e| ApiError::internal(e, "reading the tags of a file"))
}

/// A track's words
///
/// Looked for in a file beside the music first and in the file's own tag second,
/// and nowhere else — there is nothing here that asks the network.
///
/// Read on the spot rather than kept: lyrics are the one long text a music file
/// carries, and a copy in the database would be hundreds of megabytes saying what is
/// already on disk. Which also means words put on disk show up the next time
/// somebody looks, with no rescan.
///
/// Having none is not a failure. It answers 200 with no source and no lines, because
/// "there are none, and here is where they would go" is the useful answer and a 404
/// could not carry it.
#[utoipa::path(
    get,
    path = "/tracks/{id}/lyrics",
    tag = "collection",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 200, description = "The words, or that there are none", body = Lyrics),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such track, or its file is gone", body = ErrorBody),
    )
)]
pub async fn lyrics(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Lyrics>, ApiError> {
    let path = whereabouts(&pool, panel.user.id, &id).await?;

    let beside = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let found = tokio::task::spawn_blocking(move || {
        // A file beside the music wins: it is what somebody put there deliberately,
        // it can be edited without touching the music, and it is where anything
        // fetched later would be written.
        if let Some((name, words)) = crate::lyrics::find_beside(&path) {
            return Some((words, LyricSource::Beside(name)));
        }

        let read = crate::scanner::read_tags(&path).ok()?;
        let words = read.lyrics?;
        // Named where the format names it, and left unnamed where it does not:
        // "from a frame this reader has no name for" is still where they came from.
        let frame = read.lyrics_frame.unwrap_or_default().to_string();

        Some((words, LyricSource::Frame(frame)))
    })
    .await
    .map_err(|e| ApiError::internal(e, "the lyric reader gave up"))?;

    let Some((words, source)) = found else {
        return Ok(Json(Lyrics {
            source: None,
            synced: false,
            lines: Vec::new(),
            beside,
        }));
    };

    // One timed line makes the whole thing timed, which is what the LRC in the wild
    // looks like: a header of `[ar:]` and `[ti:]` that are not lines of a song, and
    // then the words.
    let synced = crate::lyrics::looks_synchronised(&words);

    let lines = if synced {
        crate::lyrics::parse(&words)
            .into_iter()
            .map(|line| LyricLine {
                at: Some(line.start),
                value: line.value,
            })
            .collect()
    } else {
        words
            .lines()
            .map(|value| LyricLine {
                at: None,
                value: value.trim_end().to_string(),
            })
            .collect()
    };

    Ok(Json(Lyrics {
        source: Some(source),
        synced,
        lines,
        beside,
    }))
}

/// Where a track's file is, for the two calls that have to open it.
///
/// Through the same finder `/rest` and the audio endpoint use, which is what keeps
/// one set of rules about who may see which library and about a stored path that
/// climbs out of one. A track whose file is gone is not found here at all, which is
/// the right answer for both callers: there is nothing to read.
async fn whereabouts(
    pool: &SqlitePool,
    who: i64,
    id: &str,
) -> Result<std::path::PathBuf, ApiError> {
    match crate::media::locate(pool, who, id).await {
        Ok(Some(track)) => Ok(track.path),
        Ok(None) | Err(crate::media::Refused::Traversal) => Err(ApiError::NotFound),
        Err(crate::media::Refused::Database(e)) => {
            Err(ApiError::internal(e, "finding a track's file"))
        }
    }
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
           LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.role = 'albumartist'
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
                  WHERE aa.album_id = al.id AND aa.role = 'albumartist') AS artist,
                al.year,
                -- Counted and summed over the same tracks, so a record cannot say
                -- it holds five and last as long as seven. Both skip what is
                -- missing: a listing that added up files nobody can play would
                -- promise more music than there is.
                (SELECT count(*) FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS tracks,
                (SELECT sum(t.duration_ms) / 1000 FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS duration,
                al.artwork_id IS NOT NULL AS cover
           FROM albums al
           LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.role = 'albumartist'
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

/// All about one record
///
/// Its figures, what it is, the running order, and who played on it — in one answer,
/// because a panel about a record is one thing to read and fetching the track list
/// apart from the rest would only mean drawing the panel twice.
///
/// A record nobody may see is a record they may not learn exists, so that is the same
/// 404 as one that is not there.
#[utoipa::path(
    get,
    path = "/albums/{id}/detail",
    tag = "collection",
    params(("id" = String, Path, description = "Which album")),
    responses(
        (status = 200, description = "Everything known about it", body = AlbumDetail),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such album, or not one you may see", body = ErrorBody),
    )
)]
pub async fn album(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<AlbumDetail>, ApiError> {
    let who = panel.user.id;

    // Figures over the tracks that are still there, except the count of the ones that
    // are not — a record missing four of its files is a thing to say out loud rather
    // than to quietly leave out of every total.
    //
    // The library and the directory come off one of its tracks. A record's files live
    // together, and where they do not the first of them is still the answer to "where
    // is this": one folder is the honest half of a fact, and a list of folders is not
    // a fact anybody asked for.
    let row: Option<AlbumRow2> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT al.public_id, al.name, al.year, al.label,
                (SELECT group_concat(a.name, ', ')
                   FROM album_artists aa JOIN artists a ON a.id = aa.artist_id
                  WHERE aa.album_id = al.id AND aa.role = 'albumartist') AS artist,
                (SELECT group_concat(DISTINCT g.name)
                   FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                   JOIN genres g ON g.id = tg.genre_id
                  WHERE t.album_id = al.id) AS genres,
                (SELECT count(*) FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS tracks,
                (SELECT count(*) FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NOT NULL) AS missing,
                (SELECT sum(t.duration_ms) / 1000 FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS duration,
                (SELECT coalesce(sum(t.file_size), 0) FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS size,
                (SELECT count(DISTINCT t.disc_number) FROM tracks t
                  WHERE t.album_id = al.id AND t.disc_number IS NOT NULL) AS discs,
                (SELECT f.path FROM tracks t JOIN folders f ON f.id = t.folder_id
                  WHERE t.album_id = al.id
                  ORDER BY t.disc_number, t.track_number LIMIT 1) AS path,
                (SELECT l.name FROM tracks t JOIN libraries l ON l.id = t.library_id
                  WHERE t.album_id = al.id LIMIT 1) AS library,
                (SELECT max(t.updated_at) FROM tracks t
                  WHERE t.album_id = al.id) AS read_at
           FROM albums al
          WHERE al.public_id = ? AND ",
        album_is_visible!("al.id")
    ))
    .bind(who)
    .bind(&id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading everything about an album"))?;

    let row = row.ok_or(ApiError::NotFound)?;

    // The running order, missing files and all: this is the one screen where a file
    // that has gone is worth showing, because it is where somebody comes to find out
    // what is gone.
    let listing: Vec<TrackOnRecord> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT t.public_id, t.title, t.track_number, t.disc_number, t.duration_ms,
                t.missing_since IS NOT NULL AS missing
           FROM tracks t JOIN albums al ON al.id = t.album_id
          WHERE al.public_id = ? AND t.library_id IN (SELECT id FROM visible_libraries)
          ORDER BY t.disc_number, t.track_number, t.title COLLATE NOCASE"
    ))
    .bind(who)
    .bind(&id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing what is on a record"))?;

    // Who it is filed under, one by one. The line above says it the way the tag
    // wrote it; this says who those names are, so the heading can lead on to them.
    let credits: Vec<CreditRow> = sqlx::query_as(
        "SELECT a.public_id, a.name
           FROM albums al
           JOIN album_artists aa ON aa.album_id = al.id AND aa.role = 'albumartist'
           JOIN artists a ON a.id = aa.artist_id
          WHERE al.public_id = ?
          ORDER BY aa.position",
    )
    .bind(&id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading who a record is filed under"))?;

    // Everybody credited on its tracks. Not the same question as who it is filed
    // under, which is the point: this is where the guests are.
    let players: Vec<CreditRow> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT DISTINCT a.public_id, a.name
           FROM tracks t
           JOIN albums al ON al.id = t.album_id
           JOIN track_artists ta ON ta.track_id = t.id AND ta.role = 'artist'
           JOIN artists a ON a.id = ta.artist_id
          WHERE al.public_id = ? AND t.library_id IN (SELECT id FROM visible_libraries)
          ORDER BY a.sort_name COLLATE NOCASE, a.name COLLATE NOCASE"
    ))
    .bind(who)
    .bind(&id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading who played on a record"))?;

    Ok(Json(AlbumDetail {
        id: row.public_id,
        name: row.name,
        artist: row.artist,
        credits: credits.into_iter().map(Credit::from).collect(),
        year: row.year,
        genres: row.genres,
        label: row.label,
        tracks: row.tracks,
        missing: row.missing,
        duration: row.duration,
        size: row.size,
        // The root of a library is the empty string, which is where a record whose
        // files sit loose at the top of one would be. Nothing to show for it.
        path: row.path.filter(|path| !path.is_empty()),
        library: row.library.unwrap_or_default(),
        read_at: row.read_at,
        // Null rather than nought where nothing on it numbers a disc, the same as a
        // track's own panel: a record that said nothing did not say one.
        discs: row.discs.filter(|discs| *discs > 0),
        listing: listing.into_iter().map(AlbumTrack::from).collect(),
        players: players.into_iter().map(Credit::from).collect(),
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
                (SELECT count(DISTINCT id) FROM (",
        records_of_artist!("a.id"),
        ")) AS albums,
                (SELECT count(*) FROM tracks t
                  WHERE ",
        track_is_theirs!("t", "a.id"),
        "    AND t.missing_since IS NULL
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

/// All about one artist
///
/// Their figures, their records, and what of theirs gets played — in one answer, like a
/// record's.
///
/// "Theirs" means every track credited to them and every track on a record they sign,
/// which is what the listing counts too, so the panel and the row that opened it
/// cannot disagree. It takes in the records they only guest on, which is the honest
/// reading of what somebody is asking when they open a name.
#[utoipa::path(
    get,
    path = "/artists/{id}/detail",
    tag = "collection",
    params(("id" = String, Path, description = "Which artist")),
    responses(
        (status = 200, description = "Everything known about them", body = ArtistDetail),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such artist, or not one you may see", body = ErrorBody),
    )
)]
pub async fn artist(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ArtistDetail>, ApiError> {
    let who = panel.user.id;

    let row: Option<ArtistRow2> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT a.id, a.public_id, a.name, a.artwork_id IS NOT NULL AS image,
                (SELECT group_concat(DISTINCT g.name)
                   FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                   JOIN genres g ON g.id = tg.genre_id
                  WHERE ",
        track_is_theirs!("t", "a.id"),
        "    AND t.library_id IN (SELECT id FROM visible_libraries)) AS genres,
                (SELECT count(DISTINCT id) FROM (",
        records_of_artist!("a.id"),
        ")) AS albums,
                (SELECT count(*) FROM tracks t
                  WHERE ",
        track_is_theirs!("t", "a.id"),
        "    AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS tracks,
                (SELECT sum(t.duration_ms) / 1000 FROM tracks t
                  WHERE ",
        track_is_theirs!("t", "a.id"),
        "    AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS duration,
                -- Summed from the per-track counts and over everybody, which is the
                -- only place it could come from: the artist stats table keeps a rating
                -- and a star and no count.
                (SELECT coalesce(sum(s.play_count), 0)
                   FROM user_track_stats s
                   JOIN tracks t ON t.id = s.track_id
                  WHERE ",
        track_is_theirs!("t", "a.id"),
        "    AND t.library_id IN (SELECT id FROM visible_libraries)) AS plays
           FROM artists a
          WHERE a.public_id = ? AND ",
        artist_is_visible!("a.id")
    ))
    .bind(who)
    .bind(&id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading everything about an artist"))?;

    let row = row.ok_or(ApiError::NotFound)?;

    // Their records, oldest first, which is how a discography reads.
    let records: Vec<RecordOfTheirs> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT al.public_id, al.name, al.year, al.artwork_id IS NOT NULL AS cover,
                (SELECT count(*) FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS tracks,
                (SELECT count(*) FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NOT NULL) AS missing,
                (SELECT sum(t.duration_ms) / 1000 FROM tracks t
                  WHERE t.album_id = al.id AND t.missing_since IS NULL) AS duration
           FROM albums al
          -- The same set the figure above these records is counted from, said
          -- once: see `records_of_artist!`.
          --
          -- Settled from her rather than asked of every record there is. Asked of
          -- each record the whole catalogue is walked and her tracks are found
          -- again for every one of them, which measured at four hundred thousand
          -- page reads against three and a half thousand records. This is two
          -- thousand and a half.
          WHERE al.id IN (",
        records_of_artist!("?"),
        ") ORDER BY al.year, al.name COLLATE NOCASE"
    ))
    .bind(who)
    .bind(row.id)
    .bind(row.id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing an artist's records"))?;

    // What of theirs actually gets played, across everybody. Nothing that has never
    // been played: a list of noughts is not a list of what gets played.
    let played_most: Vec<PlayedRow> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT t.public_id, t.title, al.name AS album, t.duration_ms,
                sum(s.play_count) AS plays
           FROM user_track_stats s
           JOIN tracks t ON t.id = s.track_id
           LEFT JOIN albums al ON al.id = t.album_id
          WHERE ",
        track_is_theirs!("t", "?"),
        "   AND t.library_id IN (SELECT id FROM visible_libraries)
          GROUP BY t.id
         HAVING plays > 0
          ORDER BY plays DESC, t.title COLLATE NOCASE
          LIMIT ",
        most_played!()
    ))
    .bind(who)
    .bind(row.id)
    .bind(row.id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading what of an artist's gets played"))?;

    // What showing their picture asks of us, where it is somebody else's work.
    // Read here rather than joined into the row above: it is one row, it is null
    // for every picture that came off the user's own disk, and the row above is
    // already carrying eight figures.
    let credit: Option<CreditRow2> = sqlx::query_as(
        "SELECT w.author, w.license, w.license_url, w.source_url
           FROM artists a JOIN artworks w ON w.id = a.artwork_id
          WHERE a.id = ? AND w.license IS NOT NULL AND w.source_url IS NOT NULL",
    )
    .bind(row.id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading what a picture asks for"))?;

    Ok(Json(ArtistDetail {
        id: row.public_id,
        name: row.name,
        genres: row.genres,
        albums: row.albums,
        tracks: row.tracks,
        duration: row.duration,
        plays: row.plays,
        image: row.image,
        credit: credit.map(Attribution::from),
        records: records.into_iter().map(ArtistAlbum::from).collect(),
        played_most: played_most.into_iter().map(PlayedTrack::from).collect(),
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

/// Which genre is being asked about.
///
/// A query parameter where everything else with a panel has a path segment, because a
/// genre is not an object: the name is the identity, and names carry slashes. "Pop/Rock"
/// is one of the commonest tags there is and it is one genre, not two segments.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Named {
    /// Spelt exactly as the listing spells it.
    pub name: String,
}

/// All about one genre
///
/// Its figures, counted over the tracks wearing it — which is where they have to come
/// from, since a genre has no row of its own to keep a count on. What it is made of is
/// not here: the tracks are a listing like any other and arrive a window at a time.
#[utoipa::path(
    get,
    path = "/genres/detail",
    tag = "collection",
    params(Named),
    responses(
        (status = 200, description = "Everything known about it", body = GenreDetail),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such genre, or nothing in it you may see", body = ErrorBody),
    )
)]
pub async fn genre(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Query(which): Query<Named>,
) -> Result<Json<GenreDetail>, ApiError> {
    let who = panel.user.id;

    let row: Option<GenreRow2> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT g.name,
                (SELECT count(DISTINCT t.album_id) FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NULL
                    AND t.album_id IS NOT NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS albums,
                (SELECT count(*) FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS tracks,
                (SELECT count(*) FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NOT NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS missing,
                (SELECT count(DISTINCT ta.artist_id) FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                   JOIN track_artists ta ON ta.track_id = t.id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS artists,
                (SELECT sum(t.duration_ms) / 1000 FROM tracks t
                   JOIN track_genres tg ON tg.track_id = t.id
                  WHERE tg.genre_id = g.id AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS duration,
                (SELECT coalesce(sum(s.play_count), 0)
                   FROM user_track_stats s
                   JOIN tracks t ON t.id = s.track_id
                   JOIN track_genres tg ON tg.track_id = t.id
                  WHERE tg.genre_id = g.id
                    AND t.library_id IN (SELECT id FROM visible_libraries)) AS plays
           FROM genres g
          WHERE g.name = ? AND ",
        has_a_visible_track!("JOIN track_genres tg ON tg.track_id = t.id WHERE tg.genre_id = g.id")
    ))
    .bind(who)
    .bind(&which.name)
    .fetch_optional(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading everything about a genre"))?;

    let row = row.ok_or(ApiError::NotFound)?;

    Ok(Json(GenreDetail {
        name: row.name,
        albums: row.albums,
        tracks: row.tracks,
        missing: row.missing,
        artists: row.artists,
        duration: row.duration,
        plays: row.plays,
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

/// What the panel announces itself as when it says something is playing.
///
/// A client's name, in the same column every OpenSubsonic client writes its own
/// into, because that is what it is: a player belonging to one account. It means the
/// panel shows up in `getNowPlaying` beside the phones, which is right — somebody
/// listening in the panel is somebody listening.
const PANEL_AS_A_PLAYER: &str = "Tocata panel";

/// Say a song has started
///
/// The other half of counting a play, and a different claim: this one is about the
/// present. It puts the song in what is playing now — where every other client's
/// does — and tells whoever this account scrobbles to that it has started.
///
/// Nothing is counted here. A song that starts and is skipped after ten seconds was
/// announced and never played, which is exactly the difference the two calls keep.
///
/// Answers before the announcement has been delivered anywhere. What is being waited
/// on is this server writing a row, not somebody else's website taking a request that
/// no reply here depends on.
#[utoipa::path(
    post,
    path = "/tracks/{id}/playing",
    tag = "collection",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 204, description = "Noted, and passed on if there is anywhere to pass it"),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn playing(
    panel: Panel,
    State(pool): State<SqlitePool>,
    State(net): State<crate::net::Net>,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    let started = crate::plays::record_now_playing(
        &pool,
        panel.user.id,
        PANEL_AS_A_PLAYER,
        &id,
        &crate::db::now(),
    )
    .await
    .map_err(|e| ApiError::internal(e, "noting what is playing"))?;

    if let Some(track_id) = started {
        let user_id = panel.user.id;

        tokio::spawn(async move {
            crate::scrobble::announce(&net, &pool, user_id, track_id).await;
        });
    }

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
    if let Some(ids) = filter.named() {
        builder.push(" AND t.public_id IN (");

        let mut named = builder.separated(", ");
        for id in ids {
            named.push_bind(id);
        }

        builder.push(")");
    }

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
    track_number: Option<i64>,
    duration_ms: Option<i64>,
    suffix: String,
    bit_rate: Option<i64>,
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
            track_number: row.track_number,
            // Milliseconds in the row and seconds in the answer, like every other
            // length this API reports.
            duration: row.duration_ms.map(|ms| ms / 1000),
            suffix: row.suffix,
            bit_rate: row.bit_rate,
            missing: row.missing,
        }
    }
}

#[derive(sqlx::FromRow)]
struct DetailRow {
    public_id: String,
    title: String,
    artists: Option<String>,
    album: Option<String>,
    album_id: Option<String>,
    album_artist: Option<String>,
    genres: Option<String>,
    track_number: Option<i64>,
    album_tracks: Option<i64>,
    disc_number: Option<i64>,
    album_discs: Option<i64>,
    year: Option<i64>,
    duration_ms: Option<i64>,
    suffix: String,
    bit_rate: Option<i64>,
    sampling_rate: Option<i64>,
    bit_depth: Option<i64>,
    path: String,
    library: String,
    file_size: i64,
    updated_at: String,
    isrc: Option<String>,
    mbid_recording: Option<String>,
    comment: Option<String>,
    missing: bool,
}

#[derive(sqlx::FromRow)]
struct CreditRow {
    public_id: String,
    name: String,
}

/// What a fetched picture asks for, as the artwork row holds it. Its own struct
/// rather than four columns on the artist's, which is already wide and is read
/// whether or not there is a picture at all.
#[derive(sqlx::FromRow)]
struct CreditRow2 {
    author: Option<String>,
    license: String,
    license_url: Option<String>,
    source_url: String,
}

impl From<CreditRow2> for Attribution {
    fn from(row: CreditRow2) -> Self {
        Self {
            author: row.author,
            license: row.license,
            license_url: row.license_url,
            source_url: row.source_url,
        }
    }
}

impl From<CreditRow> for Credit {
    fn from(row: CreditRow) -> Self {
        Self {
            id: row.public_id,
            name: row.name,
        }
    }
}

impl DetailRow {
    /// Everything the row says, and the names the row could not carry: a list does
    /// not fit in a column, so the credits are read separately and joined on here.
    fn about(self, credits: Vec<Credit>) -> TrackDetail {
        TrackDetail {
            id: self.public_id,
            title: self.title,
            artists: self.artists,
            credits,
            album: self.album,
            album_id: self.album_id,
            album_artist: self.album_artist,
            genres: self.genres,
            track_number: self.track_number,
            // Counted with a subquery, so a track with no record at all comes back
            // as nought rather than as null. Nought records hold nothing, and "of 0"
            // is not a thing to print.
            album_tracks: self.album_tracks.filter(|count| *count > 0),
            disc_number: self.disc_number,
            album_discs: self.album_discs.filter(|count| *count > 0),
            year: self.year,
            duration: self.duration_ms.map(|ms| ms / 1000),
            suffix: self.suffix,
            bit_rate: self.bit_rate,
            sampling_rate: self.sampling_rate,
            bit_depth: self.bit_depth,
            path: self.path,
            library: self.library,
            size: self.file_size,
            read_at: self.updated_at,
            isrc: self.isrc,
            mbid_recording: self.mbid_recording,
            comment: self.comment,
            missing: self.missing,
        }
    }
}

/// A record's own panel, as one row of figures. Its own struct rather than a wider
/// `AlbumRow`, because that one is read fifty at a time by the shelf and this one is
/// read once.
#[derive(sqlx::FromRow)]
struct AlbumRow2 {
    public_id: String,
    name: String,
    artist: Option<String>,
    year: Option<i64>,
    label: Option<String>,
    genres: Option<String>,
    tracks: i64,
    missing: i64,
    duration: Option<i64>,
    size: i64,
    discs: Option<i64>,
    path: Option<String>,
    library: Option<String>,
    read_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct TrackOnRecord {
    public_id: String,
    title: String,
    track_number: Option<i64>,
    disc_number: Option<i64>,
    duration_ms: Option<i64>,
    missing: bool,
}

impl From<TrackOnRecord> for AlbumTrack {
    fn from(row: TrackOnRecord) -> Self {
        Self {
            id: row.public_id,
            title: row.title,
            track_number: row.track_number,
            disc_number: row.disc_number,
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
    duration: Option<i64>,
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
            duration: row.duration,
            cover: row.cover,
        }
    }
}

/// An artist's own panel, as one row of figures. Its own struct for the same reason a
/// record's is: the listing's row is read fifty at a time and this one is read once.
#[derive(sqlx::FromRow)]
struct ArtistRow2 {
    /// The row's own, which the two statements after it are narrowed by. Read here
    /// rather than looked up again because the public id would have to be bound
    /// twice on each of them.
    id: i64,
    public_id: String,
    name: String,
    genres: Option<String>,
    albums: i64,
    tracks: i64,
    duration: Option<i64>,
    plays: i64,
    image: bool,
}

#[derive(sqlx::FromRow)]
struct RecordOfTheirs {
    public_id: String,
    name: String,
    year: Option<i64>,
    tracks: i64,
    missing: i64,
    duration: Option<i64>,
    cover: bool,
}

impl From<RecordOfTheirs> for ArtistAlbum {
    fn from(row: RecordOfTheirs) -> Self {
        Self {
            id: row.public_id,
            name: row.name,
            year: row.year,
            tracks: row.tracks,
            missing: row.missing,
            duration: row.duration,
            cover: row.cover,
        }
    }
}

#[derive(sqlx::FromRow)]
struct PlayedRow {
    public_id: String,
    title: String,
    album: Option<String>,
    duration_ms: Option<i64>,
    plays: i64,
}

impl From<PlayedRow> for PlayedTrack {
    fn from(row: PlayedRow) -> Self {
        Self {
            id: row.public_id,
            title: row.title,
            album: row.album,
            plays: row.plays,
            duration: row.duration_ms.map(|ms| ms / 1000),
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

#[derive(sqlx::FromRow)]
struct GenreRow2 {
    name: String,
    albums: i64,
    tracks: i64,
    missing: i64,
    artists: i64,
    duration: Option<i64>,
    plays: i64,
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
                "INSERT INTO albums (id, public_id, grouping_key, name, year, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 1975, ?, ?)",
            )
            .bind(id)
            .bind(format!("al{id}"))
            .bind(name.to_lowercase())
            .bind(name)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO album_artists (album_id, artist_id, role) VALUES (?, 1, 'albumartist')",
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
                     VALUES (?, 1, 'artist', 0)",
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

    /// A second account, walled off from the second library.
    ///
    /// Its own name because `somebody` always inserts `ana`, and a test that wants
    /// both a restricted account and an unrestricted one in the same collection
    /// cannot have them both be her.
    async fn somebody_else(pool: &SqlitePool) -> Panel {
        let at = db::now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('bea', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&at)
        .bind(&at)
        .fetch_one(pool)
        .await
        .unwrap();

        sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, 1)")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();

        Panel {
            id: 2,
            user: User {
                id,
                username: "bea".to_string(),
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

    /// One track whose file is really on disk, in a library rooted at a temporary
    /// directory of its own.
    ///
    /// The two calls that read a file cannot be tested against a database alone, and
    /// what they are for is exactly what a database cannot answer. `name` keeps each
    /// test's directory to itself, since these run at the same time as each other.
    async fn a_track_on_disk(
        name: &str,
        tags: &[(lofty::prelude::ItemKey, &str)],
    ) -> (SqlitePool, Panel, std::path::PathBuf) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let root = crate::fixtures::temp_root(&format!("api-{name}"));
        let file = root.join("song.wav");
        crate::fixtures::write_wav(&file);

        // ID3v2, because that is the tag a RIFF container will hold — and because it
        // is the one whose lyric frame the reader used to miss.
        if !tags.is_empty() {
            crate::fixtures::tag_file(&file, lofty::tag::TagType::Id3v2, tags);
        }

        let at = db::now();
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'kept', ?, ?, ?)",
        )
        .bind(root.to_string_lossy().as_ref())
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'root', '', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                 file_modified_at, content_type, suffix, title,
                                 last_seen_scan, created_at, updated_at)
             VALUES (1, 't1', 1, 1, 'song.wav', 44, ?, 'audio/wav', 'wav', 'Song', 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let who = somebody(&pool, false).await;
        (pool, who, file)
    }

    /// The panel of one track says everything the row could not, and stops at the
    /// same wall.
    #[tokio::test]
    async fn all_about_a_track_is_wider_than_its_row_and_no_less_walled() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;

        let Json(track) = detail(hers, State(pool.clone()), UrlPath("t10".to_string()))
            .await
            .unwrap();

        assert_eq!(track.title, "El Patio 0");
        assert_eq!(track.artists.as_deref(), Some("Triana"), "who played on it");
        assert_eq!(
            track.album_artist.as_deref(),
            Some("Triana"),
            "and who the record is filed under, which the row never carried"
        );
        assert_eq!(track.genres.as_deref(), Some("Flamenco"));
        assert_eq!(track.library, "kept");
        assert_eq!(track.size, 1);
        assert_eq!(
            track.year,
            Some(1975),
            "off the record, since the file said none"
        );

        // Two tracks on this record and one of them is gone, so "of 1" is the honest
        // reading: a figure over what could be played rather than over what was once
        // filed.
        assert_eq!(track.album_tracks, Some(1));
        assert_eq!(track.disc_number, Some(1));
        assert_eq!(track.album_discs, Some(1));

        // And the wall is the same wall. A track in a library this account may not
        // see is not a track it may learn exists.
        let walled = somebody_else(&pool).await;
        assert!(
            matches!(
                detail(walled, State(pool.clone()), UrlPath("t20".to_string())).await,
                Err(ApiError::NotFound)
            ),
            "the second library is not hers to read about either"
        );
    }

    /// A credit is a sentence, and this says who the names in it are.
    ///
    /// Both halves matter. The sentence goes out exactly as the tagger wrote it,
    /// because "feat." is a fact about who did what and joining the names with a
    /// comma would throw it away — and the names go out with their identifiers,
    /// because a line of words is not something a panel can open.
    #[tokio::test]
    async fn the_names_in_a_credit_come_with_somewhere_to_go() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;
        let at = db::now();

        // A guest, credited first, and the tagger's own sentence about the two of
        // them. Credited first while carrying the later identifier, so the order
        // this answers in can only have come from the file.
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (2, 'ar2', 'Zoë Johnston', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role, position)
             VALUES (10, 2, 'artist', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE track_artists SET position = 1 WHERE track_id = 10 AND artist_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tracks SET artist_credit = 'Zoë Johnston with Triana' WHERE id = 10")
            .execute(&pool)
            .await
            .unwrap();

        let Json(track) = detail(hers, State(pool.clone()), UrlPath("t10".to_string()))
            .await
            .unwrap();

        assert_eq!(
            track.artists.as_deref(),
            Some("Zoë Johnston with Triana"),
            "the sentence the tagger wrote, word for word"
        );
        assert_eq!(
            track
                .credits
                .iter()
                .map(|who| (who.id.as_str(), who.name.as_str()))
                .collect::<Vec<_>>(),
            vec![("ar2", "Zoë Johnston"), ("ar1", "Triana")],
            "who those names are, in the order the file listed them"
        );

        // The album is already an identifier and always was. Named here because the
        // heading leads on to it beside the artists, and the two are one answer.
        assert_eq!(track.album_id.as_deref(), Some("al1"));
    }

    /// Nobody credited is an empty list rather than a list with nothing in it worth
    /// drawing: a file that names no artist has no name in its heading to press.
    #[tokio::test]
    async fn a_track_nobody_is_credited_on_leads_nowhere() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;

        sqlx::query("DELETE FROM track_artists WHERE track_id = 10")
            .execute(&pool)
            .await
            .unwrap();

        let Json(track) = detail(hers, State(pool.clone()), UrlPath("t10".to_string()))
            .await
            .unwrap();

        assert_eq!(track.artists, None);
        assert!(track.credits.is_empty());
    }

    /// A picture that came off Commons carries what showing it asks for; one read
    /// out of the user's own files carries nothing, because it is nobody's to
    /// attribute.
    #[tokio::test]
    async fn a_fetched_picture_says_who_to_credit_and_a_local_one_does_not() {
        let pool = a_collection().await;
        let at = db::now();

        // First the one off their own disk, which is the ordinary case.
        sqlx::query(
            "INSERT INTO artworks (id, public_id, kind, source, mime_type, content_hash, fetched_at)
             VALUES (1, 'w1', 'artist', 'local_file', 'image/jpeg', 'aa', ?)",
        )
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE artists SET artwork_id = 1 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        let hers = somebody(&pool, false).await;
        let Json(read) = artist(hers, State(pool.clone()), UrlPath("ar1".to_string()))
            .await
            .unwrap();

        assert!(read.image, "there is a picture");
        assert_eq!(
            read.credit, None,
            "and nothing to say about it: it came off their own disk"
        );

        // Now one that did not.
        sqlx::query(
            "INSERT INTO artworks (id, public_id, kind, source, source_ref, mime_type,
                                   content_hash, fetched_at, author, license, license_url,
                                   source_url)
             VALUES (2, 'w2', 'artist', 'commons', 'File:Triana.jpg', 'image/jpeg', 'bb', ?,
                     'Someone', 'CC BY-SA 4.0', 'https://creativecommons.org/licenses/by-sa/4.0',
                     'https://commons.wikimedia.org/wiki/File:Triana.jpg')",
        )
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE artists SET artwork_id = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        let hers = somebody_else(&pool).await;
        let Json(read) = artist(hers, State(pool.clone()), UrlPath("ar1".to_string()))
            .await
            .unwrap();

        let credit = read.credit.expect("somebody else's work says so");
        assert_eq!(credit.author.as_deref(), Some("Someone"));
        assert_eq!(credit.license, "CC BY-SA 4.0");
        assert!(credit.source_url.contains("commons.wikimedia.org"));
    }

    /// The two questions a record's panel asks about people, kept apart and both
    /// answered with something a panel can open.
    ///
    /// Who it is filed under is the heading. Who played on it is the list at the
    /// foot, and on a compilation those are almost disjoint — a guest belongs in the
    /// second and nowhere near the first.
    #[tokio::test]
    async fn who_signs_a_record_and_who_plays_on_it_both_open() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;
        let at = db::now();

        // A guest on one of its tracks, credited nowhere on the record itself.
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, sort_name, created_at, updated_at)
             VALUES (2, 'ar2', 'Lole', 'Lole', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role, position)
             VALUES (10, 2, 'artist', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let Json(read) = album(hers, State(pool.clone()), UrlPath("al1".to_string()))
            .await
            .unwrap();

        assert_eq!(
            read.credits
                .iter()
                .map(|who| who.id.as_str())
                .collect::<Vec<_>>(),
            vec!["ar1"],
            "the guest did not sign the record, so the heading does not name her"
        );
        assert_eq!(
            read.players
                .iter()
                .map(|who| (who.id.as_str(), who.name.as_str()))
                .collect::<Vec<_>>(),
            vec![("ar2", "Lole"), ("ar1", "Triana")],
            "and the foot names everybody on its tracks, each with a way in"
        );
    }

    /// A record's own panel, and the two figures that have to be kept apart.
    #[tokio::test]
    async fn a_record_counts_what_is_there_and_says_what_is_not() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;

        let Json(read) = album(hers, State(pool.clone()), UrlPath("al1".to_string()))
            .await
            .unwrap();

        assert_eq!(read.name, "El Patio");
        assert_eq!(read.artist.as_deref(), Some("Triana"));
        assert_eq!(read.genres.as_deref(), Some("Flamenco"));
        assert_eq!(read.year, Some(1975));
        assert_eq!(read.library, "kept");

        // The fixture's first record has two tracks and one of them has gone. The
        // figures a panel prints are over what can be played; the one that has gone
        // is counted on its own, because a record missing a file is worth saying out
        // loud rather than quietly leaving out of every total.
        assert_eq!(read.tracks, 1);
        assert_eq!(read.missing, 1);
        assert_eq!(read.duration, Some(180));

        // And the running order shows both, which is the one place a file that has
        // gone belongs: this is where somebody comes to find out what is gone.
        assert_eq!(read.listing.len(), 2);
        assert_eq!(read.listing.iter().filter(|t| t.missing).count(), 1);

        assert_eq!(
            read.players,
            vec![Credit {
                id: "ar1".to_string(),
                name: "Triana".to_string(),
            }],
            "credited on its tracks, which is a different question from who it is \
             filed under — and named with what it takes to open them"
        );

        // And who it is filed under, which is the other of the two questions.
        assert_eq!(read.artist.as_deref(), Some("Triana"));
        assert_eq!(
            read.credits
                .iter()
                .map(|who| who.id.as_str())
                .collect::<Vec<_>>(),
            vec!["ar1"]
        );

        let walled = somebody_else(&pool).await;
        assert!(
            matches!(
                album(walled, State(pool.clone()), UrlPath("al2".to_string())).await,
                Err(ApiError::NotFound)
            ),
            "a record in a library she may not see is not one she may learn exists"
        );
    }

    /// An artist's own panel, and the figure that has nowhere of its own to live.
    #[tokio::test]
    async fn an_artist_adds_up_their_plays_from_the_songs() {
        let pool = a_collection().await;

        // Two plays of one of her tracks and one of another, by two different accounts,
        // because an artist's total is across everybody who listens here.
        let hers = somebody(&pool, false).await;
        let theirs = somebody_else(&pool).await;

        for (account, track, plays) in [(&hers, 10, 2), (&theirs, 10, 1), (&hers, 11, 4)] {
            sqlx::query(
                "INSERT INTO user_track_stats (user_id, track_id, play_count)
                 VALUES (?, ?, ?)",
            )
            .bind(account.user.id)
            .bind(track)
            .bind(plays)
            .execute(&pool)
            .await
            .unwrap();
        }

        let Json(read) = artist(
            again(&hers),
            State(pool.clone()),
            UrlPath("ar1".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(read.name, "Triana");
        assert_eq!(
            read.plays, 7,
            "summed over every account and every song of theirs, because the artist \
             stats table keeps a rating and a star and no count"
        );

        // The figures agree with the listing's, which count what is still there. She is
        // not walled off from anything, so that is all four of the fixture's tracks
        // less the one whose file has gone.
        assert_eq!(read.tracks, 3);
        assert_eq!(read.albums, 2);

        assert_eq!(
            read.played_most.len(),
            2,
            "only what has been played: a list of noughts is not a list of what gets \
             played"
        );
        assert_eq!(read.played_most[0].plays, 4, "the most played first");
        assert_eq!(read.played_most[1].plays, 3);

        assert_eq!(read.records.len(), 2, "both of the records she is on");
        assert_eq!(
            read.records[0].missing, 1,
            "and one of them has a hole in it"
        );
    }

    /// A discography stops at the wall, like everything else does.
    ///
    /// The statement behind it gathers her records from her rather than asking
    /// every record in the catalogue whether it is hers, and the visibility
    /// condition rides along inside that set. Which is exactly where it is easy
    /// to drop while rearranging the statement, and dropping it is silent: the
    /// panel simply lists one record more than it should, named and dated, from
    /// a library this account was walled off from.
    #[tokio::test]
    async fn an_artists_records_stop_at_the_wall() {
        let pool = a_collection().await;

        let titles = |records: &[ArtistAlbum]| {
            let mut names: Vec<String> = records.iter().map(|r| r.name.clone()).collect();
            names.sort();
            names
        };

        let Json(everything) = artist(
            somebody(&pool, false).await,
            State(pool.clone()),
            UrlPath("ar1".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(
            titles(&everything.records),
            vec!["El Patio".to_string(), "Hijos del Agobio".to_string()],
            "she signs both, and both are hers to see"
        );

        let Json(walled) = artist(
            somebody_else(&pool).await,
            State(pool.clone()),
            UrlPath("ar1".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(
            titles(&walled.records),
            vec!["El Patio".to_string()],
            "the record in the library she may not open is not one she may learn \
             the name of"
        );

        assert_eq!(
            (everything.albums, walled.albums),
            (2, 1),
            "and the figure moves with the list it stands for, for each of them"
        );
    }

    /// A genre's figures are the tracks wearing it, counted through the wall.
    ///
    /// Every one of them is asked of that set rather than read off a row, so the wall
    /// has to be written into each of them separately — which is exactly the shape of
    /// mistake where one figure is left counting the whole collection and nothing on
    /// screen says which.
    #[tokio::test]
    async fn a_genres_figures_stop_at_the_wall() {
        let pool = a_collection().await;

        // A second name, credited only in the library behind the wall, so that the
        // count of names has something to lose when the wall is up.
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, sort_name, created_at, updated_at)
             VALUES (2, 'ar2', 'Lole y Manuel', 'Lole y Manuel', ?, ?)",
        )
        .bind(db::now())
        .bind(db::now())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role, position)
             VALUES (20, 2, 'artist', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let asked = |who: Panel| {
            let pool = pool.clone();
            async move {
                genre(
                    who,
                    State(pool),
                    Query(Named {
                        name: "Flamenco".to_string(),
                    }),
                )
                .await
                .unwrap()
                .0
            }
        };

        let everything = asked(somebody(&pool, false).await).await;

        assert_eq!(
            (everything.tracks, everything.albums, everything.artists),
            (3, 2, 2),
            "four tracks wear it, one of them has gone"
        );
        assert_eq!(everything.missing, 1, "and the one that has gone is said");
        assert_eq!(
            everything.duration,
            Some(540),
            "three minutes each, over the three that are there"
        );

        let walled = asked(somebody_else(&pool).await).await;

        assert_eq!(
            (walled.tracks, walled.albums, walled.artists),
            (1, 1, 1),
            "the library she may not open counts for none of it"
        );
        assert_eq!(
            walled.missing, 1,
            "and the file that has gone is in the library she may see"
        );
        assert_eq!(walled.duration, Some(180));
    }

    /// A genre nothing visible wears is a genre that is not there.
    ///
    /// Not an empty answer: a panel about nothing would be a panel, and the row that
    /// could have opened it is not in the listing either — the listing and this ask
    /// the same question about what may be seen.
    #[tokio::test]
    async fn a_genre_with_nothing_to_show_is_not_found() {
        let pool = a_collection().await;

        // A genre in the walled library and nowhere else.
        sqlx::query("INSERT INTO genres (id, name) VALUES (2, 'Rumba')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO track_genres (track_id, genre_id) VALUES (20, 2)")
            .execute(&pool)
            .await
            .unwrap();

        let read = genre(
            somebody_else(&pool).await,
            State(pool.clone()),
            Query(Named {
                name: "Rumba".to_string(),
            }),
        )
        .await;

        assert!(
            matches!(read, Err(ApiError::NotFound)),
            "behind the wall is not there"
        );

        // And a name nobody has ever used.
        let never = genre(
            somebody(&pool, false).await,
            State(pool.clone()),
            Query(Named {
                name: "Bakalao".to_string(),
            }),
        )
        .await;

        assert!(matches!(never, Err(ApiError::NotFound)));
    }

    /// The figure above an artist's records counts those records, whatever became
    /// of their files.
    ///
    /// A disk that fails to mount is the case this is really about, and it used to
    /// read badly: the count asked that the files still be there and the list did
    /// not, so the panel printed "1 record" over two of them, and with the whole
    /// collection away it printed none over all of them. They are one expression
    /// now, and this is what says so — a count and a list written apart drift
    /// quietly, and it is the client who ends up looking wrong.
    ///
    /// The listing that opened the panel counts the same set too, so the row and
    /// the page it leads to cannot disagree either.
    #[tokio::test]
    async fn the_count_of_an_artists_records_survives_the_files_going_away() {
        let pool = a_collection().await;

        // Every file of the second record goes away, as an unmounted disk does it:
        // the rows stay, the files do not.
        sqlx::query("UPDATE tracks SET missing_since = ? WHERE album_id = 2")
            .bind(db::now())
            .execute(&pool)
            .await
            .unwrap();

        let Json(read) = artist(
            somebody(&pool, false).await,
            State(pool.clone()),
            UrlPath("ar1".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(
            read.records.len(),
            2,
            "a record whose files are away is exactly what this panel is for"
        );
        assert_eq!(read.albums, 2, "so the figure above them says two, not one");

        let Json(listed) = artists(
            somebody_else(&pool).await,
            State(pool.clone()),
            Query(Filter::default()),
            all_of_it(),
        )
        .await
        .unwrap();

        let her = listed
            .artists
            .iter()
            .find(|a| a.name == "Triana")
            .expect("she is on the listing");

        assert_eq!(
            her.albums, 1,
            "the row that opens the panel counts what the panel will show, and \
             this account may open only one of the two libraries"
        );
    }

    /// Where the file is, said without saying where the library is.
    ///
    /// The path is what the scanner stored, which is relative to the library root, and
    /// the library is named beside it. That is enough to find one file among the
    /// others and to tell two copies apart; where that library is mounted on the
    /// machine is nobody's business but the person who mounted it, and everybody who
    /// may see a library can read this.
    #[tokio::test]
    async fn a_path_locates_a_file_in_its_library_and_not_on_the_machine() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;

        let Json(track) = detail(hers, State(pool.clone()), UrlPath("t10".to_string()))
            .await
            .unwrap();

        assert_eq!(track.path, "10.flac");
        assert!(
            !track.path.starts_with('/') && !track.path.contains("kept"),
            "the library's own root has no business being in here: {}",
            track.path
        );
    }

    /// The whole reason the tab exists: credits the schema has no columns for.
    #[tokio::test]
    async fn the_tags_of_a_file_say_what_the_database_cannot() {
        use lofty::prelude::ItemKey;

        let (pool, who, _) = a_track_on_disk(
            "tags",
            &[
                (ItemKey::TrackTitle, "Abre la Puerta"),
                (ItemKey::Composer, "Jesús de la Rosa"),
                (ItemKey::Label, "Movieplay"),
                // One frame in the file and two keys to the reader, which is exactly
                // what must not come back as two rows.
                (ItemKey::TrackNumber, "1"),
                (ItemKey::TrackTotal, "5"),
                // Written and not expected back. lofty has no ID3v2 mapping for a
                // producer — ID3v2 keeps that sort of credit in the involved people
                // list, which it does not take apart — so this one is invisible in
                // an MP3 and plain in Vorbis comments. Worth pinning down here so
                // that "the producer is missing" is a known shape of this format
                // rather than a bug somebody goes looking for.
                (ItemKey::Producer, "Gonzalo García Pelayo"),
            ],
        )
        .await;

        let Json(tags) = tags(who, State(pool.clone()), UrlPath("t1".to_string()))
            .await
            .unwrap();

        assert_eq!(tags.kind.as_deref(), Some("ID3v2"));

        let said = |name: &str| {
            tags.tags
                .iter()
                .find(|tag| tag.name == name)
                .map(|tag| tag.value.clone())
        };

        // Named as ID3v2 names them, not as Tocata does.
        assert_eq!(said("TIT2").as_deref(), Some("Abre la Puerta"));
        assert_eq!(
            said("TCOM").as_deref(),
            Some("Jesús de la Rosa"),
            "the composer, which has no column anywhere in the schema"
        );
        assert_eq!(
            said("TPUB").as_deref(),
            Some("Movieplay"),
            "and the label, which has none either"
        );
        // Put back together as the file holds it. Listed as the reader hands them over
        // it would be `TRCK` twice, once saying 1 and once saying 5, which is a list
        // disagreeing with the file it claims to be reading.
        assert_eq!(said("TRCK").as_deref(), Some("1/5"));

        assert_eq!(
            tags.tags.len(),
            4,
            "and nothing invented: what this format cannot carry does not appear, \
             which is why this cannot claim to be every byte of the tag"
        );
    }

    /// A file that is not there cannot be read, and a track that is not yours cannot
    /// be asked about. Both are the same answer.
    #[tokio::test]
    async fn there_is_nothing_to_read_where_there_is_no_file() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;
        let again_hers = again(&hers);

        // t11 is the one the fixture marks as gone. Its row is still in the listing,
        // which is the point of marking rather than deleting — but there is no file
        // behind it to open.
        assert!(matches!(
            tags(hers, State(pool.clone()), UrlPath("t11".to_string())).await,
            Err(ApiError::NotFound)
        ));
        assert!(matches!(
            lyrics(again_hers, State(pool.clone()), UrlPath("t11".to_string())).await,
            Err(ApiError::NotFound)
        ));
    }

    /// The regression this pass fixed, at the level it is served from: lofty has no
    /// ID3v2 mapping for `Lyrics`, so words in an MP3 were invisible — here and over
    /// `/rest` — and nothing said so, because no words and unreachable words answer
    /// alike.
    #[tokio::test]
    async fn words_in_an_id3_frame_are_found_and_the_frame_is_named() {
        use lofty::prelude::ItemKey;

        let (pool, who, _) = a_track_on_disk(
            "id3-words",
            &[(ItemKey::UnsyncLyrics, "Sé que no puedo\nvolver atrás")],
        )
        .await;

        let Json(words) = lyrics(who, State(pool.clone()), UrlPath("t1".to_string()))
            .await
            .unwrap();

        assert_eq!(words.source, Some(LyricSource::Frame("USLT".to_string())));
        assert!(!words.synced, "no timings in there");
        assert_eq!(words.lines.len(), 2);
        assert_eq!(words.lines[0].value, "Sé que no puedo");
        assert_eq!(words.lines[0].at, None, "untimed lines carry no place");
    }

    /// A file beside the music wins over the tag inside it, and says which it was.
    ///
    /// Which of the two the words came from is the point of showing them in an
    /// administration panel: a song whose sidecar says one thing while its tag says
    /// another is a thing somebody may want to know about.
    #[tokio::test]
    async fn words_beside_the_music_win_over_the_tag_and_bring_their_timings() {
        use lofty::prelude::ItemKey;

        let (pool, who, file) =
            a_track_on_disk("beside", &[(ItemKey::UnsyncLyrics, "what the tag says")]).await;

        std::fs::write(
            file.with_extension("lrc"),
            "[ar:Triana]\n[00:12.50]Abre la puerta\n[00:19.00]niña\n",
        )
        .unwrap();

        let Json(words) = lyrics(who, State(pool.clone()), UrlPath("t1".to_string()))
            .await
            .unwrap();

        assert_eq!(
            words.source,
            Some(LyricSource::Beside("song.lrc".to_string())),
            "the file beside it, by name"
        );
        assert!(words.synced);
        assert_eq!(
            words.lines.len(),
            2,
            "the LRC header is not a line of the song"
        );
        assert_eq!(words.lines[0].at, Some(12_500));
        assert_eq!(words.lines[0].value, "Abre la puerta");
    }

    /// Having no words is an answer, and it carries the one useful thing to say.
    #[tokio::test]
    async fn a_song_with_no_words_says_where_they_would_go() {
        let (pool, who, _) = a_track_on_disk("wordless", &[]).await;

        let Json(words) = lyrics(who, State(pool.clone()), UrlPath("t1".to_string()))
            .await
            .unwrap();

        assert_eq!(words.source, None);
        assert!(words.lines.is_empty());
        assert_eq!(
            words.beside, "song",
            "the name a file beside the music would have to carry"
        );
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

    /// One track on its own reads the same as its row in the listing, and stays
    /// behind the same wall.
    ///
    /// This is what a player asks for when its queue steps onto a track that was
    /// never on screen, so the two answers have to be the same answer — hence one
    /// statement shared between them and this comparing the two rather than
    /// checking the fields by hand.
    ///
    /// The wall matters more here than in the listing. A listing narrows to what
    /// somebody may see, so a restricted account never learns the identifier of
    /// anything else; but an identifier that leaked some other way would be a way
    /// to read a title out of a library that account was walled off from.
    #[tokio::test]
    async fn one_track_reads_the_same_as_its_row_and_stays_behind_the_same_wall() {
        let pool = a_collection().await;
        let ana = somebody(&pool, false).await;
        let ana_again = again(&ana);

        let Json(listed) = tracks(ana, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        let row = listed
            .tracks
            .iter()
            .find(|track| track.album.as_deref() == Some("Hijos del Agobio"))
            .expect("the fixture has one in the second library");

        let Json(alone) = track(ana_again, State(pool.clone()), UrlPath(row.id.clone()))
            .await
            .unwrap();

        assert_eq!(&alone, row, "asked for on its own it is the same track");

        // The same identifier, asked for by somebody walled off from that library.
        let walled = somebody_else(&pool).await;
        let refused = track(walled, State(pool.clone()), UrlPath(row.id.clone())).await;

        assert!(
            matches!(refused, Err(ApiError::NotFound)),
            "a track in a library you may not see is not a track you may know exists"
        );
    }

    /// A guest on a track is read the way the record credits them, and everywhere
    /// the track is read.
    ///
    /// "Above & Beyond feat. Zoë Johnston" is the file's own sentence and the two
    /// names joined by a comma are two equals: the word between them is the fact,
    /// and nothing joining a list back together can put it back. So the credit wins
    /// where there is one — and where there is not, which is most tracks, the names
    /// answer exactly as they did.
    ///
    /// Both readings are checked because they are two statements in two places, and
    /// a row that disagreed with the panel opened over it would be the same song
    /// under two names.
    #[tokio::test]
    async fn a_credit_is_read_over_the_names_wherever_the_track_is_read() {
        let pool = a_collection().await;
        let at = db::now();

        sqlx::query(
            "INSERT INTO artists (id, public_id, name, sort_name, created_at, updated_at)
             VALUES (2, 'ar2', 'Zoë Johnston', 'Johnston, Zoë', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        // The first track of the first record gains a guest and the sentence about
        // her. Everything else on the fixture keeps its one name and no credit,
        // which is the other half of what this checks.
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role, position)
             VALUES (10, 2, 'artist', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE tracks SET artist_credit = ? WHERE id = 10")
            .bind("Triana feat. Zoë Johnston")
            .execute(&pool)
            .await
            .unwrap();

        let ana = somebody(&pool, false).await;
        let ana_again = again(&ana);

        let Json(listed) = tracks(ana, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        let guested = listed
            .tracks
            .iter()
            .find(|track| track.id == "t10")
            .expect("the fixture numbers its tracks by their record");

        assert_eq!(
            guested.artists.as_deref(),
            Some("Triana feat. Zoë Johnston"),
            "the sentence the record uses, not the two names with a comma between"
        );

        let plain = listed
            .tracks
            .iter()
            .find(|track| track.id != "t10")
            .expect("the fixture has more than one");

        assert_eq!(
            plain.artists.as_deref(),
            Some("Triana"),
            "and a track that says nothing extra is still read from its names"
        );

        // The panel that opens over the row, which asks a different statement the
        // same question.
        let Json(opened) = detail(ana_again, State(pool.clone()), UrlPath("t10".to_string()))
            .await
            .unwrap();

        assert_eq!(
            opened.artists.as_deref(),
            Some("Triana feat. Zoë Johnston"),
            "the row and the panel over it are the same song"
        );
    }

    /// Named identifiers come back as rows, and only the ones this account may see.
    ///
    /// This is what draws a queue: a queue holds identifiers, so showing it as rows
    /// needs their titles in one request rather than one request per track. Which
    /// makes it the one filter somebody could hand a list of identifiers they were
    /// never shown, so the wall matters here as much as anywhere.
    #[tokio::test]
    async fn naming_tracks_brings_back_those_tracks_and_no_others() {
        let pool = a_collection().await;
        let ana = somebody(&pool, false).await;
        let ana_again = again(&ana);

        let Json(all) = tracks(ana, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        // Two of the four, one from each library, and a stray comma and blank to be
        // dropped rather than counted.
        let named = format!("{},,{} ,", all.tracks[0].id, all.tracks[3].id);
        let asked = Query(Filter {
            ids: Some(named),
            ..Default::default()
        });

        let Json(some) = tracks(ana_again, State(pool.clone()), asked, all_of_it())
            .await
            .unwrap();

        assert_eq!(
            some.total, 2,
            "the two named, and the blanks are not a third"
        );
        assert_eq!(
            some.tracks.iter().map(|t| &t.id).collect::<Vec<_>>(),
            vec![&all.tracks[0].id, &all.tracks[3].id],
            "as the listing orders them, which is what the caller reorders"
        );

        // The same names, asked by somebody walled off from the second library.
        let walled = somebody_else(&pool).await;
        let named = format!("{},{}", all.tracks[0].id, all.tracks[3].id);
        let asked = Query(Filter {
            ids: Some(named),
            ..Default::default()
        });

        let Json(theirs) = tracks(walled, State(pool.clone()), asked, all_of_it())
            .await
            .unwrap();

        assert_eq!(
            theirs.total, 1,
            "naming a track in a library you may not see does not fetch it"
        );
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

    /// A row says who made it and what record it is from.
    ///
    /// Which sounds like the least worth asserting of anything here, and is the
    /// one thing that was wrong. Both columns are read through the credit's role,
    /// and the roles these statements asked for — `main` for a track and for an
    /// album alike — are a name the scanner never writes: it writes `artist` on a
    /// track's credit and `albumartist` on a record's. So every row came back with
    /// no artist at all, and the ordering, which is by the album artist before
    /// anything else, ordered by a column that was always null.
    ///
    /// Nothing else caught it. Not the ordering tests, because with the whole key
    /// null the rows still came out in a stable order; not the search, which reads
    /// its own index; and not the fixtures, which sowed `main` themselves and so
    /// agreed with the mistake. Hence this: the assertion is on the value in the
    /// row rather than on the count of rows, and the fixture now sows the two roles
    /// the scanner actually writes.
    #[tokio::test]
    async fn a_row_says_who_made_it_and_what_record_it_is_from() {
        let pool = a_collection().await;
        let ana = somebody(&pool, false).await;
        let ana_again = again(&ana);

        let Json(listed) = tracks(ana, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        for track in &listed.tracks {
            assert_eq!(
                track.artists.as_deref(),
                Some("Triana"),
                "the credit is read through its role, and the row lost it"
            );
            assert!(
                track.album.is_some(),
                "every track in the fixture is on a record"
            );
        }

        // The same credit, on the record rather than on the song, which is a
        // different role and a different statement.
        let Json(records) = albums(ana_again, State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        for record in &records.albums {
            assert_eq!(record.artist.as_deref(), Some("Triana"));
        }
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

    /// A name a record is filed under, with not one track crediting it — what a
    /// compilation writes, and what Purple Rain writes when it is signed by "Prince
    /// and The Revolution" while every track on it credits Prince and The Revolution
    /// apart.
    ///
    /// The listing has always let such a name in, since that is what makes it an
    /// artist at all, and it has to count what it let in the same way: a row reading
    /// nought records and nought songs is a row that opens onto nothing.
    #[tokio::test]
    async fn a_name_a_record_is_filed_under_counts_that_record() {
        let pool = a_collection().await;
        let at = db::now();

        sqlx::query(
            "INSERT INTO artists (id, public_id, name, sort_name, created_at, updated_at)
             VALUES (2, 'ar2', 'Various Artists', 'Various Artists', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO album_artists (album_id, artist_id, role, position)
             VALUES (1, 2, 'albumartist', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let hers = somebody(&pool, false).await;
        let Json(found) = artists(again(&hers), State(pool.clone()), nothing(), all_of_it())
            .await
            .unwrap();

        let row = found
            .artists
            .iter()
            .find(|a| a.name == "Various Artists")
            .expect("a name a record is filed under is an artist");

        assert_eq!(row.albums, 1, "the record they sign");
        assert_eq!(row.tracks, 1, "what is on it and still there");

        let Json(read) = artist(
            again(&hers),
            State(pool.clone()),
            UrlPath("ar2".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(read.albums, 1);
        assert_eq!(read.tracks, 1);
        assert_eq!(read.genres.as_deref(), Some("Flamenco"));
        assert_eq!(read.records.len(), 1, "the record opens from their page");
        assert_eq!(read.records[0].name, "El Patio");
    }

    /// What the panel is playing shows up as the panel's, kept apart from the same
    /// person listening on a phone, and a second song replaces the first rather
    /// than joining it.
    #[tokio::test]
    async fn what_the_panel_plays_is_noted_as_one_thing_at_a_time() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;

        let ids: Vec<String> = sqlx::query_scalar("SELECT public_id FROM tracks ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();

        assert_eq!(
            playing(
                again(&hers),
                State(pool.clone()),
                State(crate::net::Net::new()),
                UrlPath(ids[0].clone()),
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );

        let listening: Vec<(String, i64)> =
            sqlx::query_as("SELECT client, track_id FROM now_playing")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(listening.len(), 1);
        assert_eq!(listening[0].0, PANEL_AS_A_PLAYER, "named as the panel");

        let _ = playing(
            again(&hers),
            State(pool.clone()),
            State(crate::net::Net::new()),
            UrlPath(ids[1].clone()),
        )
        .await
        .unwrap();

        let second: i64 = sqlx::query_scalar("SELECT id FROM tracks WHERE public_id = ?")
            .bind(&ids[1])
            .fetch_one(&pool)
            .await
            .unwrap();

        let listening: Vec<i64> = sqlx::query_scalar("SELECT track_id FROM now_playing")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            listening,
            vec![second],
            "the next song replaces the one before it rather than joining it or \
             being ignored beside it"
        );
    }

    /// A track nobody has is not worth an error: the panel is telling us what it
    /// started, not asking for anything, and the answer is the same either way.
    #[tokio::test]
    async fn playing_something_that_is_not_here_notes_nothing_and_says_so_quietly() {
        let pool = a_collection().await;
        let hers = somebody(&pool, false).await;

        assert_eq!(
            playing(
                again(&hers),
                State(pool.clone()),
                State(crate::net::Net::new()),
                UrlPath("nothing-by-this-name".to_string()),
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );

        let noted: i64 = sqlx::query_scalar("SELECT count(*) FROM now_playing")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(noted, 0);
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
