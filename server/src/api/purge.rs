// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What a purge would cost, spelled out.
//!
//! Running one is `POST /jobs/purge`, like every other job, and what it counts
//! there is tracks. This is the one thing that call cannot say: the favourites,
//! the ratings and the playlist entries that go with those tracks and cannot be
//! scanned back. It exists because the purge is the one job with a dialogue in
//! front of it, and this is what the dialogue reads out.

use super::error::ApiError;
use super::session::Panel;
use crate::types::{ErrorBody, Loss};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;

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
