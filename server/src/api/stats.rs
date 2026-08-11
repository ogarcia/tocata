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
    let row = counts(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "counting what there is"))?;

    let (
        artists,
        albums,
        tracks,
        missing,
        unreadable,
        genres,
        playlists,
        users,
        keys,
        libraries,
        size,
        duration,
    ) = row;

    Ok(Json(Stats {
        version: env!("CARGO_PKG_VERSION").to_string(),
        artists,
        albums,
        tracks,
        missing,
        unreadable,
        genres,
        playlists,
        users,
        keys,
        libraries,
        total_size: size.unwrap_or(0),
        total_duration: duration.unwrap_or(0) / 1000,
        database_size: database_size(&config),
    }))
}

/// Every figure, in one statement and one pass over the tracks rather than four.
///
/// Most of them come off a covering index and cost next to nothing. The five about
/// tracks cannot: the index on `missing_since` is partial, so it holds the side
/// expected to be empty and not the side holding everything, there is none on
/// `unreadable_since`, and the two sums read the row whatever is indexed. Asked one
/// by one they were four scans of the widest table there is — measured at 3.7
/// seconds over eleven thousand tracks on a slow machine, for the screen that opens
/// first. Asked together they are one.
///
/// An aggregate without a `GROUP BY` answers with one row however empty the table
/// is, so a collection with nothing in it still has figures to report.
///
/// Apart from the handler because this is the part worth testing on its own, and
/// reaching it through the handler would mean building a session and a
/// configuration that have nothing to do with counting.
async fn counts(pool: &SqlitePool) -> sqlx::Result<Counts> {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM artists),
                (SELECT count(*) FROM albums),
                t.present,
                t.missing,
                -- Counted apart from the figure the last scan reports, which is
                -- about that scan: scanning one library says nothing about what
                -- another one still cannot read.
                t.unreadable,
                (SELECT count(*) FROM genres),
                (SELECT count(*) FROM playlists),
                (SELECT count(*) FROM users),
                (SELECT count(*) FROM api_keys WHERE revoked_at IS NULL),
                (SELECT count(*) FROM libraries),
                t.size,
                t.duration
           FROM (SELECT count(*) FILTER (WHERE missing_since IS NULL)         AS present,
                        count(*) FILTER (WHERE missing_since IS NOT NULL)     AS missing,
                        count(*) FILTER (WHERE unreadable_since IS NOT NULL)  AS unreadable,
                        sum(file_size)   FILTER (WHERE missing_since IS NULL) AS size,
                        sum(duration_ms) FILTER (WHERE missing_since IS NULL) AS duration
                   FROM tracks) t",
    )
    .fetch_one(pool)
    .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// A collection where every figure is a different number.
    ///
    /// Deliberately, and it is the whole point of the fixture: the statement hands
    /// back twelve values of the same type in one row, and nothing about the types
    /// would notice two of them swapped. Counts that are all different turn a
    /// transposition into a failure.
    ///
    /// One library, two artists, three albums, four tracks gone, five still there,
    /// six that would not read, seven genres, eight playlists, nine keys anybody
    /// can use and ten accounts.
    async fn a_collection_of_distinct_figures() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();

        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'music', '/srv/music', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'root', '', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for id in 1..=2 {
            sqlx::query(
                "INSERT INTO artists (id, public_id, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("ar{id}"))
            .bind(format!("Artist {id}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        for id in 1..=3 {
            sqlx::query(
                "INSERT INTO albums (id, public_id, grouping_key, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("al{id}"))
            .bind(format!("key {id}"))
            .bind(format!("Album {id}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Five still there and four gone, six of the nine unreadable — spread over
        // both, because a file that would not read can go missing afterwards and
        // the two figures are counted apart.
        //
        // What is gone is heavy and long, so a sum that forgot to leave it out
        // could not pass for the right answer.
        for (id, (missing, unreadable, size, duration)) in [
            (None, None, 1, 1_000),
            (None, None, 2, 2_000),
            (None, Some("2026-05-14T09:00:00Z"), 4, 4_000),
            (None, Some("2026-05-14T09:00:00Z"), 8, 8_000),
            (None, Some("2026-05-14T09:00:00Z"), 16, 16_000),
            (Some("2026-08-01T09:00:00Z"), None, 1_000, 900_000),
            (
                Some("2026-08-01T09:00:00Z"),
                Some("2026-05-14T09:00:00Z"),
                1_000,
                900_000,
            ),
            (
                Some("2026-08-01T09:00:00Z"),
                Some("2026-05-14T09:00:00Z"),
                1_000,
                900_000,
            ),
            (
                Some("2026-08-01T09:00:00Z"),
                Some("2026-05-14T09:00:00Z"),
                1_000,
                900_000,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            sqlx::query(
                "INSERT INTO tracks (public_id, library_id, folder_id, album_id, path, title,
                                     content_type, suffix, file_size, file_modified_at,
                                     duration_ms, missing_since, unreadable_since,
                                     last_seen_scan, created_at, updated_at)
                 VALUES (?, 1, 1, 1, ?, ?, 'audio/flac', 'flac', ?, ?, ?, ?, ?, 1, ?, ?)",
            )
            .bind(format!("t{id}"))
            .bind(format!("{id}.flac"))
            .bind(format!("Track {id}"))
            .bind(size)
            .bind(&at)
            .bind(duration)
            .bind(missing)
            .bind(unreadable)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        for id in 1..=7 {
            sqlx::query("INSERT INTO genres (id, name) VALUES (?, ?)")
                .bind(id)
                .bind(format!("Genre {id}"))
                .execute(&pool)
                .await
                .unwrap();
        }

        for id in 1..=10 {
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, password_set_at, is_admin,
                                    created_at, updated_at)
                 VALUES (?, ?, 'x', ?, 0, ?, ?)",
            )
            .bind(id)
            .bind(format!("user{id}"))
            .bind(&at)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        for id in 1..=8 {
            sqlx::query(
                "INSERT INTO playlists (id, public_id, owner_id, name, created_at, updated_at)
                 VALUES (?, ?, 1, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("p{id}"))
            .bind(format!("Playlist {id}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Nine anybody can use, and two revoked that nobody can — which is the
        // difference the `WHERE` in the statement is there to make.
        for id in 1..=11 {
            sqlx::query(
                "INSERT INTO api_keys (id, user_id, key_hash, label, revoked_at, created_at)
                 VALUES (?, 1, ?, 'phone', ?, ?)",
            )
            .bind(id)
            .bind(format!("hash{id}"))
            .bind((id > 9).then(|| at.clone()))
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        pool
    }

    /// Twelve figures, twelve different numbers, each where it belongs.
    #[tokio::test]
    async fn every_figure_comes_back_in_its_own_place() {
        let pool = a_collection_of_distinct_figures().await;

        let (
            artists,
            albums,
            tracks,
            missing,
            unreadable,
            genres,
            playlists,
            users,
            keys,
            libraries,
            size,
            duration,
        ) = counts(&pool).await.unwrap();

        assert_eq!(artists, 2);
        assert_eq!(albums, 3);
        assert_eq!(tracks, 5, "what is still there");
        assert_eq!(missing, 4);
        assert_eq!(unreadable, 6, "across what is there and what is gone alike");
        assert_eq!(genres, 7);
        assert_eq!(playlists, 8);
        assert_eq!(keys, 9, "a revoked key is not one anybody can use");
        assert_eq!(users, 10);
        assert_eq!(libraries, 1);

        assert_eq!(size, Some(31), "only what is still there weighs anything");
        assert_eq!(duration, Some(31_000));
    }

    /// The figures are added up without opening the tracks table.
    ///
    /// Five aggregates over every track there is, and a track's row is thirty-odd columns
    /// with a path and a title in it while these four are numbers — so `tracks_figures_idx`
    /// covers them and the table is never read. On the machine that made this worth doing,
    /// an Atom N2800 sharing a mechanical disk with a running scan, the Overview took
    /// eleven and a half seconds.
    ///
    /// Asked of the plan rather than of a clock: a timing on a fast machine with the
    /// database in memory says nothing about that one, and what has to stay true is that
    /// this statement reads an index and not a table.
    #[tokio::test]
    async fn the_figures_are_added_up_from_an_index() {
        let pool = a_collection_of_distinct_figures().await;

        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
            "EXPLAIN QUERY PLAN
             SELECT count(*) FILTER (WHERE missing_since IS NULL),
                    count(*) FILTER (WHERE missing_since IS NOT NULL),
                    count(*) FILTER (WHERE unreadable_since IS NOT NULL),
                    sum(file_size) FILTER (WHERE missing_since IS NULL),
                    sum(duration_ms) FILTER (WHERE missing_since IS NULL)
               FROM tracks",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        let said = plan
            .into_iter()
            .map(|row| row.3)
            .collect::<Vec<_>>()
            .join(" · ");
        assert!(
            said.contains("tracks_figures_idx"),
            "the figures should come off the covering index: {said}"
        );
    }

    /// A server before its first scan. The figures come off an aggregate over an
    /// empty table, which answers with a row of nothing rather than with no row —
    /// and the difference is whether this screen opens at all.
    #[tokio::test]
    async fn a_collection_with_nothing_in_it_still_has_figures() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let (artists, albums, tracks, missing, unreadable, .., size, duration) =
            counts(&pool).await.unwrap();

        assert_eq!(
            (artists, albums, tracks, missing, unreadable),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(size, None, "nothing weighs nothing, which reads as zero");
        assert_eq!(duration, None);
    }
}
