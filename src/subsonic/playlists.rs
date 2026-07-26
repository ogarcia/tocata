// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Playlists.
//!
//! Every change replaces the whole list of entries inside a transaction rather
//! than shifting positions about. The primary key is `(playlist, position)`, so
//! moving positions upwards in place violates it mid-statement, and any
//! arithmetic on them is a source of off-by-one bugs. Rewriting is exact and,
//! for lists of the size a person makes, no slower.

use super::auth::Authenticated;
use super::browsing;
use super::error::ApiError;
use super::models::{Child, seconds};
use super::response::{self, Empty, Repeated};
use crate::db;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::HashSet;
use tracing::error;

#[derive(Debug, Deserialize)]
pub struct IdQuery {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateQuery {
    playlist_id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    song_id: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateQuery {
    playlist_id: String,
    name: Option<String>,
    comment: Option<String>,
    public: Option<bool>,
    #[serde(default)]
    song_id_to_add: Vec<String>,
    #[serde(default)]
    song_index_to_remove: Vec<i64>,
}

#[derive(Serialize)]
struct PlaylistsBody {
    playlists: Playlists,
}

#[derive(Serialize)]
struct Playlists {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    playlist: Vec<Playlist>,
}

#[derive(Serialize)]
struct PlaylistBody {
    playlist: PlaylistWithEntries,
}

#[derive(Serialize)]
struct PlaylistWithEntries {
    #[serde(flatten)]
    playlist: Playlist,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entry: Vec<Child>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Playlist {
    id: String,
    name: String,
    song_count: i64,
    /// Seconds, summed over the entries whose files are still there.
    duration: i64,
    created: String,
    changed: String,
    owner: String,
    public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PlaylistRow {
    id: i64,
    public_id: String,
    name: String,
    comment: Option<String>,
    is_public: bool,
    created_at: String,
    updated_at: String,
    owner: String,
    song_count: i64,
    duration_ms: Option<i64>,
}

impl From<PlaylistRow> for Playlist {
    fn from(row: PlaylistRow) -> Self {
        Self {
            id: row.public_id,
            name: row.name,
            song_count: row.song_count,
            duration: seconds(row.duration_ms).unwrap_or(0),
            created: row.created_at,
            changed: row.updated_at,
            owner: row.owner,
            public: row.is_public,
            comment: row.comment,
        }
    }
}

/// A playlist with its counts. Absent tracks are left out of both the count and
/// the duration: a list promising twelve songs that plays nine is worse than one
/// that says nine.
macro_rules! playlist_columns {
    () => {
        "
    SELECT p.id, p.public_id, p.name, p.comment, p.is_public, p.created_at,
           p.updated_at, u.username AS owner,
           (SELECT count(*) FROM playlist_tracks pt
              JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = p.id AND t.missing_since IS NULL
               AND t.library_id IN (SELECT id FROM libraries WHERE enabled = 1)) AS song_count,
           (SELECT sum(t.duration_ms) FROM playlist_tracks pt
              JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = p.id AND t.missing_since IS NULL
               AND t.library_id IN (SELECT id FROM libraries WHERE enabled = 1)) AS duration_ms
      FROM playlists p
      JOIN users u ON u.id = p.owner_id"
    };
}

pub async fn get_playlists(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    // Yours, plus anything anybody marked public.
    let rows: Result<Vec<PlaylistRow>, _> = sqlx::query_as(concat!(
        playlist_columns!(),
        " WHERE p.owner_id = ? OR p.is_public = 1
          ORDER BY p.name COLLATE NOCASE"
    ))
    .bind(auth.user.id)
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => response::ok(
            auth.format,
            PlaylistsBody {
                playlists: Playlists {
                    playlist: rows.into_iter().map(Playlist::from).collect(),
                },
            },
        ),
        Err(e) => internal(e, auth.format, "listing playlists"),
    }
}

pub async fn get_playlist(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<IdQuery>,
) -> Response {
    let row = match load_playlist(&pool, &query.id).await {
        Ok(Some(row)) => row,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => return internal(e, auth.format, "loading a playlist"),
    };

    if !can_read(&auth, &row) {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    let ids: Result<Vec<i64>, _> = sqlx::query_scalar(
        "SELECT pt.track_id FROM playlist_tracks pt
           JOIN tracks t ON t.id = pt.track_id
          WHERE pt.playlist_id = ? AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM libraries WHERE enabled = 1)
          ORDER BY pt.position",
    )
    .bind(row.id)
    .fetch_all(&pool)
    .await;

    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => return internal(e, auth.format, "listing playlist entries"),
    };

    match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(entries) => response::ok(
            auth.format,
            PlaylistBody {
                playlist: PlaylistWithEntries {
                    playlist: row.into(),
                    entry: entries,
                },
            },
        ),
        Err(e) => internal(e, auth.format, "loading playlist entries"),
    }
}

/// Creates a playlist, or replaces the contents of one.
///
/// With a `playlistId` the entries given become the whole list, which is how a
/// client saves a reordering: it sends the songs in the order it wants.
pub async fn create_playlist(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<CreateQuery>,
) -> Response {
    let outcome = match &query.playlist_id {
        Some(public_id) => replace_playlist(&pool, &auth, public_id, &query.song_id).await,
        None => match &query.name {
            Some(name) => create_new(&pool, &auth, name, &query.song_id).await,
            None => {
                return ApiError::MissingParameter("name")
                    .in_format(auth.format)
                    .into_response();
            }
        },
    };

    finish(&pool, &auth, outcome).await
}

pub async fn update_playlist(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<UpdateQuery>,
) -> Response {
    let outcome = apply_update(&pool, &auth, &query).await;

    match outcome {
        Ok(Refused::Allowed(_)) => response::ok(auth.format, Empty {}),
        Ok(Refused::NotFound) => ApiError::NotFound.in_format(auth.format).into_response(),
        Ok(Refused::NotYours) => ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response(),
        Err(e) => internal(e, auth.format, "updating a playlist"),
    }
}

pub async fn delete_playlist(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<IdQuery>,
) -> Response {
    let row = match load_playlist(&pool, &query.id).await {
        Ok(Some(row)) => row,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => return internal(e, auth.format, "loading a playlist to delete"),
    };

    if !can_write(&auth, &row) {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    // The entries go with it through the cascade, and they are the only thing
    // that does: a playlist owns nothing else.
    match sqlx::query("DELETE FROM playlists WHERE id = ?")
        .bind(row.id)
        .execute(&pool)
        .await
    {
        Ok(_) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "deleting a playlist"),
    }
}

fn internal(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}

/// What a write attempt came to.
enum Refused {
    Allowed(i64),
    NotFound,
    NotYours,
}

/// A public playlist is readable by anybody; only its owner, or an
/// administrator, can change it.
fn can_read(auth: &Authenticated, row: &PlaylistRow) -> bool {
    row.is_public || row.owner == auth.user.username || auth.user.is_admin
}

fn can_write(auth: &Authenticated, row: &PlaylistRow) -> bool {
    row.owner == auth.user.username || auth.user.is_admin
}

async fn load_playlist(
    pool: &SqlitePool,
    public_id: &str,
) -> Result<Option<PlaylistRow>, sqlx::Error> {
    sqlx::query_as(concat!(playlist_columns!(), " WHERE p.public_id = ?"))
        .bind(public_id)
        .fetch_optional(pool)
        .await
}

async fn create_new(
    pool: &SqlitePool,
    auth: &Authenticated,
    name: &str,
    song_ids: &[String],
) -> Result<Refused, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let timestamp = db::now();

    let public_id = match db::public_id() {
        Ok(id) => id,
        // The only failure here is the system running out of randomness, which
        // is not a database error but has to travel as one.
        Err(e) => {
            error!("minting a playlist id: {e:#}");
            return Err(sqlx::Error::WorkerCrashed);
        }
    };

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO playlists (public_id, owner_id, name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&public_id)
    .bind(auth.user.id)
    .bind(name)
    .bind(&timestamp)
    .bind(&timestamp)
    .fetch_one(&mut *tx)
    .await?;

    let track_ids = resolve_tracks(&mut tx, song_ids).await?;
    write_entries(&mut tx, id, &track_ids).await?;

    tx.commit().await?;
    Ok(Refused::Allowed(id))
}

async fn replace_playlist(
    pool: &SqlitePool,
    auth: &Authenticated,
    public_id: &str,
    song_ids: &[String],
) -> Result<Refused, sqlx::Error> {
    let Some(row) = load_playlist(pool, public_id).await? else {
        return Ok(Refused::NotFound);
    };
    if !can_write(auth, &row) {
        return Ok(Refused::NotYours);
    }

    let mut tx = pool.begin().await?;
    let track_ids = resolve_tracks(&mut tx, song_ids).await?;
    write_entries(&mut tx, row.id, &track_ids).await?;
    touch(&mut tx, row.id).await?;
    tx.commit().await?;

    Ok(Refused::Allowed(row.id))
}

async fn apply_update(
    pool: &SqlitePool,
    auth: &Authenticated,
    query: &UpdateQuery,
) -> Result<Refused, sqlx::Error> {
    let Some(row) = load_playlist(pool, &query.playlist_id).await? else {
        return Ok(Refused::NotFound);
    };
    if !can_write(auth, &row) {
        return Ok(Refused::NotYours);
    }

    let mut tx = pool.begin().await?;

    if let Some(name) = &query.name {
        sqlx::query("UPDATE playlists SET name = ? WHERE id = ?")
            .bind(name)
            .bind(row.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(comment) = &query.comment {
        sqlx::query("UPDATE playlists SET comment = ? WHERE id = ?")
            .bind(comment)
            .bind(row.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(public) = query.public {
        sqlx::query("UPDATE playlists SET is_public = ? WHERE id = ?")
            .bind(i64::from(public))
            .bind(row.id)
            .execute(&mut *tx)
            .await?;
    }

    if !query.song_index_to_remove.is_empty() || !query.song_id_to_add.is_empty() {
        // Read the list, change it here, write it back. The indexes to remove
        // refer to the positions the client is looking at, all of them at once:
        // applying them one after another would make removing 1 and 2 delete the
        // second and the fourth entry, because everything shifts after the first
        // removal. The specification does not say which is meant, and the client
        // computed those numbers against the list it has.
        let mut entries: Vec<i64> = sqlx::query_scalar(
            "SELECT track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position",
        )
        .bind(row.id)
        .fetch_all(&mut *tx)
        .await?;

        let removing: HashSet<i64> = query.song_index_to_remove.iter().copied().collect();
        entries = entries
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !removing.contains(&(*index as i64)))
            .map(|(_, track_id)| track_id)
            .collect();

        let added = resolve_tracks(&mut tx, &query.song_id_to_add).await?;
        entries.extend(added);

        write_entries(&mut tx, row.id, &entries).await?;
    }

    touch(&mut tx, row.id).await?;
    tx.commit().await?;

    Ok(Refused::Allowed(row.id))
}

/// Answers a create or replace with the playlist as it now stands, which saves
/// the client a second call to see what it just made.
async fn finish(
    pool: &SqlitePool,
    auth: &Authenticated,
    outcome: Result<Refused, sqlx::Error>,
) -> Response {
    let id = match outcome {
        Ok(Refused::Allowed(id)) => id,
        Ok(Refused::NotFound) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Ok(Refused::NotYours) => {
            return ApiError::NotAuthorized
                .in_format(auth.format)
                .into_response();
        }
        Err(e) => return internal(e, auth.format, "saving a playlist"),
    };

    let row: Result<Option<PlaylistRow>, _> =
        sqlx::query_as(concat!(playlist_columns!(), " WHERE p.id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await;

    match row {
        Ok(Some(row)) => {
            let ids: Vec<i64> = match sqlx::query_scalar(
                "SELECT pt.track_id FROM playlist_tracks pt
                   JOIN tracks t ON t.id = pt.track_id
                  WHERE pt.playlist_id = ? AND t.missing_since IS NULL
                    AND t.library_id IN (SELECT id FROM libraries WHERE enabled = 1)
                  ORDER BY pt.position",
            )
            .bind(row.id)
            .fetch_all(pool)
            .await
            {
                Ok(ids) => ids,
                Err(e) => return internal(e, auth.format, "listing the saved playlist"),
            };

            match browsing::load_tracks_by_ids(pool, auth.user.id, &ids).await {
                Ok(entries) => response::ok(
                    auth.format,
                    PlaylistBody {
                        playlist: PlaylistWithEntries {
                            playlist: row.into(),
                            entry: entries,
                        },
                    },
                ),
                Err(e) => internal(e, auth.format, "loading the saved playlist"),
            }
        }
        Ok(None) => ApiError::Internal.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "reloading a playlist"),
    }
}

/// Turns public ids into rows, keeping the order and the repeats.
///
/// A song named twice belongs twice: that is the point of keying entries by
/// position. An id matching nothing is dropped rather than failing the call.
async fn resolve_tracks(
    tx: &mut Transaction<'_, Sqlite>,
    song_ids: &[String],
) -> Result<Vec<i64>, sqlx::Error> {
    let mut resolved = Vec::with_capacity(song_ids.len());

    for public_id in song_ids {
        let id: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE public_id = ?")
            .bind(public_id)
            .fetch_optional(&mut **tx)
            .await?;

        if let Some(id) = id {
            resolved.push(id);
        }
    }

    Ok(resolved)
}

/// Writes the entries of a playlist, replacing whatever was there.
async fn write_entries(
    tx: &mut Transaction<'_, Sqlite>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM playlist_tracks WHERE playlist_id = ?")
        .bind(playlist_id)
        .execute(&mut **tx)
        .await?;

    for (position, track_id) in track_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?, ?, ?)",
        )
        .bind(playlist_id)
        .bind(position as i64)
        .bind(track_id)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

async fn touch(tx: &mut Transaction<'_, Sqlite>, playlist_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE playlists SET updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(playlist_id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    /// The removal rule, on its own: indexes point at the list the client is
    /// looking at, and all of them are applied at once.
    fn remove_indexes(entries: &[i64], indexes: &[i64]) -> Vec<i64> {
        let removing: HashSet<i64> = indexes.iter().copied().collect();

        entries
            .iter()
            .enumerate()
            .filter(|(index, _)| !removing.contains(&(*index as i64)))
            .map(|(_, entry)| *entry)
            .collect()
    }

    #[test]
    fn removing_one_index_takes_that_entry() {
        assert_eq!(remove_indexes(&[10, 20, 30], &[1]), vec![10, 30]);
    }

    /// The bug this guards against: applied one after another, removing 1 and
    /// then 2 takes the second entry and then the fourth, because everything
    /// shifted after the first removal.
    #[test]
    fn several_indexes_refer_to_the_original_positions() {
        assert_eq!(remove_indexes(&[10, 20, 30, 40], &[1, 2]), vec![10, 40]);
        assert_eq!(remove_indexes(&[10, 20, 30, 40], &[2, 1]), vec![10, 40]);
        assert_eq!(remove_indexes(&[10, 20, 30, 40], &[0, 3]), vec![20, 30]);
    }

    #[test]
    fn a_repeated_index_removes_one_entry() {
        assert_eq!(remove_indexes(&[10, 20, 30], &[1, 1]), vec![10, 30]);
    }

    #[test]
    fn an_index_past_the_end_is_ignored() {
        assert_eq!(remove_indexes(&[10, 20], &[5]), vec![10, 20]);
        assert_eq!(remove_indexes(&[10, 20], &[-1]), vec![10, 20]);
    }

    #[test]
    fn removing_everything_leaves_an_empty_list() {
        assert!(remove_indexes(&[10, 20], &[0, 1]).is_empty());
    }

    /// The same song twice is two entries, and an index refers to one of them.
    #[test]
    fn duplicates_are_separate_entries() {
        assert_eq!(remove_indexes(&[10, 10, 10], &[1]), vec![10, 10]);
    }
}
