// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use std::path::Path;
use std::time::Duration;

/// How long a writer waits for the lock before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Begins a transaction that is going to write.
///
/// `BEGIN IMMEDIATE` rather than the plain `BEGIN` sqlx issues, and this is not
/// tuning. A deferred transaction takes a read lock and asks for the write lock
/// only when it first writes — and if another writer has committed in between,
/// SQLite refuses *immediately* with "database is locked" rather than waiting,
/// because waiting there could deadlock two transactions that each hold a read
/// lock and each want to write.
///
/// Which means [`BUSY_TIMEOUT`] does not cover it. That timeout is what a writer
/// waits with, and this is the one case where there is no waiting to be done: the
/// answer arrives at once and it is a failure. An immediate transaction asks for
/// the write lock up front, where the timeout applies again.
///
/// It was found through nine album covers asked for at once on a cold cache —
/// which is what a shelf of records does the first time it is opened. Seven came
/// back and two came back as 500s, and reloading fixed it, because by then seven
/// of them no longer needed writing. Every upsert in this program has the same
/// shape and so had the same fault waiting: a play counted while a scan writes, a
/// track starred, a bookmark moved.
pub async fn writing(pool: &SqlitePool) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// Current time in the shape the schema stores: ISO-8601, UTC, to the second.
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A moment either side of now, in the same shape. Negative offsets go back:
/// what a session's expiry is made of, and what "older than five minutes" is
/// compared against.
pub fn from_now(offset: chrono::Duration) -> String {
    (Utc::now() + offset).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A moment written by somebody else, in the shape the schema stores, or `None`
/// if it is not a moment at all.
///
/// Normalising to UTC is the whole job. Timestamps here are compared as text, so
/// `2026-08-26T11:00:00+02:00` would sort as though it were nine in the morning
/// UTC rather than the same instant as `09:00:00Z`, and a date would take effect
/// two hours late or early depending on which way the offset went.
pub fn timestamp_from(given: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(given)
        .ok()
        .map(|moment| moment.to_utc().to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// A moment given in milliseconds since the epoch, in the shape the schema
/// stores. Clients hand these over when scrobbling plays they cached offline.
pub fn from_epoch_millis(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Bytes behind a public identifier. Sixty-four bits of randomness: short
/// enough to live in a URL, wide enough that a library would need billions of
/// rows before a collision became thinkable, and the UNIQUE constraint catches
/// it if one ever does.
const PUBLIC_ID_BYTES: usize = 8;

/// Mints the opaque identifier clients see. Not derived from anything: a track
/// keeps it across renames and retags, which is what stops a corrected tag
/// from orphaning somebody's favourites.
pub fn public_id() -> Result<String> {
    let mut bytes = [0u8; PUBLIC_ID_BYTES];
    getrandom::fill(&mut bytes).context("reading from the system RNG")?;

    Ok(bytes.iter().fold(String::new(), |mut out, b| {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
        out
    }))
}

/// Opens the database, creating it if needed, and brings the schema up to
/// date. Migrations are embedded at compile time, so the binary carries its
/// own schema and needs nothing alongside it.
pub async fn connect(path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        // SQLite ships with foreign key enforcement disabled, which would
        // turn every reference in the schema into a comment.
        .foreign_keys(true)
        // Readers do not block the writer, which matters while a scan is
        // running and clients keep browsing.
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(BUSY_TIMEOUT);

    let pool = SqlitePool::connect_with(options)
        .await
        .with_context(|| format!("opening database at {}", path.display()))?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("applying database migrations")?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reason this function exists rather than the string being stored as
    /// given: text comparison is what decides whether a moment has passed, so an
    /// offset left in place would be read as though it were UTC and the moment
    /// would land hours away from the one that was meant.
    #[test]
    fn an_offset_becomes_the_same_instant_in_utc() {
        assert_eq!(
            timestamp_from("2026-08-26T11:00:00+02:00").unwrap(),
            "2026-08-26T09:00:00Z"
        );
    }

    #[test]
    fn what_is_already_utc_is_left_where_it_is() {
        assert_eq!(
            timestamp_from("2026-08-26T09:00:00Z").unwrap(),
            "2026-08-26T09:00:00Z"
        );
    }

    /// Sub-second precision is dropped rather than rejected: the schema keeps
    /// seconds, and refusing a moment for being too precise helps nobody.
    #[test]
    fn fractions_of_a_second_are_dropped() {
        assert_eq!(
            timestamp_from("2026-08-26T09:00:00.123456Z").unwrap(),
            "2026-08-26T09:00:00Z"
        );
    }

    #[test]
    fn what_is_not_a_moment_is_refused() {
        for given in ["", "tomorrow", "2026-08-26", "2026-13-01T00:00:00Z"] {
            assert!(timestamp_from(given).is_none(), "{given} is not a moment");
        }
    }
}
