// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Bookmarks and the play queue: where somebody left off.
//!
//! Both are per user and irreplaceable, so they sit on the user side of the
//! schema. A bookmark marks a position inside one track — for an audiobook or a
//! long mix — while the play queue is what a listener had lined up, so they can
//! carry on from another device.

use super::auth::Authenticated;
use super::browsing;
use super::error::ApiError;
use super::models::Child;
use super::response::{self, Empty, Repeated};
use crate::db;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::error;

/// A bookmark as the database keeps it: which track, where in it, and when.
type BookmarkRow = (i64, i64, Option<String>, String, String);

/// The saved queue itself: what is playing, where in it, when and by whom.
type QueueRow = (Option<i64>, i64, String, String);

#[derive(Debug, Deserialize)]
pub struct IdQuery {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBookmarkQuery {
    id: String,
    /// Milliseconds into the track.
    position: i64,
    comment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveQueueQuery {
    /// The tracks in order. Repeats, and may be empty to clear the queue.
    #[serde(default)]
    id: Vec<String>,
    /// Which of them is playing.
    current: Option<String>,
    /// Milliseconds into the current track.
    position: Option<i64>,
}

#[derive(Serialize)]
struct BookmarksBody {
    bookmarks: Bookmarks,
}

#[derive(Serialize)]
struct Bookmarks {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bookmark: Vec<Bookmark>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bookmark {
    position: i64,
    username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    created: String,
    changed: String,
    entry: Child,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayQueueBody {
    play_queue: PlayQueue,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayQueue {
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<i64>,
    username: String,
    changed: String,
    /// Which client saved it, so a listener can tell where they left off.
    changed_by: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entry: Vec<Child>,
}

pub async fn get_bookmarks(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let rows: Result<Vec<BookmarkRow>, _> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT b.track_id, b.position_ms, b.comment, b.created_at, b.updated_at
           FROM bookmarks b
           JOIN tracks t ON t.id = b.track_id
          WHERE b.user_id = ? AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
          ORDER BY b.updated_at DESC"
    ))
    .bind(auth.user.id)
    .bind(auth.user.id)
    .fetch_all(&pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return internal(e, auth.format, "listing bookmarks"),
    };

    let ids: Vec<i64> = rows.iter().map(|(id, _, _, _, _)| *id).collect();
    let entries = match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(entries) => entries,
        Err(e) => return internal(e, auth.format, "loading bookmarked tracks"),
    };

    // The loader returns one entry per id in order, so zipping lines up.
    let bookmark = entries
        .into_iter()
        .zip(rows)
        .map(
            |(entry, (_, position, comment, created, changed))| Bookmark {
                position,
                username: auth.user.username.clone(),
                comment,
                created,
                changed,
                entry,
            },
        )
        .collect();

    response::ok(
        auth.format,
        BookmarksBody {
            bookmarks: Bookmarks { bookmark },
        },
    )
}

pub async fn create_bookmark(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<CreateBookmarkQuery>,
) -> Response {
    let track_id: Result<Option<i64>, _> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT id FROM tracks WHERE public_id = ? AND missing_since IS NULL
        AND library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(auth.user.id)
    .bind(&query.id)
    .fetch_optional(&pool)
    .await;

    let track_id = match track_id {
        Ok(Some(id)) => id,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => return internal(e, auth.format, "looking up a track to bookmark"),
    };

    let timestamp = db::now();
    let written = sqlx::query(
        "INSERT INTO bookmarks (user_id, track_id, position_ms, comment, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, track_id) DO UPDATE SET
             position_ms = excluded.position_ms,
             comment = excluded.comment,
             updated_at = excluded.updated_at",
    )
    .bind(auth.user.id)
    .bind(track_id)
    .bind(query.position.max(0))
    .bind(&query.comment)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await;

    match written {
        Ok(_) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "saving a bookmark"),
    }
}

pub async fn delete_bookmark(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
) -> Response {
    let deleted = sqlx::query(
        "DELETE FROM bookmarks
          WHERE user_id = ?
            AND track_id = (SELECT id FROM tracks WHERE public_id = ?)",
    )
    .bind(auth.user.id)
    .bind(&query.id)
    .execute(&pool)
    .await;

    match deleted {
        // Deleting one that was not there is not a failure: the caller wanted it
        // gone and it is gone.
        Ok(_) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "deleting a bookmark"),
    }
}

pub async fn get_play_queue(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let queue: Result<Option<QueueRow>, _> = sqlx::query_as(
        "SELECT current_track_id, position_ms, changed_at, changed_by
           FROM play_queues WHERE user_id = ?",
    )
    .bind(auth.user.id)
    .fetch_optional(&pool)
    .await;

    let Some((current_track_id, position, changed, changed_by)) = (match queue {
        Ok(queue) => queue,
        Err(e) => return internal(e, auth.format, "loading the play queue"),
    }) else {
        // Nothing saved yet. An empty element rather than a 70: not having a
        // queue is a state, not a failure.
        return response::ok(
            auth.format,
            PlayQueueBody {
                play_queue: PlayQueue {
                    current: None,
                    position: None,
                    username: auth.user.username,
                    changed: db::now(),
                    changed_by: auth.client,
                    entry: Vec::new(),
                },
            },
        );
    };

    let ids: Result<Vec<i64>, _> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT q.track_id FROM play_queue_tracks q
           JOIN tracks t ON t.id = q.track_id
          WHERE q.user_id = ? AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
          ORDER BY q.position"
    ))
    .bind(auth.user.id)
    .bind(auth.user.id)
    .fetch_all(&pool)
    .await;

    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => return internal(e, auth.format, "listing the play queue"),
    };

    let entries = match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(entries) => entries,
        Err(e) => return internal(e, auth.format, "loading the play queue"),
    };

    // The current track is reported by its public id, which is what the client
    // sent and what it will send back.
    let current = match current_track_id {
        Some(id) => match sqlx::query_scalar("SELECT public_id FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_optional(&pool)
            .await
        {
            Ok(current) => current,
            Err(e) => return internal(e, auth.format, "resolving the current track"),
        },
        None => None,
    };

    response::ok(
        auth.format,
        PlayQueueBody {
            play_queue: PlayQueue {
                current,
                position: Some(position),
                username: auth.user.username,
                changed,
                changed_by,
                entry: entries,
            },
        },
    )
}

pub async fn save_play_queue(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<SaveQueueQuery>,
) -> Response {
    match write_queue(&pool, &auth, &query).await {
        Ok(()) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "saving the play queue"),
    }
}

async fn write_queue(
    pool: &SqlitePool,
    auth: &Authenticated,
    query: &SaveQueueQuery,
) -> Result<(), sqlx::Error> {
    let mut tx = crate::db::writing(pool).await?;

    // The queue row first, because the entries carry a foreign key to it: the
    // entries cannot exist before the queue they belong to. The current track is
    // filled in afterwards, once the ids have been resolved.
    sqlx::query(
        "INSERT INTO play_queues (user_id, position_ms, changed_at, changed_by)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET
             position_ms = excluded.position_ms,
             changed_at = excluded.changed_at,
             changed_by = excluded.changed_by",
    )
    .bind(auth.user.id)
    .bind(query.position.unwrap_or(0).max(0))
    .bind(db::now())
    .bind(&auth.client)
    .execute(&mut *tx)
    .await?;

    // Rewritten whole, like a playlist and for the same reason: the key is
    // (user, position), and shifting positions in place violates it.
    sqlx::query("DELETE FROM play_queue_tracks WHERE user_id = ?")
        .bind(auth.user.id)
        .execute(&mut *tx)
        .await?;

    let mut current_track_id = None;
    let mut position = 0i64;

    for public_id in &query.id {
        let id: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE public_id = ?")
            .bind(public_id)
            .fetch_optional(&mut *tx)
            .await?;

        let Some(id) = id else { continue };

        if query.current.as_deref() == Some(public_id.as_str()) {
            current_track_id = Some(id);
        }

        sqlx::query("INSERT INTO play_queue_tracks (user_id, position, track_id) VALUES (?, ?, ?)")
            .bind(auth.user.id)
            .bind(position)
            .bind(id)
            .execute(&mut *tx)
            .await?;

        position += 1;
    }

    sqlx::query("UPDATE play_queues SET current_track_id = ? WHERE user_id = ?")
        .bind(current_track_id)
        .bind(auth.user.id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await
}

fn internal(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}
