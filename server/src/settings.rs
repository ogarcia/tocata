// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Settings that describe the collection rather than the deployment.
//!
//! Where the data directory is and which port to listen on describe where the
//! server runs, and they belong to whoever starts it. Which words are articles
//! describes what language the music is in, which is a property of the music.
//! Somebody with a Spanish collection should not have to edit a compose file and
//! restart a container to say so.
//!
//! The environment seeds this on first run and never again, so a value set from
//! the panel is not quietly undone by the next restart.

use crate::db::InTurn;
use crate::db::now;
use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// How a time of day is written, both in the row and in the field somebody types
/// it into. Local time: it is chosen by somebody who means "while I am asleep".
pub const HOUR_AND_MINUTE: &str = "%H:%M";

/// A quick scan every time the server starts. What Tocata did before any of this
/// was a setting, and the right thing for a server that was off all night.
const SCANS_AT_STARTUP: bool = true;

/// How long a login lasts unless somebody says otherwise.
const SESSION_DAYS: i64 = 30;

/// Everything the row holds, in the shape the rest of the program wants.
///
/// Whole rather than in pieces: it is one row of five values, so every read
/// costs the same and nothing has to decide which half it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub ignored_articles: Vec<String>,
    pub scan_at_startup: bool,
    /// `HH:MM` in local time, or nothing for no schedule.
    pub scan_at: Option<String>,
    /// Days something may stay absent before a scan clears it out, or nothing to
    /// leave that to whoever asks for a purge.
    pub absent_grace_days: Option<i64>,
    pub session_days: i64,
    /// Whether the server may go looking for pictures of the artists on
    /// somebody else's servers. Off until asked, like every other way out of
    /// this machine.
    pub fetch_portraits: bool,
}

/// Writes the initial row if there is none, leaving an existing one alone.
///
/// Only the articles are seeded from outside, because only they have an
/// environment variable behind them. The rest start at what the server used to do
/// with no setting at all, so nothing changes for a collection that already
/// exists.
pub async fn seed(pool: &SqlitePool, ignored_articles: &[String]) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (id, ignored_articles, scan_at_startup, session_days, updated_at)
              VALUES (1, ?, ?, ?, ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(ignored_articles.join(" "))
    .bind(SCANS_AT_STARTUP)
    .bind(SESSION_DAYS)
    .bind(now())
    .in_turn(pool)
    .await
    .context("seeding the settings")?;

    Ok(())
}

/// Reads the settings. The row exists because seeding runs at startup.
pub async fn load(pool: &SqlitePool) -> Result<Settings> {
    let row: (String, bool, Option<String>, Option<i64>, i64, bool) = sqlx::query_as(
        "SELECT ignored_articles, scan_at_startup, scan_at, absent_grace_days, session_days,
                fetch_portraits
           FROM settings WHERE id = 1",
    )
    .fetch_one(pool)
    .await
    .context("reading the settings")?;

    let (
        ignored_articles,
        scan_at_startup,
        scan_at,
        absent_grace_days,
        session_days,
        fetch_portraits,
    ) = row;

    Ok(Settings {
        ignored_articles: ignored_articles
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        scan_at_startup,
        scan_at,
        absent_grace_days,
        session_days,
        fetch_portraits,
    })
}

/// Writes the row back, whole.
///
/// The caller reads, changes what it was asked to change and stores the result,
/// so a partial change never has to be expressed in SQL. There is one row and one
/// administrator writing it, so the read and the write not being one statement
/// costs nothing.
pub async fn store(pool: &SqlitePool, settings: &Settings) -> Result<()> {
    sqlx::query(
        "UPDATE settings
            SET ignored_articles = ?, scan_at_startup = ?, scan_at = ?,
                absent_grace_days = ?, session_days = ?, fetch_portraits = ?,
                updated_at = ?
          WHERE id = 1",
    )
    .bind(settings.ignored_articles.join(" "))
    .bind(settings.scan_at_startup)
    .bind(&settings.scan_at)
    .bind(settings.absent_grace_days)
    .bind(settings.session_days)
    .bind(settings.fetch_portraits)
    .bind(now())
    .in_turn(pool)
    .await
    .context("changing the settings")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn empty() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    fn words(of: &[&str]) -> Vec<String> {
        of.iter().map(|w| w.to_string()).collect()
    }

    #[tokio::test]
    async fn seeding_sets_the_first_value() {
        let pool = empty().await;
        seed(&pool, &words(&["The", "La"])).await.unwrap();

        assert_eq!(load(&pool).await.unwrap().ignored_articles, ["The", "La"]);
    }

    /// What a collection that has never been told anything behaves like: the
    /// same as the server did before any of this could be chosen.
    #[tokio::test]
    async fn what_is_seeded_is_what_the_server_used_to_do() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();

        let settings = load(&pool).await.unwrap();

        assert!(settings.scan_at_startup);
        assert_eq!(settings.scan_at, None);
        assert_eq!(settings.absent_grace_days, None);
        assert_eq!(settings.session_days, 30);
    }

    /// The point of seeding only once: a restart with the variable still set
    /// must not undo what somebody chose in the panel.
    #[tokio::test]
    async fn seeding_again_leaves_a_chosen_value_alone() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();

        let mut settings = load(&pool).await.unwrap();
        settings.ignored_articles = words(&["Der", "Die", "Das"]);
        store(&pool, &settings).await.unwrap();

        seed(&pool, &words(&["The"])).await.unwrap();

        assert_eq!(
            load(&pool).await.unwrap().ignored_articles,
            ["Der", "Die", "Das"]
        );
    }

    /// Emptying the list is a real choice — a collection where no word should be
    /// skipped — and it has to survive the round trip rather than come back as
    /// one empty string.
    #[tokio::test]
    async fn no_articles_at_all_is_a_setting_too() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();

        let mut settings = load(&pool).await.unwrap();
        settings.ignored_articles = Vec::new();
        store(&pool, &settings).await.unwrap();

        assert!(load(&pool).await.unwrap().ignored_articles.is_empty());
    }

    /// Every value at once, including the two that are worth nothing at all.
    #[tokio::test]
    async fn everything_survives_the_round_trip() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();

        let chosen = Settings {
            ignored_articles: words(&["Der"]),
            scan_at_startup: false,
            scan_at: Some("04:00".to_string()),
            absent_grace_days: Some(0),
            session_days: 1,
            fetch_portraits: true,
        };
        store(&pool, &chosen).await.unwrap();

        assert_eq!(load(&pool).await.unwrap(), chosen);
    }

    /// The schema refuses a time it could never act on, so a value that got past
    /// the API by some other route cannot sit in the row waiting to be read.
    #[tokio::test]
    async fn a_time_that_is_not_one_is_refused() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();

        let mut settings = load(&pool).await.unwrap();
        settings.scan_at = Some("4am".to_string());

        assert!(store(&pool, &settings).await.is_err());
    }

    /// The check on the primary key is what keeps this a settings row rather
    /// than a settings table.
    #[tokio::test]
    async fn a_second_row_is_refused() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();

        let second = sqlx::query(
            "INSERT INTO settings (id, ignored_articles, scan_at_startup,
                                   session_days, updated_at)
                            VALUES (2, 'x', 1, 30, 'x')",
        )
        .execute(&pool)
        .await;

        assert!(second.is_err());
    }
}
