// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Emptying the queue.
//!
//! Runs on its own, every minute, for as long as the process lives, and asks one
//! question: is anything due. Almost always the answer is nothing and the whole
//! pass is an index lookup against an empty result.
//!
//! **The waiting is per destination and not per listen.** When a service will not
//! take something, everything queued for that service is pushed out together —
//! one attempt per service per pass, in a batch. The alternative asks a machine
//! that is off the same question forty times in a minute and learns nothing new
//! each time.
//!
//! **Nothing is discarded for failing**, with one exception. A listen that could
//! not be delivered is a listen still worth delivering: services come back, and
//! the wait grows to hours rather than the queue giving up on somebody's history.
//! The exception is a listen the far end has *refused* — a 400, meaning it will
//! not take this listen however often it is offered — because keeping that is
//! keeping a row that can only ever fail again.

use super::{Destination, Listen, Service, listenbrainz, named};
use crate::db;
use crate::net::Net;
use anyhow::{Context, Result};
use chrono::Duration;
use sqlx::SqlitePool;
use std::time::Duration as Wait;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};

/// How often the queue is looked at. A minute, which is also the shortest wait a
/// failure can lead to: looking more often than the soonest retry could arrive
/// would be asking a question whose answer cannot have changed.
const TICK: Wait = Wait::from_secs(60);

/// How long to wait after a destination has failed, by how many times running it
/// has failed.
///
/// A minute for the blip, and then out to six hours for the service that has been
/// down since yesterday — which is still four attempts a day, so a week of
/// listening comes back on its own once the machine does.
fn later(attempts: i64) -> Duration {
    match attempts {
        ..=1 => Duration::minutes(1),
        2 => Duration::minutes(5),
        3 => Duration::minutes(15),
        4 => Duration::minutes(30),
        5 => Duration::hours(1),
        6 => Duration::hours(3),
        _ => Duration::hours(6),
    }
}

/// Watches the queue, and empties it when there is anything in it.
pub async fn as_they_come(net: Net, pool: SqlitePool) {
    let mut ticker = interval(TICK);
    // A pass delayed by a slow service is not worth catching up on: the next one
    // will find whatever this one left.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        if let Err(e) = pass(&net, &pool).await {
            warn!("could not send the queued listens: {e:#}");
        }
    }
}

/// One look at the queue: every destination with something due, in turn.
async fn pass(net: &Net, pool: &SqlitePool) -> Result<()> {
    let now = db::now();

    let due: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT user_id, service FROM scrobble_queue WHERE next_try_at <= ?",
    )
    .bind(&now)
    .fetch_all(pool)
    .await
    .context("looking for listens to send")?;

    for (user_id, service) in due {
        // A service this version no longer has. Its rows stay where they are: the
        // program decides what can be sent, and a downgrade should not throw
        // somebody's listening history away.
        let Some(service) = named(&service) else {
            continue;
        };

        if let Err(e) = hand_over(net, pool, user_id, service).await {
            warn!("sending to {}: {e:#}", service.name());
        }
    }

    Ok(())
}

/// A row of the queue, as it comes back. The time is read as seconds since the
/// epoch by the database, because that is the shape the wire wants and SQLite can
/// say it in the same breath as it reads the row.
type Queued = (
    i64,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    i64,
);

/// Everything due for one destination, offered in one batch.
async fn hand_over(net: &Net, pool: &SqlitePool, user_id: i64, service: Service) -> Result<()> {
    let now = db::now();

    // Read again per destination rather than once for all of them: a token
    // changed, or a destination switched off, between one pass and the next is
    // exactly the case where reading it late is reading it right.
    let found: Option<(String, String)> = sqlx::query_as(
        "SELECT s.url, s.token
           FROM scrobblers s
           JOIN users u ON u.id = s.user_id
          WHERE s.user_id = ? AND s.service = ?
            AND s.enabled = 1 AND u.scrobbling_enabled = 1",
    )
    .bind(user_id)
    .bind(service.name())
    .fetch_optional(pool)
    .await
    .context("reading a destination")?;

    // Switched off since it was queued. The rows stay: switching it on again is
    // meant to send what was waiting, which is the difference between switching
    // off and removing.
    let Some((url, token)) = found else {
        return Ok(());
    };

    let rows: Vec<Queued> = sqlx::query_as(
        "SELECT id, unixepoch(played_at), title, artist, album, mbid_recording,
                mbid_release, mbid_artist, isrc, track_number, duration_ms, attempts
           FROM scrobble_queue
          WHERE user_id = ? AND service = ? AND next_try_at <= ?
          ORDER BY played_at
          LIMIT ?",
    )
    .bind(user_id)
    .bind(service.name())
    .bind(&now)
    .bind(listenbrainz::AT_ONCE as i64)
    .fetch_all(pool)
    .await
    .context("reading the queued listens")?;

    if rows.is_empty() {
        return Ok(());
    }

    let attempts = rows.iter().map(|row| row.11).max().unwrap_or(0);
    let ids: Vec<i64> = rows.iter().map(|row| row.0).collect();
    let listens: Vec<Listen> = rows.into_iter().map(read).collect();

    let destination = Destination {
        user_id,
        service,
        url,
        token,
    };

    match offer(net, &destination, &listens).await {
        Verdict::Taken => {
            done(pool, &ids).await?;
            info!(
                "sent {} listen(s) to {}",
                ids.len(),
                destination.service.name()
            );
        }
        // Refused for what it is rather than for when it arrived. Kept would mean
        // offering the same rejected listen every six hours for ever.
        Verdict::Refused(why) => {
            done(pool, &ids).await?;
            warn!(
                "{} refused {} listen(s) and they have been dropped: {why}",
                destination.service.name(),
                ids.len()
            );
        }
        Verdict::NotYet { wait, why } => {
            let wait = wait.unwrap_or_else(|| later(attempts + 1));
            // Counted against the destination and not against the listens: being
            // told to slow down is not a failure to deliver, and letting it drive
            // the backoff would push a busy service out to six hours.
            postpone(pool, &destination, &ids, wait, why.as_deref(), false).await?;
        }
        Verdict::Failed(why) => {
            postpone(
                pool,
                &destination,
                &ids,
                later(attempts + 1),
                Some(&why),
                true,
            )
            .await?;
            warn!(
                "{} did not take {} listen(s): {why}",
                destination.service.name(),
                ids.len()
            );
        }
    }

    Ok(())
}

/// A row as a listen.
fn read(row: Queued) -> Listen {
    let (
        _,
        at,
        title,
        artist,
        album,
        mbid_recording,
        mbid_release,
        mbid_artist,
        isrc,
        track_number,
        duration_ms,
        _,
    ) = row;

    Listen {
        at,
        title,
        artist,
        album,
        mbid_recording,
        mbid_release,
        mbid_artist,
        isrc,
        track_number,
        duration_ms,
    }
}

/// What came of offering a batch.
enum Verdict {
    /// Accepted. The rows can go.
    Taken,
    /// The far end will not have these listens, whenever they are offered.
    Refused(String),
    /// Not now: too many requests, and possibly how long to wait.
    NotYet {
        wait: Option<Duration>,
        why: Option<String>,
    },
    /// Something else — a machine that is off, a token that is wrong, a bad
    /// gateway. All worth trying again later.
    Failed(String),
}

/// Offers a batch and reads the answer.
async fn offer(net: &Net, destination: &Destination, listens: &[Listen]) -> Verdict {
    let json = match listenbrainz::submission(listens) {
        Ok(json) => json,
        // Ours to fix, not theirs to refuse, so it is not a refusal: it would be
        // wrong to drop somebody's listens over a bug in writing them out.
        Err(e) => return Verdict::Failed(format!("the listens could not be written out: {e}")),
    };

    let url = listenbrainz::submitting(&destination.service.root(&destination.url));

    let answer = match net.post(&url, &destination.token, json).await {
        Ok(answer) => answer,
        Err(e) => return Verdict::Failed(format!("{e:#}")),
    };

    let said = answer.body().trim().chars().take(200).collect::<String>();

    match answer.status {
        status if (200..300).contains(&status) => Verdict::Taken,
        400 => Verdict::Refused(said),
        429 => Verdict::NotYet {
            wait: answer
                .seconds(listenbrainz::RESET_IN)
                .or_else(|| answer.seconds("retry-after"))
                .map(|seconds| Duration::seconds(seconds as i64)),
            why: Some(said),
        },
        status => Verdict::Failed(format!("{status} {said}")),
    }
}

/// Takes delivered listens out of the queue.
///
/// One statement per row rather than a list built into the SQL, because sqlx takes
/// no SQL assembled at runtime — and fifty deletes inside one transaction is one
/// commit either way.
async fn done(pool: &SqlitePool, ids: &[i64]) -> Result<()> {
    let mut tx = db::writing(pool).await.context("emptying the queue")?;

    for id in ids {
        sqlx::query("DELETE FROM scrobble_queue WHERE id = ?")
            .bind(id)
            .execute(&mut **tx)
            .await
            .context("removing a sent listen")?;
    }

    tx.commit().await.context("emptying the queue")
}

/// Pushes a destination's listens out into the future.
///
/// Everything due for that destination moves, not only the batch that was offered:
/// the next pass would otherwise pick up the next fifty and ask a machine that is
/// off all over again, a minute later.
///
/// Only the batch that was actually offered has its count raised, since that is
/// what the count is of. `counts` is false for a rate limit, which is the far end
/// managing its own load rather than anything failing.
async fn postpone(
    pool: &SqlitePool,
    destination: &Destination,
    ids: &[i64],
    wait: Duration,
    why: Option<&str>,
    counts: bool,
) -> Result<()> {
    let next = db::from_now(wait);
    let mut tx = db::writing(pool).await.context("holding the queue back")?;

    sqlx::query(
        "UPDATE scrobble_queue SET next_try_at = ?
          WHERE user_id = ? AND service = ? AND next_try_at <= ?",
    )
    .bind(&next)
    .bind(destination.user_id)
    .bind(destination.service.name())
    .bind(db::now())
    .execute(&mut **tx)
    .await
    .context("holding a destination back")?;

    for id in ids {
        sqlx::query(
            "UPDATE scrobble_queue
                SET attempts = attempts + ?, last_error = ?
              WHERE id = ?",
        )
        .bind(i64::from(counts))
        .bind(why)
        .bind(id)
        .execute(&mut **tx)
        .await
        .context("writing down why a listen is waiting")?;
    }

    tx.commit().await.context("holding the queue back")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the wait rather than its exact values: it has to grow, and it
    /// has to stop growing, because neither a fixed minute nor a doubling that
    /// runs to weeks is what a service coming back wants.
    #[test]
    fn the_wait_grows_and_then_stops() {
        assert_eq!(later(1), Duration::minutes(1));
        assert!(later(3) > later(2));
        assert!(later(5) > later(4));
        assert_eq!(later(9), later(40), "it stops at six hours");
        assert_eq!(later(40), Duration::hours(6));
    }

    /// A count that somehow arrived at zero, or below it, is still a first
    /// attempt: what must not happen is a wait of nothing, which would ask a
    /// service that is off once a pass for ever.
    #[test]
    fn an_impossible_count_still_waits() {
        assert_eq!(later(0), Duration::minutes(1));
        assert_eq!(later(-3), Duration::minutes(1));
    }
}
