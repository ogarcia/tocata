// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Asking for what a scan only marked to be removed for good.
//!
//! The removing itself is in [`crate::purge`], which a scan also reaches when the
//! collection has been given a quarantine. What is here is the two calls, and it
//! is two on purpose: the first says what would go, in terms of the things that
//! cannot be got back — a catalogue rebuilds itself on the next scan, a rating
//! does not — and the second does it.

use super::error::ApiError;
use super::session::{Administrator, Panel};
use crate::config::Config;
use crate::scanner::Progress;
use crate::types::{ErrorBody, Loss, Removed};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;
use std::sync::Arc;

/// What a purge would remove
///
/// A dry run. The figures are the irreplaceable ones: a catalogue rebuilds itself
/// on the next scan, so albums and artists are not the interesting loss, and a
/// rating nobody wrote down again is.
#[utoipa::path(
    get,
    path = "/purge",
    tag = "purge",
    responses(
        (status = 200, description = "What would go", body = Loss),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn preview(
    _panel: Panel,
    State(pool): State<SqlitePool>,
) -> Result<Json<Loss>, ApiError> {
    let row: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL),
                (SELECT count(*) FROM playlist_tracks p
                   JOIN tracks t ON t.id = p.track_id
                  WHERE t.missing_since IS NOT NULL),
                (SELECT count(*) FROM user_track_stats s
                   JOIN tracks t ON t.id = s.track_id
                  WHERE t.missing_since IS NOT NULL AND s.starred_at IS NOT NULL),
                (SELECT count(*) FROM user_track_stats s
                   JOIN tracks t ON t.id = s.track_id
                  WHERE t.missing_since IS NOT NULL AND s.rating IS NOT NULL),
                (SELECT count(*) FROM user_track_stats s
                   JOIN tracks t ON t.id = s.track_id
                  WHERE t.missing_since IS NOT NULL AND s.play_count > 0),
                (SELECT count(*) FROM bookmarks b
                   JOIN tracks t ON t.id = b.track_id
                  WHERE t.missing_since IS NOT NULL)",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "counting what a purge would remove"))?;

    let (tracks, playlist_entries, favourites, ratings, played, bookmarks) = row;

    Ok(Json(Loss {
        tracks,
        playlist_entries,
        favourites,
        ratings,
        played,
        bookmarks,
    }))
}

/// Purge what is absent
///
/// Deletes every track marked absent, then everything left with nothing in it:
/// empty folders, albums with no tracks, artists with neither, genres and moods
/// nobody uses, and cover art nothing points at.
///
/// There is no undoing it short of a backup. Refused while a scan is running,
/// since a scan is in the middle of deciding what is absent.
#[utoipa::path(
    post,
    path = "/purge",
    tag = "purge",
    responses(
        (status = 200, description = "What was removed", body = Removed),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 409, description = "A scan is running", body = ErrorBody),
    )
)]
pub async fn purge(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    State(progress): State<Arc<Progress>>,
    State(config): State<Arc<Config>>,
) -> Result<Json<Removed>, ApiError> {
    if progress.is_scanning() {
        return Err(ApiError::Conflict(
            "A scan is running; it is deciding what is absent",
        ));
    }

    // No threshold: somebody asking for a purge means everything a scan has
    // marked, whatever the quarantine says. That is what the dry run above
    // counted, and what the dialogue asking them said.
    crate::purge::absent(&pool, config.data_dir(), None)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e, "purging what is absent"))
}
