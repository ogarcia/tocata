// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How much of the collection somebody has marked as theirs.
//!
//! The listings themselves are not here: a favourite is not a fifth kind of thing to
//! list, it is the collection narrowed by who is reading it, so `/tracks`, `/albums`
//! and `/artists` answer for them with `starred=true` — which is also what makes
//! searching your own favourites, and playing them, the endpoints that already exist.
//!
//! What is here is the one question those listings cannot answer between them: how
//! many there are of each. A screen with three tabs draws all three counts before any
//! of them is opened, and three listings asked for one row each would be three
//! requests to learn what one statement knows.

use super::error::ApiError;
use super::session::Panel;
use crate::types::{ErrorBody, Favourites};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;

/// What you have marked
///
/// Counted over what this account can actually reach: a favourite in a library it may
/// no longer see is not a figure it should be told about, and the tab it belongs to
/// would then count rows the listing does not show.
///
/// Every figure is asked of the marks rather than of the music — from
/// `user_track_stats` to the tracks and not the other way round — because what is
/// being counted is a handful of rows belonging to one account, not a pass over the
/// collection looking for the ones that carry a mark.
#[utoipa::path(
    get,
    path = "/favourites",
    tag = "collection",
    responses(
        (status = 200, description = "How many of each, and how long the tracks run",
         body = Favourites),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn counts(
    panel: Panel,
    State(pool): State<SqlitePool>,
) -> Result<Json<Favourites>, ApiError> {
    let who = panel.user.id;

    let row: (i64, i64, i64, Option<i64>) = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT
           (SELECT count(*) FROM user_track_stats s
              JOIN tracks t ON t.id = s.track_id
             WHERE s.user_id = ? AND s.starred_at IS NOT NULL
               AND t.library_id IN (SELECT id FROM visible_libraries)),
           (SELECT count(*) FROM user_album_stats s
              JOIN albums al ON al.id = s.album_id
             WHERE s.user_id = ? AND s.starred_at IS NOT NULL
               AND ",
        album_is_visible!("al.id"),
        "),
           (SELECT count(*) FROM user_artist_stats s
              JOIN artists a ON a.id = s.artist_id
             WHERE s.user_id = ? AND s.starred_at IS NOT NULL
               AND ",
        artist_is_visible!("a.id"),
        "),
           -- Over the tracks that are still there, like every other length this API
           -- reports: a figure that added up files nobody can play would promise more
           -- music than there is.
           (SELECT sum(t.duration_ms) / 1000 FROM user_track_stats s
              JOIN tracks t ON t.id = s.track_id
             WHERE s.user_id = ? AND s.starred_at IS NOT NULL
               AND t.missing_since IS NULL
               AND t.library_id IN (SELECT id FROM visible_libraries))"
    ))
    .bind(who)
    .bind(who)
    .bind(who)
    .bind(who)
    .bind(who)
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "counting what somebody has marked"))?;

    Ok(Json(Favourites {
        tracks: row.0,
        albums: row.1,
        artists: row.2,
        duration: row.3,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    /// Two accounts, a track, an album and an artist, and nothing marked yet.
    async fn a_collection() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = "2026-01-01T00:00:00Z";
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, is_admin, created_at, updated_at)
             VALUES (1, 'one', 'x', 0, ?1, ?1), (2, 'two', 'x', 0, ?1, ?1);

             INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'kept', '/music', ?1, ?1);
             INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'root', '', 1);

             INSERT INTO artists (id, public_id, name, sort_name, created_at, updated_at)
             VALUES (1, 'ar1', 'Nick Drake', 'Drake, Nick', ?1, ?1);
             INSERT INTO albums (id, public_id, grouping_key, name, year, created_at, updated_at)
             VALUES (1, 'al1', 'pink moon', 'Pink Moon', 1972, ?1, ?1);
             INSERT INTO album_artists (album_id, artist_id, role)
             VALUES (1, 1, 'albumartist');
             INSERT INTO tracks
                 (id, public_id, library_id, folder_id, album_id, path, file_size,
                  file_modified_at, content_type, suffix, title, duration_ms,
                  last_seen_scan, created_at, updated_at)
             VALUES (1, 't1', 1, 1, 1, 'a.flac', 1, ?1, 'audio/flac', 'flac',
                     'Pink Moon', 126000, 1, ?1, ?1),
                    (2, 't2', 1, 1, 1, 'b.flac', 1, ?1, 'audio/flac', 'flac',
                     'Road', 124000, 1, ?1, ?1);

             INSERT INTO track_artists (track_id, artist_id, role, position)
             VALUES (1, 1, 'artist', 0), (2, 1, 'artist', 0)",
        )
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    fn asking(id: i64) -> Panel {
        Panel {
            id,
            user: User {
                id,
                username: format!("user{id}"),
                is_admin: false,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        }
    }

    async fn star(pool: &SqlitePool, statement: &'static str) {
        sqlx::query(statement).execute(pool).await.unwrap();
    }

    /// What one account has marked is not what another has, which is the whole of what
    /// these figures are: three tables keyed on the account.
    #[tokio::test]
    async fn every_figure_is_about_the_account_asking() {
        let pool = a_collection().await;

        star(
            &pool,
            "INSERT INTO user_track_stats (user_id, track_id, starred_at)
             VALUES (1, 1, '2026-08-01T00:00:00Z'), (1, 2, '2026-08-02T00:00:00Z');
             INSERT INTO user_album_stats (user_id, album_id, starred_at)
             VALUES (1, 1, '2026-08-01T00:00:00Z');
             INSERT INTO user_artist_stats (user_id, artist_id, starred_at)
             VALUES (2, 1, '2026-08-01T00:00:00Z')",
        )
        .await;

        let Json(mine) = counts(asking(1), State(pool.clone())).await.unwrap();
        assert_eq!((mine.tracks, mine.albums, mine.artists), (2, 1, 0));
        assert_eq!(mine.duration, Some(250), "the two tracks, in seconds");

        let Json(theirs) = counts(asking(2), State(pool.clone())).await.unwrap();
        assert_eq!((theirs.tracks, theirs.albums, theirs.artists), (0, 0, 1));
        assert_eq!(theirs.duration, None, "nothing marked, nothing to add up");
    }

    /// A row with a rating and no mark is not a favourite.
    ///
    /// The three tables hold both, and a play count besides, so counting the rows
    /// rather than the marks would make rating something enough to file it.
    #[tokio::test]
    async fn a_rating_on_its_own_is_not_a_favourite() {
        let pool = a_collection().await;

        star(
            &pool,
            "INSERT INTO user_track_stats (user_id, track_id, rating, play_count)
             VALUES (1, 1, 5, 12)",
        )
        .await;

        let Json(mine) = counts(asking(1), State(pool)).await.unwrap();
        assert_eq!(mine.tracks, 0);
    }

    /// A favourite in a library the account may not reach is not one of its figures.
    ///
    /// Otherwise a tab counts rows the listing beside it cannot show, and the screen
    /// says "12 albums" over eleven of them for ever.
    #[tokio::test]
    async fn nothing_counts_from_a_library_out_of_reach() {
        let pool = a_collection().await;

        star(
            &pool,
            "INSERT INTO user_track_stats (user_id, track_id, starred_at)
             VALUES (1, 1, '2026-08-01T00:00:00Z');
             INSERT INTO user_album_stats (user_id, album_id, starred_at)
             VALUES (1, 1, '2026-08-01T00:00:00Z');
             INSERT INTO user_artist_stats (user_id, artist_id, starred_at)
             VALUES (1, 1, '2026-08-01T00:00:00Z')",
        )
        .await;

        let Json(before) = counts(asking(1), State(pool.clone())).await.unwrap();
        assert_eq!((before.tracks, before.albums, before.artists), (1, 1, 1));

        // The one library switched off, which takes everything in it out of reach.
        sqlx::query("UPDATE libraries SET enabled = 0")
            .execute(&pool)
            .await
            .unwrap();

        let Json(after) = counts(asking(1), State(pool)).await.unwrap();
        assert_eq!((after.tracks, after.albums, after.artists), (0, 0, 0));
        assert_eq!(after.duration, None);
    }

    /// A favourite whose file has gone is still a favourite.
    ///
    /// It is counted and it stays in the listing — a scan marks rather than deletes,
    /// and the row is exactly the one somebody wants to see — but it lends the total
    /// no length, because it has none to play.
    #[tokio::test]
    async fn a_track_whose_file_is_gone_is_still_counted() {
        let pool = a_collection().await;

        star(
            &pool,
            "INSERT INTO user_track_stats (user_id, track_id, starred_at)
             VALUES (1, 1, '2026-08-01T00:00:00Z');
             UPDATE tracks SET missing_since = '2026-08-03T00:00:00Z' WHERE id = 1",
        )
        .await;

        let Json(mine) = counts(asking(1), State(pool)).await.unwrap();
        assert_eq!(mine.tracks, 1, "still marked, still listed");
        assert_eq!(mine.duration, None, "and no music to promise");
    }
}
