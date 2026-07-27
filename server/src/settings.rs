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

use crate::db::now;
use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Everything the row holds, in the shape the rest of the program wants.
pub struct Settings {
    pub ignored_articles: Vec<String>,
}

/// Writes the initial row if there is none, leaving an existing one alone.
pub async fn seed(pool: &SqlitePool, ignored_articles: &[String]) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (id, ignored_articles, updated_at) VALUES (1, ?, ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(ignored_articles.join(" "))
    .bind(now())
    .execute(pool)
    .await
    .context("seeding the settings")?;

    Ok(())
}

/// Reads the settings. The row exists because seeding runs at startup.
pub async fn load(pool: &SqlitePool) -> Result<Settings> {
    let ignored_articles: String =
        sqlx::query_scalar("SELECT ignored_articles FROM settings WHERE id = 1")
            .fetch_one(pool)
            .await
            .context("reading the settings")?;

    Ok(Settings {
        ignored_articles: ignored_articles
            .split_whitespace()
            .map(str::to_string)
            .collect(),
    })
}

/// Replaces the list of articles.
pub async fn set_ignored_articles(pool: &SqlitePool, articles: &[String]) -> Result<()> {
    sqlx::query("UPDATE settings SET ignored_articles = ?, updated_at = ? WHERE id = 1")
        .bind(articles.join(" "))
        .bind(now())
        .execute(pool)
        .await
        .context("changing the ignored articles")?;

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

    /// The point of seeding only once: a restart with the variable still set
    /// must not undo what somebody chose in the panel.
    #[tokio::test]
    async fn seeding_again_leaves_a_chosen_value_alone() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();
        set_ignored_articles(&pool, &words(&["Der", "Die", "Das"]))
            .await
            .unwrap();

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
        set_ignored_articles(&pool, &[]).await.unwrap();

        assert!(load(&pool).await.unwrap().ignored_articles.is_empty());
    }

    /// The check on the primary key is what keeps this a settings row rather
    /// than a settings table.
    #[tokio::test]
    async fn a_second_row_is_refused() {
        let pool = empty().await;
        seed(&pool, &words(&["The"])).await.unwrap();

        let second = sqlx::query(
            "INSERT INTO settings (id, ignored_articles, updated_at)
                                  VALUES (2, 'x', 'x')",
        )
        .execute(&pool)
        .await;

        assert!(second.is_err());
    }
}
