// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The files a scan could not account for, named.
//!
//! Both counts already existed and neither could be opened. The last scan says
//! "unreadable 4" and the collection says "missing 6", and somebody who wants to do
//! something about either had to go to the log — where the answer is one warning per
//! file, buried in a scan of eleven thousand.
//!
//! So this names them. It answers with two lists rather than one filtered listing,
//! because they are opposite problems: an unreadable file is on the disk and its
//! music is not in the collection, and a missing track is in the collection and its
//! file is not on the disk. What the reader owes each of them is different, which is
//! the whole argument for two.
//!
//! Nothing here deletes or repairs anything. Fixing an unreadable file happens
//! outside Tocata and the next scan picks it up; forgetting the missing ones is the
//! purge, which has its own job and its own dialogue on the same screen.

use super::error::ApiError;
use super::session::Administrator;
use crate::settings;
use crate::types::{ErrorBody, MissingTrack, NeedingAttention, UnreadableFile};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;

/// The most rows either list will hand over.
///
/// A screen for reading, not a report. The counts beside the lists are the whole
/// truth, and a collection that reaches this many has one problem to go and solve
/// rather than fifty of them to read.
const MOST: i64 = 50;

/// Files needing attention
///
/// The unreadable files and the missing tracks, with what each of them costs.
/// Administrators only: this is the state of the server's disks, not of anybody's
/// music.
#[utoipa::path(
    get,
    path = "/attention",
    tag = "maintenance",
    responses(
        (status = 200, description = "Both lists, and how long each really is",
         body = NeedingAttention),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn needing_attention(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
) -> Result<Json<NeedingAttention>, ApiError> {
    // Whole paths, joined here rather than in the panel: the library's root is not
    // something a client should have to know, and the root of one is the empty
    // string, which would leave a doubled separator at the front of every path in it.
    let unreadable: Vec<(String, String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT CASE WHEN l.path = '' THEN t.path ELSE l.path || '/' || t.path END,
                l.name, t.file_size, t.unreadable_since, t.unreadable_error
           FROM tracks t JOIN libraries l ON l.id = t.library_id
          WHERE t.unreadable_since IS NOT NULL
          ORDER BY t.unreadable_since, t.path
          LIMIT ?",
    )
    .bind(MOST)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing what could not be read"))?;

    // Oldest first, which is the order they will be cleared out in where a grace
    // period is set: the row closest to going is the row to look at first.
    let missing: Vec<MissingRow> = sqlx::query_as(
        // Credited the way the record credits it, like every other listing: a row
        // somebody is deciding the fate of should read the way it read everywhere
        // else, or it is not obviously the same song.
        "SELECT t.public_id, t.title,
                coalesce(t.artist_credit,
                  (SELECT group_concat(a.name, ', ')
                     FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
                    WHERE ta.track_id = t.id AND ta.role = 'artist')),
                al.name, coalesce(t.year, al.year),
                CASE WHEN l.path = '' THEN t.path ELSE l.path || '/' || t.path END,
                t.missing_since,
                (SELECT coalesce(sum(s.play_count), 0) FROM user_track_stats s
                  WHERE s.track_id = t.id),
                (SELECT count(*) FROM user_track_stats s
                  WHERE s.track_id = t.id AND s.rating IS NOT NULL),
                (SELECT count(*) FROM playlist_tracks p WHERE p.track_id = t.id)
           FROM tracks t
           JOIN libraries l ON l.id = t.library_id
           LEFT JOIN albums al ON al.id = t.album_id
          WHERE t.missing_since IS NOT NULL
          ORDER BY t.missing_since, t.path
          LIMIT ?",
    )
    .bind(MOST)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing what has gone missing"))?;

    let (unreadable_total, missing_total): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM tracks WHERE unreadable_since IS NOT NULL),
                (SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL)",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "counting what needs attention"))?;

    let grace_days = settings::load(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "reading how long a file may stay absent"))?
        .absent_grace_days;

    Ok(Json(NeedingAttention {
        unreadable: unreadable
            .into_iter()
            .map(|(path, library, size, since, why)| UnreadableFile {
                path,
                library,
                size,
                since,
                why,
            })
            .collect(),
        missing: missing.into_iter().map(MissingTrack::from).collect(),
        unreadable_total,
        missing_total,
        grace_days,
    }))
}

/// One missing track as the statement above reads it.
type MissingRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i32>,
    String,
    String,
    i64,
    i64,
    i64,
);

impl From<MissingRow> for MissingTrack {
    fn from(row: MissingRow) -> Self {
        let (id, title, artist, album, year, path, since, plays, raters, playlists) = row;

        Self {
            id,
            title,
            artist,
            album,
            year,
            path,
            since,
            plays,
            raters,
            playlists,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::user::User;

    /// A library with one file that will not open and one track whose file has gone,
    /// and a third that is perfectly fine so the lists have something to leave out.
    async fn a_collection_with_trouble() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        settings::seed(&pool, &[]).await.unwrap();

        let at = db::now();

        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'kept', '/srv/music', ?, ?)",
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

        sqlx::query(
            "INSERT INTO albums (id, public_id, grouping_key, name, year, created_at, updated_at)
             VALUES (1, 'b1', 'tagged\u{1f}Nick Drake\u{1f}Pink Moon', 'Pink Moon', 1972, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (1, 'ar1', 'Nick Drake', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        for (id, path, title, missing, unreadable, why) in [
            (
                1,
                "Talk Talk/03 After the Flood.flac",
                "03 After the Flood",
                None,
                Some("2026-05-14T09:00:00Z"),
                Some("Encountered an invalid item size"),
            ),
            (
                2,
                "Nick Drake/Pink Moon/01 Pink Moon.flac",
                "Pink Moon",
                Some("2026-08-01T09:00:00Z"),
                None,
                None,
            ),
            (
                3,
                "Nick Drake/Pink Moon/02 Road.flac",
                "Road",
                None,
                None,
                None,
            ),
        ] {
            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                     file_size, file_modified_at, content_type, suffix, title,
                                     missing_since, unreadable_since, unreadable_error,
                                     last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, 1, 1, 1, ?, 90596966, ?, 'audio/flac', 'flac', ?,
                         ?, ?, ?, 1, ?, ?)",
            )
            .bind(id)
            .bind(format!("t{id}"))
            .bind(path)
            .bind(&at)
            .bind(title)
            .bind(missing)
            .bind(unreadable)
            .bind(why)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO track_artists (track_id, artist_id, role, position)
                 VALUES (?, 1, 'artist', 0)",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }

        // What the missing one holds: two accounts that played it, one of whom rated
        // it, and a place in a playlist.
        for (id, name) in [(1, "ana"), (2, "beto")] {
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, is_admin, created_at, updated_at)
                 VALUES (?, ?, 'x', 1, ?, ?)",
            )
            .bind(id)
            .bind(name)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::query(
            "INSERT INTO user_track_stats (user_id, track_id, play_count, rating)
             VALUES (1, 2, 200, 5), (2, 2, 14, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO playlists (id, public_id, owner_id, name, created_at, updated_at)
             VALUES (1, 'p1', 1, 'Nocturnal', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (1, 2, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    fn an_administrator() -> Administrator {
        Administrator {
            user: User {
                id: 1,
                username: "ana".to_string(),
                is_admin: true,
            },
        }
    }

    /// Each list holds its own problem and neither holds the healthy track.
    #[tokio::test]
    async fn the_two_lists_hold_opposite_problems_and_nothing_else() {
        let pool = a_collection_with_trouble().await;

        let Json(found) = needing_attention(an_administrator(), State(pool))
            .await
            .unwrap();

        assert_eq!(found.unreadable_total, 1);
        assert_eq!(found.missing_total, 1);

        let file = &found.unreadable[0];
        assert_eq!(
            file.path, "/srv/music/Talk Talk/03 After the Flood.flac",
            "whole, because it is a path somebody is about to type at a shell"
        );
        assert_eq!(file.library, "kept");
        assert_eq!(file.since, "2026-05-14T09:00:00Z");
        assert_eq!(
            file.why.as_deref(),
            Some("Encountered an invalid item size"),
            "the reader's own words, which are cryptic and are the only true answer"
        );

        let track = &found.missing[0];
        assert_eq!(track.title, "Pink Moon");
        assert_eq!(track.artist.as_deref(), Some("Nick Drake"));
        assert_eq!(track.album.as_deref(), Some("Pink Moon"));
        assert_eq!(track.year, Some(1972));
        assert_eq!(
            track.path,
            "/srv/music/Nick Drake/Pink Moon/01 Pink Moon.flac"
        );

        // Everybody's plays, and how many people rated it rather than a score that
        // would be one of theirs.
        assert_eq!(track.plays, 214);
        assert_eq!(track.raters, 1);
        assert_eq!(track.playlists, 1);
    }

    /// Nobody but an administrator, which is what the extractor is for — and what
    /// keeps this from being a way to read the server's paths.
    #[tokio::test]
    async fn a_healthy_collection_answers_with_two_empty_lists() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        settings::seed(&pool, &[]).await.unwrap();

        let Json(found) = needing_attention(an_administrator(), State(pool))
            .await
            .unwrap();

        assert!(found.unreadable.is_empty());
        assert!(found.missing.is_empty());
        assert_eq!(found.unreadable_total, 0);
        assert_eq!(found.missing_total, 0);
        assert_eq!(found.grace_days, None, "nothing is cleared out by itself");
    }
}
