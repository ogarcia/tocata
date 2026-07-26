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

/// Current time in the shape the schema stores: ISO-8601, UTC, to the second.
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
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
