// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the listener says about the music: favourites, ratings, plays.
//!
//! This is the half of the database that cannot be rebuilt by rescanning, which
//! is why it lives in its own tables and why the scanner never touches it.

use super::auth::Authenticated;
use super::error::ApiError;
use super::response::{self, Empty, Repeated};
use crate::db;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::error;

/// Highest rating the API allows. Zero is not a rating: it means remove one.
const MAX_RATING: i64 = 5;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StarQuery {
    /// Songs, or directories in the folder view. Repeats.
    #[serde(default)]
    id: Vec<String>,
    #[serde(default)]
    album_id: Vec<String>,
    #[serde(default)]
    artist_id: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingQuery {
    id: String,
    rating: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrobbleQuery {
    #[serde(default)]
    id: Vec<String>,
    /// When each song was played, in milliseconds since the epoch. Lets a client
    /// hand over plays it cached while offline with their real times.
    #[serde(default)]
    time: Vec<i64>,
    submission: Option<bool>,
}

/// Which table an id turned out to belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Track,
    Album,
    Artist,
}

pub async fn star(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<StarQuery>,
) -> Response {
    match set_starred(&pool, auth.user.id, &query, Some(db::now())).await {
        Ok(()) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "starring"),
    }
}

pub async fn unstar(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<StarQuery>,
) -> Response {
    match set_starred(&pool, auth.user.id, &query, None).await {
        Ok(()) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "unstarring"),
    }
}

pub async fn set_rating(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<RatingQuery>,
) -> Response {
    if !(0..=MAX_RATING).contains(&query.rating) {
        return ApiError::MissingParameter("rating")
            .in_format(auth.format)
            .into_response();
    }

    // Zero means "no rating", and the column only accepts one to five, so it
    // becomes a null rather than a zero.
    let rating = (query.rating > 0).then_some(query.rating);

    match write_rating(&pool, auth.user.id, &query.id, rating).await {
        Ok(true) => response::ok(auth.format, Empty {}),
        Ok(false) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => internal(e, auth.format, "setting a rating"),
    }
}

pub async fn scrobble(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<ScrobbleQuery>,
) -> Response {
    if query.id.is_empty() {
        return ApiError::MissingParameter("id")
            .in_format(auth.format)
            .into_response();
    }

    // The default is a real play, not a now-playing notification.
    let submission = query.submission.unwrap_or(true);

    for (index, id) in query.id.iter().enumerate() {
        let played_at = query
            .time
            .get(index)
            .map(|millis| db::from_epoch_millis(*millis))
            .unwrap_or_else(db::now);

        let outcome = if submission {
            record_play(&pool, auth.user.id, id, &played_at).await
        } else {
            record_now_playing(&pool, auth.user.id, &auth.client, id, &played_at).await
        };

        if let Err(e) = outcome {
            return internal(e, auth.format, "scrobbling");
        }
    }

    response::ok(auth.format, Empty {})
}

fn internal(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}

/// Sets or clears the starred mark on everything the request named.
///
/// An id that matches nothing is skipped rather than failing the call: the
/// specification defines no error for a partly valid list, and half applying the
/// request would be worse than ignoring the part that made no sense.
async fn set_starred(
    pool: &SqlitePool,
    user_id: i64,
    query: &StarQuery,
    starred_at: Option<String>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    // A bare `id` is documented as a song or a folder, but clients also send
    // album and artist ids there, so it is resolved against all three. Public
    // ids are unique per table and opaque, so there is nothing to disambiguate.
    for id in &query.id {
        if let Some((kind, internal_id)) = resolve(&mut tx, id).await? {
            write_starred(&mut tx, user_id, kind, internal_id, starred_at.as_deref()).await?;
        }
    }

    for id in &query.album_id {
        if let Some(internal_id) = resolve_as(&mut tx, Kind::Album, id).await? {
            write_starred(
                &mut tx,
                user_id,
                Kind::Album,
                internal_id,
                starred_at.as_deref(),
            )
            .await?;
        }
    }

    for id in &query.artist_id {
        if let Some(internal_id) = resolve_as(&mut tx, Kind::Artist, id).await? {
            write_starred(
                &mut tx,
                user_id,
                Kind::Artist,
                internal_id,
                starred_at.as_deref(),
            )
            .await?;
        }
    }

    tx.commit().await
}

async fn write_starred(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: i64,
    kind: Kind,
    id: i64,
    starred_at: Option<&str>,
) -> Result<(), sqlx::Error> {
    // Written out per table because sqlx will not take SQL built at runtime, and
    // the three statements differ only in the column name.
    match kind {
        Kind::Track => {
            sqlx::query(
                "INSERT INTO user_track_stats (user_id, track_id, starred_at)
                 VALUES (?, ?, ?)
                 ON CONFLICT (user_id, track_id) DO UPDATE SET starred_at = excluded.starred_at",
            )
            .bind(user_id)
            .bind(id)
            .bind(starred_at)
            .execute(&mut **tx)
            .await?;
        }
        Kind::Album => {
            sqlx::query(
                "INSERT INTO user_album_stats (user_id, album_id, starred_at)
                 VALUES (?, ?, ?)
                 ON CONFLICT (user_id, album_id) DO UPDATE SET starred_at = excluded.starred_at",
            )
            .bind(user_id)
            .bind(id)
            .bind(starred_at)
            .execute(&mut **tx)
            .await?;
        }
        Kind::Artist => {
            sqlx::query(
                "INSERT INTO user_artist_stats (user_id, artist_id, starred_at)
                 VALUES (?, ?, ?)
                 ON CONFLICT (user_id, artist_id) DO UPDATE SET starred_at = excluded.starred_at",
            )
            .bind(user_id)
            .bind(id)
            .bind(starred_at)
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(())
}

async fn write_rating(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
    rating: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let Some((kind, id)) = resolve(&mut tx, public_id).await? else {
        return Ok(false);
    };

    match kind {
        Kind::Track => {
            sqlx::query(
                "INSERT INTO user_track_stats (user_id, track_id, rating)
                 VALUES (?, ?, ?)
                 ON CONFLICT (user_id, track_id) DO UPDATE SET rating = excluded.rating",
            )
            .bind(user_id)
            .bind(id)
            .bind(rating)
            .execute(&mut *tx)
            .await?;
        }
        Kind::Album => {
            sqlx::query(
                "INSERT INTO user_album_stats (user_id, album_id, rating)
                 VALUES (?, ?, ?)
                 ON CONFLICT (user_id, album_id) DO UPDATE SET rating = excluded.rating",
            )
            .bind(user_id)
            .bind(id)
            .bind(rating)
            .execute(&mut *tx)
            .await?;
        }
        Kind::Artist => {
            sqlx::query(
                "INSERT INTO user_artist_stats (user_id, artist_id, rating)
                 VALUES (?, ?, ?)
                 ON CONFLICT (user_id, artist_id) DO UPDATE SET rating = excluded.rating",
            )
            .bind(user_id)
            .bind(id)
            .bind(rating)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(true)
}

/// Counts a play: one more for the tally, and the time it happened.
async fn record_play(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
    played_at: &str,
) -> Result<(), sqlx::Error> {
    let Some(track_id): Option<i64> =
        sqlx::query_scalar("SELECT id FROM tracks WHERE public_id = ?")
            .bind(public_id)
            .fetch_optional(pool)
            .await?
    else {
        return Ok(());
    };

    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO user_track_stats (user_id, track_id, play_count, last_played)
         VALUES (?, ?, 1, ?)
         ON CONFLICT (user_id, track_id) DO UPDATE SET
             play_count = play_count + 1,
             last_played = excluded.last_played",
    )
    .bind(user_id)
    .bind(track_id)
    .bind(played_at)
    .execute(&mut *tx)
    .await?;

    // The album tally too, so a client can sort albums by how much they get
    // played without adding up their tracks every time.
    sqlx::query(
        "INSERT INTO user_album_stats (user_id, album_id, play_count, last_played)
         SELECT ?, album_id, 1, ? FROM tracks WHERE id = ? AND album_id IS NOT NULL
         ON CONFLICT (user_id, album_id) DO UPDATE SET
             play_count = play_count + 1,
             last_played = excluded.last_played",
    )
    .bind(user_id)
    .bind(played_at)
    .bind(track_id)
    .execute(&mut *tx)
    .await?;

    // A play is a play whether or not the client also announced it beforehand,
    // and the song is no longer playing once it has been scrobbled.
    sqlx::query("DELETE FROM now_playing WHERE user_id = ? AND track_id = ?")
        .bind(user_id)
        .bind(track_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await
}

/// Records that a song is playing right now, without counting it as played.
async fn record_now_playing(
    pool: &SqlitePool,
    user_id: i64,
    client: &str,
    public_id: &str,
    started_at: &str,
) -> Result<(), sqlx::Error> {
    let Some(track_id): Option<i64> =
        sqlx::query_scalar("SELECT id FROM tracks WHERE public_id = ?")
            .bind(public_id)
            .fetch_optional(pool)
            .await?
    else {
        return Ok(());
    };

    // Keyed by user and client, so the same person listening on their phone and
    // their desktop shows up as two things playing rather than one overwriting
    // the other.
    sqlx::query(
        "INSERT INTO now_playing (user_id, client, track_id, started_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id, client) DO UPDATE SET
             track_id = excluded.track_id,
             started_at = excluded.started_at",
    )
    .bind(user_id)
    .bind(client)
    .bind(track_id)
    .bind(started_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Finds what an opaque id refers to, trying each table in turn.
async fn resolve(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    public_id: &str,
) -> Result<Option<(Kind, i64)>, sqlx::Error> {
    for kind in [Kind::Track, Kind::Album, Kind::Artist] {
        if let Some(id) = resolve_as(tx, kind, public_id).await? {
            return Ok(Some((kind, id)));
        }
    }

    Ok(None)
}

async fn resolve_as(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    kind: Kind,
    public_id: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let query = match kind {
        Kind::Track => "SELECT id FROM tracks WHERE public_id = ?",
        Kind::Album => "SELECT id FROM albums WHERE public_id = ?",
        Kind::Artist => "SELECT id FROM artists WHERE public_id = ?",
    };

    sqlx::query_scalar(query)
        .bind(public_id)
        .fetch_optional(&mut **tx)
        .await
}
