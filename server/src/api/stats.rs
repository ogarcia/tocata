// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What there is, in figures.

use super::error::ApiError;
use super::session::Panel;
use crate::config::Config;
use crate::types::{ErrorBody, Stats};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Every figure in one row, in the order the statement below asks for them.
type Counts = (
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
);

/// Server figures
///
/// Counts of everything, the size of the collection, and how much room the
/// database takes.
#[utoipa::path(
    get,
    path = "/stats",
    tag = "stats",
    responses(
        (status = 200, description = "What there is", body = Stats),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn stats(
    _panel: Panel,
    State(pool): State<SqlitePool>,
    State(config): State<Arc<Config>>,
) -> Result<Json<Stats>, ApiError> {
    // One statement rather than ten round trips. Every figure is a count over an
    // indexed column, so the query is cheap however big the collection is.
    let row: Counts = sqlx::query_as(
        "SELECT (SELECT count(*) FROM artists),
                (SELECT count(*) FROM albums),
                (SELECT count(*) FROM tracks WHERE missing_since IS NULL),
                (SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL),
                (SELECT count(*) FROM genres),
                (SELECT count(*) FROM playlists),
                (SELECT count(*) FROM users),
                (SELECT count(*) FROM libraries),
                (SELECT sum(file_size) FROM tracks WHERE missing_since IS NULL),
                (SELECT sum(duration_ms) FROM tracks WHERE missing_since IS NULL)",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "counting what there is"))?;

    let (artists, albums, tracks, missing, genres, playlists, users, libraries, size, duration) =
        row;

    Ok(Json(Stats {
        version: env!("CARGO_PKG_VERSION").to_string(),
        artists,
        albums,
        tracks,
        missing,
        genres,
        playlists,
        users,
        libraries,
        total_size: size.unwrap_or(0),
        total_duration: duration.unwrap_or(0) / 1000,
        database_size: database_size(&config),
    }))
}

/// Size of the database and its log. A file that cannot be read counts as
/// nothing: a figure for a panel is not worth failing a request over.
fn database_size(config: &Config) -> i64 {
    let database = config.database_path();
    let log = database.with_extension("db-wal");

    [database, log]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len() as i64)
        .sum()
}
