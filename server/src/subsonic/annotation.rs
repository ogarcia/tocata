// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the listener says about the music: favourites, ratings, plays.
//!
//! This is the half of the database that cannot be rebuilt by rescanning, which
//! is why it lives in its own tables and why the scanner never touches it.

use super::asked::{Asked, Repeated};
use super::auth::Authenticated;
use super::error::ApiError;
use super::response::{self, Empty};
use crate::db;
use crate::net::Net;
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

/// What a client reports about a playback session.
///
/// `playbackRate` is not among these and is not read. It is there for a server that
/// estimates where the listener is between reports; nothing here estimates, because
/// nothing here has to — what counts a play is the position the client sends when it
/// stops, not a clock kept on this side. A field taken and never used would be a
/// promise this endpoint does not keep.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportQuery {
    media_id: String,
    media_type: Media,
    position_ms: i64,
    state: Doing,
    ignore_scrobble: Option<bool>,
}

/// What the id refers to. A value that is neither is answered as the unreadable
/// parameter it is, and serde names the two it could have been.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Media {
    Song,
    Podcast,
}

/// What the player is doing, in its own words: `starting`, `playing`, `paused` or
/// `stopped`. Anything else is answered as the unreadable parameter it is, and serde
/// names the four it could have been.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Doing {
    Starting,
    Playing,
    Paused,
    Stopped,
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
    State(net): State<Net>,
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
        if submission {
            let played_at = query
                .time
                .get(index)
                .map(|millis| db::from_epoch_millis(*millis))
                .unwrap_or_else(db::now);

            // Which queues it for whoever this person scrobbles to, as part of
            // counting it. Every play goes through there.
            if let Err(e) = crate::plays::record_play(&pool, auth.user.id, id, &played_at).await {
                return internal(e, auth.format, "scrobbling");
            }
        } else {
            // An announcement is about now, whatever time came with it. A play can
            // be handed over late and keep the time it happened, but "now playing"
            // is a claim about the present, and it is the one the window that
            // forgets it is measured from: a client that named a time of its own
            // could otherwise arrange to be playing something for ever.
            match crate::plays::record_now_playing(
                &pool,
                auth.user.id,
                &auth.client,
                id,
                &db::now(),
            )
            .await
            {
                // Sent from a task of its own, unlike a play, which only ever
                // writes a row here. This one does go out over the wire, and what
                // the client is waiting for is an acknowledgement of its own
                // announcement rather than a round trip through somebody else's
                // server — which may be a machine that is off.
                Ok(Some(track_id)) => {
                    let (net, pool, user_id) = (net.clone(), pool.clone(), auth.user.id);

                    tokio::spawn(async move {
                        crate::scrobble::announce(&net, &pool, user_id, track_id).await;
                    });
                }
                // Nothing to announce, which is what an unknown id comes to.
                Ok(None) => {}
                Err(e) => return internal(e, auth.format, "scrobbling"),
            }
        }
    }

    response::ok(auth.format, Empty {})
}

/// What a player is doing, reported as it happens.
///
/// The difference from `scrobble` is who decides. A scrobble is a client saying
/// "this was played", having kept its own clock; this is a client saying where it is
/// and leaving the judgement here. Which is the point of it: a client that reports a
/// timeline and never scrobbles would otherwise show up nowhere and count nothing.
///
/// So the reports are read as follows, and no further:
///
/// - **starting** and **playing** say that this is what is on. Both are written down
///   as now playing, rather than waiting for `playing` as a server keeping a clock
///   would: nothing here counts time, so a moment of buffering costs nothing, and a
///   listener who pressed play expects to appear at once.
/// - **paused** leaves what was written alone. It does not refresh it either — the
///   row expires on its own once the song could no longer be running, and a pause
///   long enough to reach that is a pause worth dropping out of the display.
/// - **stopped** is where a play is counted or is not, and then the announcement
///   goes: either way this client has stopped and is no longer playing anything.
///
/// A play counts on stop when the position reached [`crate::plays::counts_as_played`]
/// — four minutes, or half a shorter song — and `ignoreScrobble` was not asked for.
/// That flag is the client saying "show this, do not count it", which is exactly what
/// it gets: the display is refreshed and nothing is tallied or passed on.
pub async fn report_playback(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    State(net): State<Net>,
    Asked(query): Asked<ReportQuery>,
) -> Response {
    // A podcast episode is a thing this server does not have, so its id names
    // nothing here — which is what a 70 says.
    if query.media_type == Media::Podcast {
        return ApiError::NotFound.in_format(auth.format).into_response();
    }

    let duration = match duration_of(&pool, &query.media_id).await {
        Ok(duration) => duration,
        Err(e) => return internal(e, auth.format, "reading how long a song is"),
    };

    match what_it_comes_to(&query, duration) {
        Outcome::LetItStand => response::ok(auth.format, Empty {}),

        // Counting a play clears the announcement of it by itself, and passes it on
        // to whoever this person scrobbles to.
        Outcome::Played => {
            match crate::plays::record_play(&pool, auth.user.id, &query.media_id, &db::now()).await
            {
                Ok(()) => response::ok(auth.format, Empty {}),
                Err(e) => internal(e, auth.format, "counting a reported play"),
            }
        }

        Outcome::StoppedShort => {
            match crate::plays::forget_now_playing(&pool, auth.user.id, &auth.client).await {
                Ok(()) => response::ok(auth.format, Empty {}),
                Err(e) => internal(e, auth.format, "forgetting what was playing"),
            }
        }

        Outcome::NowPlaying => {
            match crate::plays::record_now_playing(
                &pool,
                auth.user.id,
                &auth.client,
                &query.media_id,
                &db::now(),
            )
            .await
            {
                // Passed on from a task of its own, for the reason the scrobble above
                // gives: what the client waits on is this server, not somebody else's.
                Ok(Some(track_id)) => {
                    let (net, pool, user_id) = (net.clone(), pool.clone(), auth.user.id);

                    tokio::spawn(async move {
                        crate::scrobble::announce(&net, &pool, user_id, track_id).await;
                    });

                    response::ok(auth.format, Empty {})
                }
                // An id nothing answers to. Unlike the scrobble, which takes a list
                // and skips what it cannot place, this call names one thing.
                Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
                Err(e) => internal(e, auth.format, "reporting a playback"),
            }
        }
    }
}

/// What one report amounts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Write down that this is what is on.
    NowPlaying,
    /// Count it, which also takes it off what is on.
    Played,
    /// Take it off what is on without counting it.
    StoppedShort,
    /// Leave everything as it is.
    LetItStand,
}

/// The whole of the reading, in one place and with no database in reach.
///
/// It is a table of four states and two flags, which is small enough to get wrong
/// quietly and small enough to check exhaustively. Kept apart from the handler for
/// the second reason.
fn what_it_comes_to(query: &ReportQuery, duration_ms: Option<i64>) -> Outcome {
    match query.state {
        Doing::Starting | Doing::Playing => Outcome::NowPlaying,

        // A pause is not a stop and not a play. The row written earlier stands and
        // is not refreshed either: it expires once the song could no longer be
        // running, and a pause that long has earned its way out of the display.
        Doing::Paused => Outcome::LetItStand,

        Doing::Stopped => {
            let counts = !query.ignore_scrobble.unwrap_or(false)
                && crate::plays::counts_as_played(query.position_ms, duration_ms);

            if counts {
                Outcome::Played
            } else {
                Outcome::StoppedShort
            }
        }
    }
}

/// How long a song runs, or nothing if the file never said.
async fn duration_of(pool: &SqlitePool, public_id: &str) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT duration_ms FROM tracks WHERE public_id = ?")
        .bind(public_id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
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
    let mut tx = crate::db::writing(pool).await?;

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
    let mut tx = crate::db::writing(pool).await?;

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
            .execute(&mut **tx)
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
            .execute(&mut **tx)
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
            .execute(&mut **tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(true)
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

#[cfg(test)]
mod report_tests {
    use super::*;

    /// Three minutes, which is the length of nearly every song there is.
    const THREE_MINUTES: Option<i64> = Some(180_000);

    fn reported(state: Doing, position_ms: i64, ignore_scrobble: Option<bool>) -> ReportQuery {
        ReportQuery {
            media_id: "trk1".to_string(),
            media_type: Media::Song,
            position_ms,
            state,
            ignore_scrobble,
        }
    }

    /// Where the whole thing is decided: four states, and a stop that either counts
    /// or does not. Read wrongly, none of it fails — it either counts plays nobody
    /// made or loses the ones they did, and both are quiet.
    #[test]
    fn what_a_report_comes_to() {
        for state in [Doing::Starting, Doing::Playing] {
            assert_eq!(
                what_it_comes_to(&reported(state, 0, None), THREE_MINUTES),
                Outcome::NowPlaying,
                "{state:?} is what is on"
            );
        }

        assert_eq!(
            what_it_comes_to(&reported(Doing::Paused, 30_000, None), THREE_MINUTES),
            Outcome::LetItStand,
            "a pause changes nothing either way"
        );

        assert_eq!(
            what_it_comes_to(&reported(Doing::Stopped, 95_000, None), THREE_MINUTES),
            Outcome::Played,
            "past half of it, so it was played"
        );

        assert_eq!(
            what_it_comes_to(&reported(Doing::Stopped, 20_000, None), THREE_MINUTES),
            Outcome::StoppedShort,
            "twenty seconds in is not a play, and not still playing either"
        );
    }

    /// The flag is the client saying "show it, do not count it", and it is obeyed
    /// however long the listen was.
    #[test]
    fn a_report_that_asks_not_to_be_counted_is_not() {
        assert_eq!(
            what_it_comes_to(
                &reported(Doing::Stopped, 175_000, Some(true)),
                THREE_MINUTES
            ),
            Outcome::StoppedShort,
            "all but the last seconds of it, and asked not to count"
        );

        assert_eq!(
            what_it_comes_to(
                &reported(Doing::Stopped, 175_000, Some(false)),
                THREE_MINUTES
            ),
            Outcome::Played,
            "and asked plainly, it counts"
        );
    }

    /// An hour-long mix is not half listened to in half an hour of anybody's
    /// patience, which is why the rule has a ceiling as well as a half.
    #[test]
    fn a_long_recording_counts_after_four_minutes() {
        let an_hour = Some(3_600_000);

        assert_eq!(
            what_it_comes_to(&reported(Doing::Stopped, 250_000, None), an_hour),
            Outcome::Played,
            "four minutes of it is a listen"
        );

        assert_eq!(
            what_it_comes_to(&reported(Doing::Stopped, 200_000, None), an_hour),
            Outcome::StoppedShort,
            "and three is not"
        );
    }

    /// A file that never said how long it is: the four minutes are all the rule has
    /// left, and half of nothing cannot be worked out.
    #[test]
    fn a_song_of_unknown_length_is_judged_by_the_four_minutes() {
        assert_eq!(
            what_it_comes_to(&reported(Doing::Stopped, 241_000, None), None),
            Outcome::Played
        );
        assert_eq!(
            what_it_comes_to(&reported(Doing::Stopped, 239_000, None), None),
            Outcome::StoppedShort
        );
    }
}
