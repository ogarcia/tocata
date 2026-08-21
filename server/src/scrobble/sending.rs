// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Emptying the queue.
//!
//! Runs on its own for as long as the process lives, and **sleeps until there is
//! something to do** rather than asking every minute whether there is. Two things
//! can give it work: a listen queued, which happens when somebody plays a song,
//! and a retry coming due, which the queue itself knows the hour of. So the wait
//! is either until that hour or until somebody says a row has arrived, and a
//! server nobody is listening to asks the database nothing at all.
//!
//! That last part is the whole of why this shape was chosen. A question asked
//! every minute keeps a connection to SQLite alive for ever, and a connection is
//! a thread with a two megabyte stack; a server in the small hours was paying for
//! it to ask whether anybody had listened to anything while nobody was awake.
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
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration as Wait;
use tokio::sync::Notify;
use tracing::{info, warn};

/// The shortest wait between two passes, and the shortest a failure can lead to.
///
/// It is a floor rather than a period. Nothing waits for it when there is nothing
/// to do — that wait has no end and is ended by [`there_is_one`] — and nothing
/// waits longer than the hour the queue itself names. What it bounds is how often
/// a pass can follow a pass: a row left due by a write that failed would otherwise
/// be picked up, failed on and picked up again as fast as the loop could turn.
const SOONEST: Wait = Wait::from_secs(60);

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

/// Somebody to tell that a row has arrived, one per database.
///
/// **Keyed by the file, for the same reason the write queue is** — see
/// [`crate::db`], which learned it the hard way. What waits here waits on one
/// database, so two of them in a process have nothing to say to each other, and
/// the test suite opens hundreds. A database held in memory has a name of its own
/// too, so the tests are kept apart without asking.
static ARRIVALS: Mutex<BTreeMap<PathBuf, Arc<Notify>>> = Mutex::new(BTreeMap::new());

/// The bell for one database, hung the first time somebody reaches for it.
///
/// Reachable from the rest of [`super`] and no further: the two writes that make a
/// row due ring it through [`there_is_one`], and the tests beside them are the ones
/// that can say whether a write did.
pub(super) fn arrivals(pool: &SqlitePool) -> Arc<Notify> {
    let name = pool.connect_options().get_filename().to_path_buf();

    ARRIVALS
        .lock()
        .expect("the arrivals registry is never poisoned")
        .entry(name)
        .or_insert_with(|| Arc::new(Notify::new()))
        .clone()
}

/// Says that the queue has something in it that it did not have before.
///
/// Called by whatever put it there, and it has to be: this is what the sender is
/// waiting on when the queue is empty, so a row written without ringing it would
/// sit there until the next one arrived. Both callers are in [`super`], next to
/// the statements that make a row due.
///
/// Cheap enough to call whether or not anybody is listening, and safe either way:
/// a notification with nobody waiting is kept, so a listen queued while a pass is
/// already running is not lost — the pass that follows finds it.
pub(super) fn there_is_one(pool: &SqlitePool) {
    arrivals(pool).notify_one();
}

/// Empties the queue whenever there is anything in it, and waits the rest of the
/// time.
///
/// The wait comes in three shapes, and which one it is says everything about what
/// the server is doing:
///
/// - **Nothing queued** — the wait has no end. Only [`there_is_one`] ends it, and
///   that means somebody played a song. This is what a quiet server does all
///   night, and it costs one sleeping task and no database at all.
/// - **Something due later** — a retry, so the wait is until its hour, which the
///   queue is asked for rather than guessed at. A listen queued meanwhile cuts
///   the wait short, because it is due now and the retry is not.
/// - **Something due now** — a pass, and then at least [`SOONEST`] before the
///   next one.
pub async fn as_they_come(net: Net, pool: SqlitePool) {
    let arrived = arrivals(&pool);

    loop {
        let wait = match due_in(&pool).await {
            // Nothing to send and nothing to wait for but a listen.
            Ok(None) => {
                arrived.notified().await;
                continue;
            }
            Ok(Some(seconds)) if seconds > 0 => {
                // Whatever the queue says, and not a minute more. Slept on the
                // monotonic clock, so a machine that suspends for the afternoon
                // comes back owing the wait it was in the middle of rather than
                // having served it — which is the right way round for a retry:
                // late is a queue doing its job, early is asking a service that
                // is still down.
                Wait::from_secs(seconds as u64)
            }
            // Due now, or overdue.
            Ok(Some(_)) => {
                if let Err(e) = pass(&net, &pool).await {
                    warn!("could not send the queued listens: {e:#}");
                }

                SOONEST
            }
            Err(e) => {
                warn!("could not tell when the next listen is due: {e:#}");
                SOONEST
            }
        };

        // Either the wait runs out or a listen arrives, and the top of the loop
        // works out which of them it was by asking the queue again. Nothing is
        // decided here, so a wait cut short by an arrival cannot get the answer
        // wrong.
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = arrived.notified() => {}
        }
    }
}

/// How long until the queue has something to hand over: none if it is empty, zero
/// or less if something is due already.
///
/// Counted by SQLite rather than here, the way the queue's own times are read: the
/// row holds an hour written the way people write them and the answer wanted is a
/// number of seconds, and asking for the subtraction is one less place where two
/// clocks could disagree about what `now` means.
///
/// The soonest hour of a whole queue looks like a question that reads all of it and
/// is not one: `scrobble_queue_due_idx` leads with this column, so the answer is
/// the front of the index and the rows are never opened. Held there by
/// `the_soonest_hour_is_not_a_search`.
async fn due_in(pool: &SqlitePool) -> Result<Option<i64>> {
    let (seconds,): (Option<i64>,) =
        sqlx::query_as("SELECT unixepoch(MIN(next_try_at)) - unixepoch('now') FROM scrobble_queue")
            .fetch_one(pool)
            .await
            .context("looking for when the next listen is due")?;

    Ok(seconds)
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
    let found: Option<(String, String)> = sqlx::query_as(concat!(
        "SELECT s.url, s.token
           FROM scrobblers s
           JOIN users u ON u.id = s.user_id
          WHERE s.user_id = ? AND s.service = ?
            AND s.enabled = 1 AND u.scrobbling_enabled = 1
            AND ",
        reaching_out!()
    ))
    .bind(user_id)
    .bind(service.name())
    .fetch_optional(pool)
    .await
    .context("reading a destination")?;

    // Switched off since it was queued, here or at the server's own way out. The
    // rows stay: switching it on again is meant to send what was waiting, which is
    // the difference between switching off and removing.
    //
    // And nothing is counted as an attempt, so a week off the network does not push
    // every destination out to the six-hour wait it would then have to climb back
    // down from.
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
    use tokio::time::timeout;

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
    /// An empty queue answers "nothing", which is what turns the wait into one
    /// with no end. A number here — even a large one — would be a server asking
    /// again on a timer, which is the thing this stopped doing.
    #[tokio::test]
    async fn an_empty_queue_is_nothing_to_wait_for() {
        let pool = a_queue().await;

        assert_eq!(due_in(&pool).await.unwrap(), None);
    }

    /// A listen just queued is due at once, so what comes back is not in the
    /// future: the loop passes rather than sleeping.
    #[tokio::test]
    async fn a_listen_that_has_arrived_is_due_now() {
        let pool = a_queue().await;

        waiting(&pool, &db::now()).await;

        assert!(due_in(&pool).await.unwrap().is_some_and(|due| due <= 0));
    }

    /// The hour is read from the queue rather than guessed at, and the soonest one
    /// is the one that matters: a retry due in five minutes must not be waited out
    /// behind one due in six hours.
    #[tokio::test]
    async fn the_wait_is_until_the_soonest_of_them() {
        let pool = a_queue().await;

        waiting(&pool, &db::from_now(Duration::hours(6))).await;
        waiting(&pool, &db::from_now(Duration::minutes(5))).await;

        let due = due_in(&pool).await.unwrap().expect("something is queued");
        assert!(
            (240..=300).contains(&due),
            "five minutes away, not six hours: {due}"
        );
    }

    /// The loop itself, and the whole of what changed: it is woken by the bell and
    /// not by a clock.
    ///
    /// A pass is what proves it. The attempt fails — there is nothing at the other
    /// end — and failing is what writes `attempts`, so a row that has been tried
    /// is a row the loop reached. Well inside the minute the old shape would have
    /// waited, and inside the floor a pass now waits *after* one, so neither could
    /// be what let it through.
    #[tokio::test]
    async fn a_listen_arriving_is_what_wakes_it() {
        let pool = a_queue().await;
        let sending = tokio::spawn(as_they_come(Net::new(), pool.clone()));

        // Asleep with no wait set: the queue was empty when it looked.
        tokio::time::sleep(Wait::from_millis(50)).await;
        assert_eq!(tried(&pool).await, None, "nothing to have tried yet");

        waiting(&pool, &db::now()).await;
        there_is_one(&pool);

        let woke = timeout(Wait::from_secs(5), async {
            while tried(&pool).await != Some(1) {
                tokio::time::sleep(Wait::from_millis(20)).await;
            }
        })
        .await;

        sending.abort();
        assert!(woke.is_ok(), "the listen sat in the queue: nothing woke it");
    }

    /// How many times the one row in the queue has been offered, or none if there
    /// is no row.
    async fn tried(pool: &SqlitePool) -> Option<i64> {
        sqlx::query_scalar("SELECT attempts FROM scrobble_queue LIMIT 1")
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    /// A backlog is somebody's whole listening history that has not gone out yet, and
    /// this is asked every time the sender wakes. Reading it to find the front of it
    /// would make a long queue expensive to be patient with.
    #[tokio::test]
    async fn the_soonest_hour_is_not_a_search() {
        let pool = a_queue().await;
        waiting(&pool, &db::now()).await;

        let (plan,): (String,) = sqlx::query_as(
            "EXPLAIN QUERY PLAN
             SELECT unixepoch(MIN(next_try_at)) - unixepoch('now') FROM scrobble_queue",
        )
        .fetch_one(&pool)
        .await
        .map(|row: (i64, i64, i64, String)| (row.3,))
        .unwrap();

        assert!(
            plan.contains("scrobble_queue_due_idx"),
            "the queue is being read to find the front of it: {plan}"
        );
    }

    /// A database with somebody in it, and somewhere for their listens to go: the
    /// queue hangs off a destination, so a row cannot exist without one.
    async fn a_queue() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        crate::settings::seed(&pool, &[]).await.unwrap();
        sqlx::query("UPDATE settings SET reach_out = 1 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at, updated_at)
             VALUES (1, 'ana', 'x', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        // A port nothing listens on, so an attempt fails at once rather than
        // waiting out a name that does not resolve. What the tests here are about
        // is when an attempt happens, not what the far end said.
        sqlx::query(
            "INSERT INTO scrobblers (user_id, service, url, token, enabled, created_at, updated_at)
             VALUES (1, 'listenbrainz', 'http://127.0.0.1:1', 'tok', 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    /// One row of the queue, due when it says. Written straight rather than through
    /// [`super::queue`], because what is being tested is the reading.
    async fn waiting(pool: &SqlitePool, due: &str) {
        sqlx::query(
            "INSERT INTO scrobble_queue
                  (user_id, service, played_at, title, artist, attempts, next_try_at, created_at)
             VALUES (1, 'listenbrainz', ?, 'Song', 'Artist', 0, ?, ?)",
        )
        .bind(db::now())
        .bind(due)
        .bind(db::now())
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn an_impossible_count_still_waits() {
        assert_eq!(later(0), Duration::minutes(1));
        assert_eq!(later(-3), Duration::minutes(1));
    }
}
