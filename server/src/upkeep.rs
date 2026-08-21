// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Keeping the collection current without being asked.
//!
//! Three things start a scan — the server coming up, the hour somebody chose,
//! and a request from the panel — and all three want the same thing to happen
//! afterwards, so they all come through here. What follows a scan is whatever the
//! settings say: today that is clearing out what has been absent long enough, and
//! anything else that belongs after a scan belongs in [`ran`].
//!
//! The schedule is checked once a minute rather than slept until, because the
//! hour can be changed from the panel while the server is running and a sleep
//! decided an hour ago would not know. It is a time of day, too, so a sleep would
//! have to answer for the clock going back an hour and for the machine suspending
//! through the appointment; looking at what time it is cannot get either wrong.
//!
//! **Looking at the clock is free; asking the database is not.** So the hour being
//! watched for comes from [`crate::settings::Current`] rather than from the row,
//! and a minute in which nothing is due now costs nothing at all. It used to cost
//! a query, which kept a connection — and so a thread — alive in a server that was
//! otherwise idle all night.
//!
//! **Nothing here reaches the network.** Looking for pictures of the artists is
//! not one of the things that follows a scan, and that is deliberate: a scan
//! runs on a schedule and at startup, so hanging it off one would mean a server
//! on an isolated network trying to reach the internet every night without
//! anybody ever having asked it to. It happens when somebody presses the button
//! and at no other time.

use crate::db;
use crate::scanner::{self, Mode};
use crate::settings;
use crate::state::AppState;
use chrono::{Duration, Local, NaiveDateTime, NaiveTime};
use std::time::Duration as Wait;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{error, info, warn};

/// How often the schedule is looked at. A minute, because that is the finest a
/// time of day written as `HH:MM` can ask for.
const TICK: Wait = Wait::from_secs(60);

/// Runs a scan, says how it went, and then does whatever the settings say should
/// follow one.
pub async fn scan(state: &AppState, mode: Mode) {
    match scanner::scan_all(&state.pool, mode, &state.scan).await {
        Ok(Some(outcome)) => {
            info!(
                "scan finished: {} folders, {} tracks ({} unchanged, {} new, {} changed), \
                 {} failed, {} gone",
                outcome.folders,
                outcome.tracks,
                outcome.unchanged,
                outcome.added,
                outcome.changed,
                outcome.failed,
                outcome.gone
            );

            ran(state).await;
        }
        // Either another scan was already running, or this one was cancelled.
        // Neither has a reliable idea of what is absent, so nothing follows it.
        Ok(None) => {}
        Err(e) => error!("scan failed: {e:#}"),
    }
}

/// What a finished scan leads to.
async fn ran(state: &AppState) {
    let pool = &state.pool;
    let data_dir = state.config.data_dir();

    let settings = match settings::load(pool).await {
        Ok(settings) => settings,
        Err(e) => return warn!("could not read the settings after a scan: {e:#}"),
    };

    // No quarantine means what a scan marks stays marked until somebody asks for
    // it to go, which is the safe default and what Tocata did before this was a
    // setting at all.
    let Some(days) = settings.absent_grace_days else {
        return;
    };

    // Zero days is "as soon as a scan finds it gone", which this expresses
    // without a case of its own: everything marked up to this moment.
    let until = db::from_now(-Duration::days(days));

    if let Err(e) = crate::purge::absent(pool, data_dir, Some(&until)).await {
        warn!("could not clear out what is absent: {e:#}");
    }
}

/// Watches the clock, and scans when it says to.
///
/// Runs until the process stops. There is nothing to cancel: it holds no
/// connection between ticks, and the scan it may be waiting on answers to the
/// same cancellation everything else does.
pub async fn on_schedule(state: AppState) {
    let mut ticker = interval(TICK);
    // A tick missed because a scan was running is not worth catching up on: the
    // scan that delayed it did the work it would have done.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first tick of an interval is immediate, and there is nothing to look at
    // yet at the moment the server starts.
    ticker.tick().await;

    let mut previous = Local::now().naive_local();

    loop {
        ticker.tick().await;

        let now = Local::now().naive_local();
        let since = std::mem::replace(&mut previous, now);

        // Cloned out of the borrow rather than read through it: what follows
        // waits on a scan, and a guard cannot be held across that.
        let at = state.settings.borrow().scan_at.clone();

        let Some(at) = at.as_deref().and_then(when) else {
            continue;
        };

        if !crossed(since, now, at) {
            continue;
        }

        if state.scan.is_scanning() {
            info!("the scheduled scan was skipped: one is already running");
            continue;
        }

        info!("starting the scheduled scan");
        scan(&state, Mode::Incremental).await;

        // From here rather than from before the scan, so that a long one does not
        // leave a window behind it wide enough to hold another appointment.
        previous = Local::now().naive_local();
    }
}

/// The hour somebody typed, or nothing if the row holds something that is not
/// one. Refused when it is written, so this only fires for a row edited by hand.
fn when(written: &str) -> Option<NaiveTime> {
    match NaiveTime::parse_from_str(written, settings::HOUR_AND_MINUTE) {
        Ok(at) => Some(at),
        Err(_) => {
            warn!("the scan schedule is not an hour and minute: {written}");
            None
        }
    }
}

/// Whether the chosen minute of the day fell in the span just gone by.
///
/// A window rather than "is it that minute now", because a tick can arrive late
/// — the machine slept, or a scan held this task for an hour — and an appointment
/// missed by a few seconds would otherwise wait a day.
///
/// Both dates are tried so that a window running over midnight catches an hour on
/// either side of it. Trying the same date twice, which is the usual case, costs
/// a comparison.
fn crossed(since: NaiveDateTime, now: NaiveDateTime, at: NaiveTime) -> bool {
    [since.date(), now.date()].iter().any(|day| {
        let appointment = day.and_time(at);

        appointment > since && appointment <= now
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(written: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(written, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn at(written: &str) -> NaiveTime {
        NaiveTime::parse_from_str(written, settings::HOUR_AND_MINUTE).unwrap()
    }

    #[test]
    fn the_minute_it_falls_in_is_the_one_that_runs() {
        assert!(crossed(
            moment("2026-08-01 03:59:30"),
            moment("2026-08-01 04:00:30"),
            at("04:00")
        ));
    }

    /// The whole point of a window: the same appointment must not run again on
    /// the next tick.
    #[test]
    fn the_minute_after_does_not_run_it_again() {
        assert!(!crossed(
            moment("2026-08-01 04:00:30"),
            moment("2026-08-01 04:01:30"),
            at("04:00")
        ));
    }

    /// A machine that slept through the appointment still keeps it.
    #[test]
    fn a_late_tick_still_catches_it() {
        assert!(crossed(
            moment("2026-08-01 03:50:00"),
            moment("2026-08-01 06:20:00"),
            at("04:00")
        ));
    }

    /// The case the second date is there for.
    #[test]
    fn a_window_over_midnight_catches_an_hour_on_either_side() {
        assert!(crossed(
            moment("2026-07-31 23:59:30"),
            moment("2026-08-01 00:00:30"),
            at("00:00")
        ));
        assert!(crossed(
            moment("2026-07-31 23:58:00"),
            moment("2026-08-01 00:01:00"),
            at("23:59")
        ));
    }

    #[test]
    fn an_hour_outside_the_window_does_nothing() {
        assert!(!crossed(
            moment("2026-08-01 03:59:30"),
            moment("2026-08-01 04:00:30"),
            at("05:00")
        ));
    }

    /// The row is checked by the schema and by the API, so this only guards
    /// against a database edited by hand — but it guards by skipping the
    /// schedule, not by panicking in a task nobody is watching.
    #[test]
    fn a_time_that_is_not_one_is_no_schedule() {
        assert!(when("halfway through the night").is_none());
        assert_eq!(when("04:00"), Some(at("04:00")));
    }
}
