// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Lists somebody made, which is the one part of a collection the server did not put
//! there.
//!
//! **A list is private or public to everybody on this server, and nothing in between.**
//! There is no sharing with one person: this is a house server, and a setting whose
//! options are "me" and "everyone here" is a setting people can hold in their head.
//! Public is what `/rest` has always been able to set, so the panel shows it rather
//! than pretending it is not there.
//!
//! **Only the owner writes.** Reading a public one is reading, and everything else —
//! renaming, reordering, adding, deleting — is the owner's alone, an administrator
//! included. Administration is about accounts and libraries; what somebody put in a
//! list of their own is not.
//!
//! **Entries are keyed by position, and duplicates belong.** The same song twice is two
//! entries, which is what the schema keys on and what a running order sometimes means.
//! Positions cannot be shuffled in place — SQLite walks the rows in order and would
//! break the primary key mid-statement — so every change that moves anything rewrites
//! the whole list inside one transaction.
//!
//! **What a reader cannot see is left alone rather than left out.** A list can hold
//! tracks from a library this account may not reach: they are missing from every figure
//! and every page it is shown, and a reorder moves what it asked to move without
//! disturbing them. Anything else would quietly empty somebody's list of the half they
//! were walled off from.

use super::error::ApiError;
use super::session::Panel;
use crate::db::{self, InTurn};
use crate::types::{
    Adding, ErrorBody, Holding, Moving, NewPlaylist, Playlist, PlaylistChanges, PlaylistEntry,
    PlaylistTracks, Playlists, Track,
};
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use sqlx::{Sqlite, SqlitePool, Transaction};
use tracing::error;

/// A list with its figures, counted over what whoever is asking can actually reach.
///
/// Not the macro `/rest` reads. That one answers a different shape — it has no use for
/// how many of the files have gone, and this has none for when the list was made — and
/// the two are one statement each rather than one statement with two readers.
macro_rules! a_playlist_row {
    () => {
        concat!(
            visible_libraries!(),
            "SELECT p.id, p.public_id, p.name, p.comment, p.is_public, p.updated_at,
                    u.username AS owner,
                    (SELECT count(*) FROM playlist_tracks pt
                       JOIN tracks t ON t.id = pt.track_id
                      WHERE pt.playlist_id = p.id AND t.missing_since IS NULL
                        AND t.library_id IN (SELECT id FROM visible_libraries)) AS tracks,
                    (SELECT sum(t.duration_ms) / 1000 FROM playlist_tracks pt
                       JOIN tracks t ON t.id = pt.track_id
                      WHERE pt.playlist_id = p.id AND t.missing_since IS NULL
                        AND t.library_id IN (SELECT id FROM visible_libraries)) AS duration,
                    -- The files that have gone, which is a fact worth saying on the row
                    -- rather than hiding: a list that says '9 tracks · 3 missing' is
                    -- one somebody can go and settle.
                    (SELECT count(*) FROM playlist_tracks pt
                       JOIN tracks t ON t.id = pt.track_id
                      WHERE pt.playlist_id = p.id AND t.missing_since IS NOT NULL
                        AND t.library_id IN (SELECT id FROM visible_libraries)) AS missing
               FROM playlists p
               JOIN users u ON u.id = p.owner_id"
        )
    };
}

#[derive(sqlx::FromRow)]
struct PlaylistRow {
    id: i64,
    public_id: String,
    name: String,
    comment: Option<String>,
    is_public: bool,
    updated_at: String,
    owner: String,
    tracks: i64,
    duration: Option<i64>,
    missing: i64,
}

impl PlaylistRow {
    /// The answer, once we know who is reading it: whose it is only means something
    /// against somebody.
    fn seen_by(self, who: &str) -> Playlist {
        Playlist {
            mine: self.owner == who,
            id: self.public_id,
            name: self.name,
            comment: self.comment,
            owner: self.owner,
            public: self.is_public,
            tracks: self.tracks,
            duration: self.duration,
            missing: self.missing,
            changed: self.updated_at,
        }
    }
}

/// Your lists
///
/// Yours, and the ones other accounts have made public. Nothing else exists as far as
/// this answers: a private list of somebody else's is not a thing this account may know
/// about, so it is absent rather than refused.
///
/// Not paged. A collection has thousands of tracks and an account has a handful of
/// lists, and the screen groups them rather than scrolling them.
#[utoipa::path(
    get,
    path = "/playlists",
    tag = "playlists",
    responses(
        (status = 200, description = "The lists this account may see", body = Playlists),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list(
    panel: Panel,
    State(pool): State<SqlitePool>,
) -> Result<Json<Playlists>, ApiError> {
    let rows: Vec<PlaylistRow> = sqlx::query_as(concat!(
        a_playlist_row!(),
        " WHERE p.owner_id = ? OR p.is_public = 1
          ORDER BY p.name COLLATE NOCASE"
    ))
    .bind(panel.user.id)
    .bind(panel.user.id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing the playlists"))?;

    Ok(Json(Playlists {
        playlists: rows
            .into_iter()
            .map(|row| row.seen_by(&panel.user.username))
            .collect(),
    }))
}

/// One list
#[utoipa::path(
    get,
    path = "/playlists/{id}",
    tag = "playlists",
    params(("id" = String, Path, description = "Which list")),
    responses(
        (status = 200, description = "What it is and how much of it there is", body = Playlist),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such list, or not one you may see", body = ErrorBody),
    )
)]
pub async fn one(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Playlist>, ApiError> {
    let row = readable(&pool, &panel, &id).await?;

    Ok(Json(row.seen_by(&panel.user.username)))
}

/// What is in a list
///
/// In its own order, a window at a time, with the position each entry sits at. The
/// position is what a row is taken out by, and it is the list's own numbering rather
/// than the track number on its record.
///
/// Tracks whose files have gone are here. This is the one screen where they are worth
/// showing — it is where somebody comes to find out what went — and they say so.
#[utoipa::path(
    get,
    path = "/playlists/{id}/tracks",
    tag = "playlists",
    params(
        ("id" = String, Path, description = "Which list"),
        super::collection::Paging,
    ),
    responses(
        (status = 200, description = "A page of it, and how many in all", body = PlaylistTracks),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such list, or not one you may see", body = ErrorBody),
    )
)]
pub async fn tracks(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
    Query(paging): Query<super::collection::Paging>,
) -> Result<Json<PlaylistTracks>, ApiError> {
    let row = readable(&pool, &panel, &id).await?;
    let (limit, offset) = paging.window();
    let who = panel.user.id;

    // Counted apart from the page, and over the same rows: the total is what the foot
    // of the list counts against, so a figure taken from a different condition would
    // have the screen asking for a window that is not there.
    let total: i64 = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT count(*) FROM playlist_tracks pt
           JOIN tracks t ON t.id = pt.track_id
          WHERE pt.playlist_id = ?
            AND t.library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(who)
    .bind(row.id)
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "counting what is in a playlist"))?;

    let rows: Vec<EntryRow> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT pt.position,
                t.public_id, t.title,
                coalesce(t.artist_credit,
                  (SELECT group_concat(a.name, ', ')
                     FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
                    WHERE ta.track_id = t.id AND ta.role = 'artist')) AS artists,
                al.name AS album, al.public_id AS album_id,
                (SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                  WHERE tg.track_id = t.id ORDER BY g.name LIMIT 1) AS genre,
                t.track_number, t.duration_ms, t.suffix, t.bit_rate,
                t.missing_since IS NOT NULL AS missing,
                s.starred_at
           FROM playlist_tracks pt
           JOIN tracks t ON t.id = pt.track_id
           LEFT JOIN albums al ON al.id = t.album_id
           LEFT JOIN user_track_stats s ON s.track_id = t.id AND s.user_id = ?
          WHERE pt.playlist_id = ?
            AND t.library_id IN (SELECT id FROM visible_libraries)
          ORDER BY pt.position
          LIMIT ? OFFSET ?"
    ))
    .bind(who)
    .bind(who)
    .bind(row.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading what is in a playlist"))?;

    Ok(Json(PlaylistTracks {
        total,
        tracks: rows.into_iter().map(PlaylistEntry::from).collect(),
    }))
}

/// One entry of a list: where it sits, and the track itself.
#[derive(sqlx::FromRow)]
struct EntryRow {
    position: i64,
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
    starred_at: Option<String>,
}

impl From<EntryRow> for PlaylistEntry {
    fn from(row: EntryRow) -> Self {
        Self {
            at: row.position,
            track: Track {
                id: row.public_id,
                title: row.title,
                artists: row.artists,
                album: row.album,
                album_id: row.album_id,
                genre: row.genre,
                track_number: row.track_number,
                duration: row.duration_ms.map(|ms| ms / 1000),
                suffix: row.suffix,
                bit_rate: row.bit_rate,
                missing: row.missing,
                starred_at: row.starred_at,
            },
        }
    }
}

/// Which of your lists a track is already in
///
/// What the picker in a track's panel is drawn from: a list it is already in is named as
/// such and cannot be pressed, so pressing twice cannot quietly add a second copy.
///
/// Only your own, because those are the only ones anything can be added to. A track in a
/// library this account cannot reach answers an empty list rather than a 404: what is
/// being asked is what may be added to, and the answer is nothing.
#[utoipa::path(
    get,
    path = "/tracks/{id}/playlists",
    tag = "playlists",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 200, description = "The lists of yours holding it", body = Holding),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn holding(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Holding>, ApiError> {
    let playlists: Vec<String> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT DISTINCT p.public_id
           FROM playlists p
           JOIN playlist_tracks pt ON pt.playlist_id = p.id
           JOIN tracks t ON t.id = pt.track_id
          WHERE p.owner_id = ? AND t.public_id = ?
            AND t.library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(panel.user.id)
    .bind(panel.user.id)
    .bind(&id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "finding which lists hold a track"))?;

    Ok(Json(Holding { playlists }))
}

/// Make a list
///
/// Empty, or holding what it is given. Both ways in from the panel come through here:
/// the button on the screen of lists makes an empty one, and saving what is playing
/// makes one holding the queue.
#[utoipa::path(
    post,
    path = "/playlists",
    tag = "playlists",
    request_body = NewPlaylist,
    responses(
        (status = 201, description = "The list as it now stands", body = Playlist),
        (status = 400, description = "A list with no name", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Json(asked): Json<NewPlaylist>,
) -> Result<(StatusCode, Json<Playlist>), ApiError> {
    let name = named(&asked.name)?;
    let who = panel.user.id;

    let public_id = db::public_id().map_err(|e| {
        error!("minting a playlist id: {e:#}");
        ApiError::Internal
    })?;

    let mut writing = db::writing(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "making a playlist"))?;

    let at = db::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO playlists (public_id, owner_id, name, comment, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&public_id)
    .bind(who)
    .bind(name)
    .bind(
        asked
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty()),
    )
    .bind(&at)
    .bind(&at)
    .fetch_one(&mut **writing)
    .await
    .map_err(|e| ApiError::internal(e, "making a playlist"))?;

    if let Some(wanted) = &asked.tracks {
        let held = reachable(&mut writing, who, wanted).await?;
        write_entries(&mut writing, id, &held).await?;
    }

    writing
        .commit()
        .await
        .map_err(|e| ApiError::internal(e, "making a playlist"))?;

    let row = readable(&pool, &panel, &public_id).await?;

    Ok((StatusCode::CREATED, Json(row.seen_by(&panel.user.username))))
}

/// Change a list
///
/// Its name, what it says about itself, and whether everybody here can see it. Anything
/// left out is left alone.
///
/// Whether it is public is on the same screen as the rest rather than three clicks away
/// in a menu, because it is the one property of a list that other people can notice.
#[utoipa::path(
    patch,
    path = "/playlists/{id}",
    tag = "playlists",
    params(("id" = String, Path, description = "Which list")),
    request_body = PlaylistChanges,
    responses(
        (status = 200, description = "The list as it now stands", body = Playlist),
        (status = 400, description = "A name that is no name", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not yours to change", body = ErrorBody),
        (status = 404, description = "No such list", body = ErrorBody),
    )
)]
pub async fn change(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
    Json(changes): Json<PlaylistChanges>,
) -> Result<Json<Playlist>, ApiError> {
    let row = writable(&pool, &panel, &id).await?;

    let mut writing = db::writing(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "changing a playlist"))?;

    if let Some(name) = &changes.name {
        let name = named(name)?;

        sqlx::query("UPDATE playlists SET name = ? WHERE id = ?")
            .bind(name)
            .bind(row.id)
            .execute(&mut **writing)
            .await
            .map_err(|e| ApiError::internal(e, "renaming a playlist"))?;
    }

    // An explicit empty comment clears it: what a list says about itself is something
    // somebody can take back, and an empty string in the column would be a description
    // that draws a blank line.
    if let Some(comment) = &changes.comment {
        let said = comment.trim();

        sqlx::query("UPDATE playlists SET comment = ? WHERE id = ?")
            .bind((!said.is_empty()).then_some(said))
            .bind(row.id)
            .execute(&mut **writing)
            .await
            .map_err(|e| ApiError::internal(e, "describing a playlist"))?;
    }

    if let Some(public) = changes.public {
        sqlx::query("UPDATE playlists SET is_public = ? WHERE id = ?")
            .bind(i64::from(public))
            .bind(row.id)
            .execute(&mut **writing)
            .await
            .map_err(|e| ApiError::internal(e, "changing who may see a playlist"))?;
    }

    touch(&mut writing, row.id).await?;
    writing
        .commit()
        .await
        .map_err(|e| ApiError::internal(e, "changing a playlist"))?;

    let row = readable(&pool, &panel, &id).await?;

    Ok(Json(row.seen_by(&panel.user.username)))
}

/// Delete a list
///
/// The list and its entries go; nothing about the music does. A list is a way of
/// arranging what is there, so deleting one is the cheapest thing in this API to undo
/// by hand and the only thing it can lose is the arrangement.
#[utoipa::path(
    delete,
    path = "/playlists/{id}",
    tag = "playlists",
    params(("id" = String, Path, description = "Which list")),
    responses(
        (status = 204, description = "Gone"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not yours to delete", body = ErrorBody),
        (status = 404, description = "No such list", body = ErrorBody),
    )
)]
pub async fn remove(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    let row = writable(&pool, &panel, &id).await?;

    // The entries go with it through the schema's own cascade, which is where that
    // belongs: a list without its entries is not a state anything should be able to
    // reach, however this row is deleted.
    sqlx::query("DELETE FROM playlists WHERE id = ?")
        .bind(row.id)
        .in_turn(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "deleting a playlist"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Add to a list
///
/// At the end, in the order they were named, repeats included: the same song twice is
/// two entries, which is what a running order sometimes means.
///
/// A track this account cannot reach is dropped rather than refused. What is being
/// asked for is "add these", and the ones it may not see are not part of "these" as far
/// as it is concerned.
#[utoipa::path(
    post,
    path = "/playlists/{id}/tracks",
    tag = "playlists",
    params(("id" = String, Path, description = "Which list")),
    request_body = Adding,
    responses(
        (status = 200, description = "The list as it now stands", body = Playlist),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not yours to add to", body = ErrorBody),
        (status = 404, description = "No such list", body = ErrorBody),
    )
)]
pub async fn add(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
    Json(asked): Json<Adding>,
) -> Result<Json<Playlist>, ApiError> {
    let row = writable(&pool, &panel, &id).await?;

    let mut writing = db::writing(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "adding to a playlist"))?;

    let held = reachable(&mut writing, panel.user.id, &asked.tracks).await?;

    // Appended by position rather than rewritten, which is the one change to a list
    // that can be: nothing already in it moves, so nothing can break the key.
    let next: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(position) + 1, 0) FROM playlist_tracks WHERE playlist_id = ?",
    )
    .bind(row.id)
    .fetch_one(&mut **writing)
    .await
    .map_err(|e| ApiError::internal(e, "finding the end of a playlist"))?;

    for (nth, track) in held.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?, ?, ?)",
        )
        .bind(row.id)
        .bind(next + nth as i64)
        .bind(track)
        .execute(&mut **writing)
        .await
        .map_err(|e| ApiError::internal(e, "adding to a playlist"))?;
    }

    touch(&mut writing, row.id).await?;
    writing
        .commit()
        .await
        .map_err(|e| ApiError::internal(e, "adding to a playlist"))?;

    let row = readable(&pool, &panel, &id).await?;

    Ok(Json(row.seen_by(&panel.user.username)))
}

/// Move one entry
///
/// By the positions the list itself reports, which is what makes this safe for a list
/// holding tracks the reader cannot see: they keep their place, and what is asked to
/// move moves past them.
///
/// The whole list is rewritten inside the transaction, because positions cannot be
/// shifted in place — SQLite walks the rows in ascending order and cannot defer a
/// uniqueness check, so an update that moved one row onto another's number would fail
/// halfway through.
#[utoipa::path(
    patch,
    path = "/playlists/{id}/tracks",
    tag = "playlists",
    params(("id" = String, Path, description = "Which list")),
    request_body = Moving,
    responses(
        (status = 204, description = "Moved, or already there"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not yours to reorder", body = ErrorBody),
        (status = 404, description = "No such list", body = ErrorBody),
    )
)]
pub async fn reorder(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
    Json(asked): Json<Moving>,
) -> Result<StatusCode, ApiError> {
    let row = writable(&pool, &panel, &id).await?;

    let mut writing = db::writing(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "reordering a playlist"))?;

    let entries: Vec<i64> = sqlx::query_scalar(
        "SELECT track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position",
    )
    .bind(row.id)
    .fetch_all(&mut **writing)
    .await
    .map_err(|e| ApiError::internal(e, "reading a playlist to reorder it"))?;

    if let Some(moved) = moved(&entries, asked.from, asked.to) {
        write_entries(&mut writing, row.id, &moved).await?;
        touch(&mut writing, row.id).await?;
    }

    writing
        .commit()
        .await
        .map_err(|e| ApiError::internal(e, "reordering a playlist"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Take one entry out
///
/// By position, so the same song appearing twice loses the copy that was pointed at.
#[utoipa::path(
    delete,
    path = "/playlists/{id}/tracks/{at}",
    tag = "playlists",
    params(
        ("id" = String, Path, description = "Which list"),
        ("at" = i64, Path, description = "Which position in it"),
    ),
    responses(
        (status = 204, description = "Out, or never there"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not yours to change", body = ErrorBody),
        (status = 404, description = "No such list", body = ErrorBody),
    )
)]
pub async fn drop_one(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath((id, at)): UrlPath<(String, i64)>,
) -> Result<StatusCode, ApiError> {
    let row = writable(&pool, &panel, &id).await?;

    let mut writing = db::writing(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "taking something out of a playlist"))?;

    // Read, drop, write back. The positions after it have to close up, and closing them
    // up in place is the one thing this table cannot do.
    let entries: Vec<i64> = sqlx::query_scalar(
        "SELECT track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position",
    )
    .bind(row.id)
    .fetch_all(&mut **writing)
    .await
    .map_err(|e| ApiError::internal(e, "reading a playlist"))?;

    if let Some(left) = without(&entries, at) {
        write_entries(&mut writing, row.id, &left).await?;
        touch(&mut writing, row.id).await?;
    }

    writing
        .commit()
        .await
        .map_err(|e| ApiError::internal(e, "taking something out of a playlist"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// A name with something in it, trimmed, or a refusal.
///
/// A list called nothing at all cannot be told from another one on any screen, and a
/// name of spaces is that with extra steps.
fn named(asked: &str) -> Result<&str, ApiError> {
    let name = asked.trim();

    if name.is_empty() {
        return Err(ApiError::Invalid("A playlist needs a name"));
    }

    Ok(name)
}

/// The list, if this account may read it at all.
///
/// A private list of somebody else's answers 404 rather than 403: whether it exists is
/// itself something they may not know.
async fn readable(pool: &SqlitePool, panel: &Panel, id: &str) -> Result<PlaylistRow, ApiError> {
    let row: Option<PlaylistRow> = sqlx::query_as(concat!(
        a_playlist_row!(),
        " WHERE p.public_id = ? AND (p.owner_id = ? OR p.is_public = 1)"
    ))
    .bind(panel.user.id)
    .bind(id)
    .bind(panel.user.id)
    .fetch_optional(pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading a playlist"))?;

    row.ok_or(ApiError::NotFound)
}

/// The list, if it is this account's own to change.
///
/// An administrator has no say here, which is the whole difference between this and
/// `readable`: a public list of somebody else's can be read and played by anybody and
/// changed by nobody but its owner. What is in a list is not administration.
///
/// 403 and not 404, because a public list somebody else owns is one this account has
/// already been told about.
async fn writable(pool: &SqlitePool, panel: &Panel, id: &str) -> Result<PlaylistRow, ApiError> {
    let row = readable(pool, panel, id).await?;

    if row.owner != panel.user.username {
        return Err(ApiError::NotAuthorized);
    }

    Ok(row)
}

/// The tracks named, as rows, keeping the order and the repeats — and only the ones this
/// account can reach.
async fn reachable(
    writing: &mut Transaction<'_, Sqlite>,
    who: i64,
    named: &[String],
) -> Result<Vec<i64>, ApiError> {
    let mut held = Vec::with_capacity(named.len());

    for id in named {
        let found: Option<i64> = sqlx::query_scalar(concat!(
            visible_libraries!(),
            "SELECT t.id FROM tracks t
              WHERE t.public_id = ? AND t.library_id IN (SELECT id FROM visible_libraries)"
        ))
        .bind(who)
        .bind(id)
        .fetch_optional(&mut **writing)
        .await
        .map_err(|e| ApiError::internal(e, "finding what is being added"))?;

        if let Some(found) = found {
            held.push(found);
        }
    }

    Ok(held)
}

/// Writes the entries of a list, replacing whatever was there.
async fn write_entries(
    writing: &mut Transaction<'_, Sqlite>,
    playlist: i64,
    tracks: &[i64],
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM playlist_tracks WHERE playlist_id = ?")
        .bind(playlist)
        .execute(&mut **writing)
        .await
        .map_err(|e| ApiError::internal(e, "clearing a playlist"))?;

    for (position, track) in tracks.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?, ?, ?)",
        )
        .bind(playlist)
        .bind(position as i64)
        .bind(track)
        .execute(&mut **writing)
        .await
        .map_err(|e| ApiError::internal(e, "writing a playlist"))?;
    }

    Ok(())
}

/// When the list last changed, which is what its row is sorted and read by.
async fn touch(writing: &mut Transaction<'_, Sqlite>, playlist: i64) -> Result<(), ApiError> {
    sqlx::query("UPDATE playlists SET updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(playlist)
        .execute(&mut **writing)
        .await
        .map_err(|e| ApiError::internal(e, "noting when a playlist changed"))?;

    Ok(())
}

/// The list with one entry moved, or nothing at all where there is nothing to do.
///
/// Where it lands is brought inside the list rather than refused, the same as dragging a
/// row in the queue: past the end means last. A position nothing sits at is nothing to
/// move, and a move that lands where it started is not a move — both answer `None`, so
/// the caller writes nothing and the row's date does not change over a drag that
/// amounted to nothing.
fn moved(entries: &[i64], from: i64, to: i64) -> Option<Vec<i64>> {
    let held = entries.len();

    if from < 0 || from as usize >= held || held < 2 {
        return None;
    }

    let from = from as usize;
    let to = (to.max(0) as usize).min(held - 1);

    if to == from {
        return None;
    }

    let mut moved = entries.to_vec();
    let one = moved.remove(from);
    moved.insert(to, one);

    Some(moved)
}

/// The list without the entry at that position, or nothing where there was none.
fn without(entries: &[i64], at: i64) -> Option<Vec<i64>> {
    if at < 0 || at as usize >= entries.len() {
        return None;
    }

    let mut left = entries.to_vec();
    left.remove(at as usize);

    Some(left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    /// Two accounts and two libraries with two tracks each, so a wall between them has
    /// something on both sides.
    ///
    /// Nobody is restricted here. A test that wants a wall puts one up itself, because
    /// the interesting cases are about a list written before the wall existed.
    async fn a_collection() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = "2026-01-01T00:00:00Z";
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, is_admin, created_at, updated_at)
             VALUES (1, 'ana', 'x', 0, ?1, ?1), (2, 'bea', 'x', 1, ?1, ?1);

             INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'kept', '/kept', ?1, ?1), (2, 'walled', '/walled', ?1, ?1);
             INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'root', '', 1), (2, 'f2', 2, 'root', '', 1);

             INSERT INTO tracks
                 (id, public_id, library_id, folder_id, path, file_size, file_modified_at,
                  content_type, suffix, title, duration_ms, last_seen_scan, created_at,
                  updated_at)
             VALUES (1, 't1', 1, 1, 'a.flac', 1, ?1, 'audio/flac', 'flac', 'One', 60000, 1,
                     ?1, ?1),
                    (2, 't2', 1, 1, 'b.flac', 1, ?1, 'audio/flac', 'flac', 'Two', 60000, 1,
                     ?1, ?1),
                    (3, 't3', 2, 2, 'c.flac', 1, ?1, 'audio/flac', 'flac', 'Three', 60000, 1,
                     ?1, ?1),
                    (4, 't4', 2, 2, 'd.flac', 1, ?1, 'audio/flac', 'flac', 'Four', 60000, 1,
                     ?1, ?1)",
        )
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    fn ana() -> Panel {
        asking(1, "ana")
    }

    /// An administrator, on purpose: everything she is refused here she is refused as
    /// the owner of nothing, not as a listener.
    fn bea() -> Panel {
        asking(2, "bea")
    }

    fn asking(id: i64, username: &str) -> Panel {
        Panel {
            id,
            user: User {
                id,
                username: username.to_string(),
                is_admin: username == "bea",
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        }
    }

    /// Walls an account off from everything but the first library.
    async fn walled(pool: &SqlitePool, who: i64) {
        sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, 1)")
            .bind(who)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn made(pool: &SqlitePool, who: Panel, name: &str, tracks: &[&str]) -> Playlist {
        let (_, Json(made)) = create(
            who,
            State(pool.clone()),
            Json(NewPlaylist {
                name: name.to_string(),
                comment: None,
                tracks: Some(tracks.iter().map(|id| id.to_string()).collect()),
            }),
        )
        .await
        .unwrap();

        made
    }

    /// What a list holds, as titles in order, so a test can say the whole answer in one
    /// line. Not `holding`, which is the handler that answers which lists hold a track.
    async fn titles(pool: &SqlitePool, who: Panel, id: &str) -> Vec<String> {
        let Json(page) = tracks(
            who,
            State(pool.clone()),
            UrlPath(id.to_string()),
            Query(super::super::collection::Paging {
                offset: 0,
                limit: Some(200),
            }),
        )
        .await
        .unwrap();

        page.tracks
            .into_iter()
            .map(|entry| entry.track.title)
            .collect()
    }

    /// Yours, plus what anybody made public, and nothing else at all.
    #[tokio::test]
    async fn a_private_list_of_somebody_elses_does_not_exist() {
        let pool = a_collection().await;

        made(&pool, ana(), "Mine", &[]).await;
        let hers = made(&pool, bea(), "Hers", &[]).await;
        let shared = made(&pool, bea(), "Shared", &[]).await;

        let _ = change(
            bea(),
            State(pool.clone()),
            UrlPath(shared.id.clone()),
            Json(PlaylistChanges {
                public: Some(true),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

        let Json(seen) = list(ana(), State(pool.clone())).await.unwrap();
        let names: Vec<&str> = seen.playlists.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Mine", "Shared"]);

        let theirs = seen
            .playlists
            .iter()
            .find(|p| p.name == "Shared")
            .expect("the public one");
        assert!(!theirs.mine, "not hers to change");
        assert_eq!(theirs.owner, "bea", "and it says whose it is");
        assert!(theirs.public);

        // And the private one is a 404 rather than a refusal: whether it exists is
        // itself something she may not know.
        let asked = one(ana(), State(pool), UrlPath(hers.id)).await;
        assert!(matches!(asked, Err(ApiError::NotFound)));
    }

    /// A public list can be read by anybody and changed by nobody but its owner — an
    /// administrator included, which is what `bea` being one is here to prove.
    #[tokio::test]
    async fn a_public_list_is_still_only_its_owners_to_change() {
        let pool = a_collection().await;
        let mine = made(&pool, ana(), "Mine", &["t1"]).await;

        let _ = change(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(PlaylistChanges {
                public: Some(true),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

        // She can read it and what is in it.
        let _ = one(bea(), State(pool.clone()), UrlPath(mine.id.clone()))
            .await
            .unwrap();
        assert_eq!(titles(&pool, bea(), &mine.id).await, ["One"]);

        // And nothing else.
        let renamed = change(
            bea(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(PlaylistChanges {
                name: Some("Hers now".to_string()),
                ..Default::default()
            }),
        )
        .await;
        assert!(matches!(renamed, Err(ApiError::NotAuthorized)));

        let added = add(
            bea(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(Adding {
                tracks: vec!["t2".to_string()],
            }),
        )
        .await;
        assert!(matches!(added, Err(ApiError::NotAuthorized)));

        let moved = reorder(
            bea(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(Moving { from: 0, to: 1 }),
        )
        .await;
        assert!(matches!(moved, Err(ApiError::NotAuthorized)));

        let dropped = drop_one(bea(), State(pool.clone()), UrlPath((mine.id.clone(), 0))).await;
        assert!(matches!(dropped, Err(ApiError::NotAuthorized)));

        let deleted = remove(bea(), State(pool.clone()), UrlPath(mine.id.clone())).await;
        assert!(matches!(deleted, Err(ApiError::NotAuthorized)));

        // None of which left a mark.
        assert_eq!(titles(&pool, ana(), &mine.id).await, ["One"]);
    }

    /// A list called nothing cannot be told from another one on any screen.
    #[tokio::test]
    async fn a_list_needs_a_name() {
        let pool = a_collection().await;

        for asked in ["", "   "] {
            let refused = create(
                ana(),
                State(pool.clone()),
                Json(NewPlaylist {
                    name: asked.to_string(),
                    ..Default::default()
                }),
            )
            .await;

            assert!(matches!(refused, Err(ApiError::Invalid(_))), "{asked:?}");
        }

        // And renaming one to nothing is the same refusal, so a list cannot lose the
        // name it already has.
        let mine = made(&pool, ana(), "Mine", &[]).await;
        let refused = change(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(PlaylistChanges {
                name: Some(" ".to_string()),
                ..Default::default()
            }),
        )
        .await;

        assert!(matches!(refused, Err(ApiError::Invalid(_))));
        let Json(still) = one(ana(), State(pool), UrlPath(mine.id)).await.unwrap();
        assert_eq!(still.name, "Mine");
    }

    /// Added at the end, in order, and the same song twice is two entries.
    #[tokio::test]
    async fn what_is_added_lands_at_the_end_and_a_repeat_belongs() {
        let pool = a_collection().await;
        let mine = made(&pool, ana(), "Mine", &["t1"]).await;

        let Json(after) = add(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(Adding {
                tracks: vec!["t2".to_string(), "t1".to_string()],
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            after.tracks, 3,
            "and the answer says how many there are now"
        );
        assert_eq!(titles(&pool, ana(), &mine.id).await, ["One", "Two", "One"]);
    }

    /// Taking one out closes the gap, so the positions stay a run of numbers.
    #[tokio::test]
    async fn taking_an_entry_out_renumbers_what_is_left() {
        let pool = a_collection().await;
        let mine = made(&pool, ana(), "Mine", &["t1", "t2", "t3"]).await;

        drop_one(ana(), State(pool.clone()), UrlPath((mine.id.clone(), 0)))
            .await
            .unwrap();

        let Json(page) = tracks(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Query(super::super::collection::Paging {
                offset: 0,
                limit: Some(200),
            }),
        )
        .await
        .unwrap();

        let at: Vec<i64> = page.tracks.iter().map(|entry| entry.at).collect();
        assert_eq!(at, [0, 1], "a run of numbers, not 1 and 2");
        assert_eq!(
            page.tracks
                .iter()
                .map(|entry| entry.track.title.as_str())
                .collect::<Vec<_>>(),
            ["Two", "Three"]
        );
    }

    /// Nothing out of reach goes into a list, and nothing out of reach is counted in
    /// one.
    #[tokio::test]
    async fn a_wall_holds_in_both_directions() {
        let pool = a_collection().await;

        // Made while she could see everything, then walled off from the second library.
        let mine = made(&pool, ana(), "Mine", &["t1", "t3"]).await;
        walled(&pool, 1).await;

        let Json(seen) = one(ana(), State(pool.clone()), UrlPath(mine.id.clone()))
            .await
            .unwrap();
        assert_eq!(seen.tracks, 1, "the one she can reach");
        assert_eq!(titles(&pool, ana(), &mine.id).await, ["One"]);

        // And a track from behind the wall is dropped rather than refused: what she
        // asked for was "add these", and that one is not one of hers.
        let Json(after) = add(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(Adding {
                tracks: vec!["t4".to_string(), "t2".to_string()],
            }),
        )
        .await
        .unwrap();

        assert_eq!(after.tracks, 2);
        assert_eq!(titles(&pool, ana(), &mine.id).await, ["One", "Two"]);

        // Asked of the table and not of the listing, which is the only way to tell this
        // from a track that went in and is merely invisible: every figure she is shown
        // counts what she can reach, so a smuggled entry would look exactly like none.
        let written: Vec<i64> = sqlx::query_scalar(
            "SELECT track_id FROM playlist_tracks WHERE playlist_id = 1 ORDER BY position",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(written, [1, 3, 2], "t4 never went in");
    }

    /// Reordering leaves what the reader cannot see exactly where it was.
    ///
    /// The reason positions travel in the answer rather than the panel sending the whole
    /// list back: a list written before a library was taken away holds tracks its owner
    /// can no longer see, and a reorder that rewrote it from what is on screen would
    /// quietly throw them out.
    #[tokio::test]
    async fn reordering_does_not_lose_what_is_out_of_sight() {
        let pool = a_collection().await;
        let mine = made(&pool, ana(), "Mine", &["t1", "t3", "t2"]).await;
        walled(&pool, 1).await;

        // On screen she has One at 0 and Two at 2 — the hidden one keeps its place in
        // between, which is why she is asked to move by position and not by row.
        assert_eq!(titles(&pool, ana(), &mine.id).await, ["One", "Two"]);

        reorder(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(Moving { from: 2, to: 0 }),
        )
        .await
        .unwrap();

        assert_eq!(titles(&pool, ana(), &mine.id).await, ["Two", "One"]);

        // And the one she cannot see is still in there, between them.
        let kept: Vec<i64> = sqlx::query_scalar(
            "SELECT track_id FROM playlist_tracks WHERE playlist_id = 1 ORDER BY position",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(kept, [2, 1, 3], "nothing was thrown away");
    }

    /// Deleting a list takes its entries and nothing else.
    #[tokio::test]
    async fn deleting_a_list_leaves_the_music_alone() {
        let pool = a_collection().await;
        let mine = made(&pool, ana(), "Mine", &["t1", "t2"]).await;

        remove(ana(), State(pool.clone()), UrlPath(mine.id.clone()))
            .await
            .unwrap();

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM playlist_tracks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0, "the entries went with it");

        let songs: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(songs, 4, "and the music did not");

        let gone = one(ana(), State(pool), UrlPath(mine.id)).await;
        assert!(matches!(gone, Err(ApiError::NotFound)));
    }

    /// Which lists hold a track: yours, and only yours.
    ///
    /// What the picker in a track's panel greys out. A public list of somebody else's
    /// holding it is not an answer to that question — there is nothing to add to there —
    /// and offering it would be offering something the next call refuses.
    #[tokio::test]
    async fn only_your_own_lists_answer_for_holding_a_track() {
        let pool = a_collection().await;
        let mine = made(&pool, ana(), "Mine", &["t1", "t2"]).await;
        made(&pool, ana(), "Empty", &[]).await;
        let hers = made(&pool, bea(), "Hers", &["t1"]).await;

        let _ = change(
            bea(),
            State(pool.clone()),
            UrlPath(hers.id.clone()),
            Json(PlaylistChanges {
                public: Some(true),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

        let Json(held) = holding(ana(), State(pool.clone()), UrlPath("t1".to_string()))
            .await
            .unwrap();
        assert_eq!(held.playlists, std::slice::from_ref(&mine.id));

        // A track in none of them is an empty answer and not a miss.
        let Json(none) = holding(ana(), State(pool.clone()), UrlPath("t3".to_string()))
            .await
            .unwrap();
        assert!(none.playlists.is_empty());

        // And the same track twice in one list names it once: the picker draws a row per
        // list, not a row per entry.
        let _ = add(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(Adding {
                tracks: vec!["t1".to_string()],
            }),
        )
        .await
        .unwrap();

        let Json(again) = holding(ana(), State(pool), UrlPath("t1".to_string()))
            .await
            .unwrap();
        assert_eq!(again.playlists, [mine.id]);
    }

    /// A description can be taken back, which is not the same as never having had one.
    #[tokio::test]
    async fn a_description_can_be_cleared() {
        let pool = a_collection().await;
        let mine = made(&pool, ana(), "Mine", &[]).await;

        let Json(said) = change(
            ana(),
            State(pool.clone()),
            UrlPath(mine.id.clone()),
            Json(PlaylistChanges {
                comment: Some("  Quiet things  ".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(said.comment.as_deref(), Some("Quiet things"), "trimmed");

        let Json(cleared) = change(
            ana(),
            State(pool),
            UrlPath(mine.id),
            Json(PlaylistChanges {
                comment: Some(String::new()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(cleared.comment, None);
    }

    /// Moving inside a list, which is the whole of the reordering: everything else is
    /// reading it and writing it back.
    #[test]
    fn an_entry_moves_to_where_it_was_dropped() {
        assert_eq!(moved(&[1, 2, 3, 4], 0, 2), Some(vec![2, 3, 1, 4]));
        assert_eq!(moved(&[1, 2, 3, 4], 3, 0), Some(vec![4, 1, 2, 3]));
    }

    /// Past either end lands on that end, like a row dragged past the bottom of the
    /// queue: that is an answer, not a mistake.
    #[test]
    fn a_move_past_the_end_lands_on_the_end() {
        assert_eq!(moved(&[1, 2, 3], 0, 99), Some(vec![2, 3, 1]));
        assert_eq!(moved(&[1, 2, 3], 2, -4), Some(vec![3, 1, 2]));
    }

    /// Nothing to do is nothing written, so a drag that came back to where it started
    /// does not move the date on the row.
    #[test]
    fn a_move_that_changes_nothing_is_not_a_move() {
        assert_eq!(moved(&[1, 2, 3], 1, 1), None);
        assert_eq!(moved(&[1, 2, 3], 7, 0), None, "nothing sits there");
        assert_eq!(moved(&[1, 2, 3], -1, 0), None);
        assert_eq!(moved(&[1], 0, 0), None, "one entry is no order");
        assert_eq!(moved(&[], 0, 0), None);
    }

    /// The same song twice is two entries, and a position points at one of them.
    #[test]
    fn a_repeat_is_two_entries_and_moves_on_its_own() {
        assert_eq!(moved(&[1, 1, 2], 2, 0), Some(vec![2, 1, 1]));
        assert_eq!(without(&[1, 1, 2], 0), Some(vec![1, 2]));
    }

    #[test]
    fn taking_one_out_closes_the_gap() {
        assert_eq!(without(&[1, 2, 3], 1), Some(vec![1, 3]));
        assert_eq!(without(&[1, 2, 3], 9), None, "nothing sits there");
        assert_eq!(without(&[1, 2, 3], -1), None);
        assert_eq!(without(&[1], 0), Some(vec![]));
    }
}
