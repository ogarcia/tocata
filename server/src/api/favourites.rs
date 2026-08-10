// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How much of the collection somebody has marked as theirs.
//!
//! The listings themselves are not here: a favourite is not a fifth kind of thing to
//! list, it is the collection narrowed by who is reading it, so `/tracks`, `/albums`
//! and `/artists` answer for them with `starred=true` — which is also what makes
//! searching your own favourites, and playing them, the endpoints that already exist.
//!
//! What is here is the one question those listings cannot answer between them — how
//! many there are of each — and the marking itself. A screen with three tabs draws all
//! three counts before any of them is opened, and three listings asked for one row each
//! would be three requests to learn what one statement knows.
//!
//! **Marking is `PUT` and unmarking is `DELETE`, on the same address.** Both are
//! idempotent, which is what a heart wants: pressing it twice from two devices leaves
//! it marked once, and a request that was already true is not an error.
//!
//! **What OpenSubsonic writes and what this writes are the same rows.** `star` and
//! `unstar` have worked since that side was written; this is the panel's door to the
//! same three tables, and neither can see anything the other cannot.

use super::error::ApiError;
use super::session::Panel;
use crate::db::{self, InTurn};
use crate::types::{ErrorBody, Favourites};
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use serde::Deserialize;
use sqlx::SqlitePool;

/// Which of the three kinds of thing is being marked.
///
/// A path segment rather than three pairs of handlers, because what differs between
/// them is one table and one column and everything else — who is asking, whether they
/// may see it, what marking means — is the same question three times.
#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Tracks,
    Albums,
    Artists,
}

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

/// Mark something as a favourite
///
/// Idempotent: marking what is already marked leaves the mark where it was rather than
/// moving it, so pressing the heart from two devices does not reorder anything.
///
/// A track, record or name this account may not see answers the same 404 as one that is
/// not there. Nothing about a mark being the account's own lifts the wall round a
/// library.
#[utoipa::path(
    put,
    path = "/favourites/{kind}/{id}",
    tag = "collection",
    params(
        ("kind" = Kind, Path, description = "tracks, albums or artists"),
        ("id" = String, Path, description = "Which one"),
    ),
    responses(
        (status = 204, description = "Marked"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such thing, or not one you may see", body = ErrorBody),
    )
)]
pub async fn mark(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath((kind, id)): UrlPath<(Kind, String)>,
) -> Result<StatusCode, ApiError> {
    let who = panel.user.id;
    let which = reachable(&pool, who, kind, &id).await?;

    // A row may already be there carrying a rating or a play count, so the mark is
    // written onto it rather than instead of it — and left alone if it is already
    // marked, which is what keeps a second press from moving something up the listing.
    let statement = match kind {
        Kind::Tracks => {
            "INSERT INTO user_track_stats (user_id, track_id, starred_at) VALUES (?, ?, ?)
             ON CONFLICT (user_id, track_id)
             DO UPDATE SET starred_at = coalesce(starred_at, excluded.starred_at)"
        }
        Kind::Albums => {
            "INSERT INTO user_album_stats (user_id, album_id, starred_at) VALUES (?, ?, ?)
             ON CONFLICT (user_id, album_id)
             DO UPDATE SET starred_at = coalesce(starred_at, excluded.starred_at)"
        }
        Kind::Artists => {
            "INSERT INTO user_artist_stats (user_id, artist_id, starred_at) VALUES (?, ?, ?)
             ON CONFLICT (user_id, artist_id)
             DO UPDATE SET starred_at = coalesce(starred_at, excluded.starred_at)"
        }
    };

    sqlx::query(statement)
        .bind(who)
        .bind(which)
        .bind(db::now())
        .in_turn(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "marking something as a favourite"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Take a favourite mark off
///
/// The mark goes and the row stays: it carries the play count and the rating too, and
/// unmarking a song is not forgetting that it was played.
///
/// Idempotent as well, and for the same reason the marking is. Unmarking what was never
/// marked is not a mistake worth an error.
#[utoipa::path(
    delete,
    path = "/favourites/{kind}/{id}",
    tag = "collection",
    params(
        ("kind" = Kind, Path, description = "tracks, albums or artists"),
        ("id" = String, Path, description = "Which one"),
    ),
    responses(
        (status = 204, description = "Not marked any more"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such thing, or not one you may see", body = ErrorBody),
    )
)]
pub async fn unmark(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath((kind, id)): UrlPath<(Kind, String)>,
) -> Result<StatusCode, ApiError> {
    let who = panel.user.id;
    let which = reachable(&pool, who, kind, &id).await?;

    let statement = match kind {
        Kind::Tracks => {
            "UPDATE user_track_stats SET starred_at = NULL WHERE user_id = ? AND track_id = ?"
        }
        Kind::Albums => {
            "UPDATE user_album_stats SET starred_at = NULL WHERE user_id = ? AND album_id = ?"
        }
        Kind::Artists => {
            "UPDATE user_artist_stats SET starred_at = NULL WHERE user_id = ? AND artist_id = ?"
        }
    };

    sqlx::query(statement)
        .bind(who)
        .bind(which)
        .in_turn(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "taking a favourite mark off"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// The row this account may mark, by its public identifier, or a 404.
///
/// Two things at once, and they belong together: what the tables key on is the internal
/// id, and whether somebody may write a mark about something is whether they may see it
/// at all. Answering the first without asking the second would let an account file away
/// a record it cannot be told exists.
async fn reachable(pool: &SqlitePool, who: i64, kind: Kind, id: &str) -> Result<i64, ApiError> {
    let statement = match kind {
        Kind::Tracks => concat!(
            visible_libraries!(),
            "SELECT t.id FROM tracks t
              WHERE t.public_id = ? AND t.library_id IN (SELECT id FROM visible_libraries)"
        ),
        Kind::Albums => concat!(
            visible_libraries!(),
            "SELECT al.id FROM albums al WHERE al.public_id = ? AND ",
            album_is_visible!("al.id")
        ),
        Kind::Artists => concat!(
            visible_libraries!(),
            "SELECT a.id FROM artists a WHERE a.public_id = ? AND ",
            artist_is_visible!("a.id")
        ),
    };

    sqlx::query_scalar(statement)
        .bind(who)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ApiError::internal(e, "finding what is being marked"))?
        .ok_or(ApiError::NotFound)
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

    /// What the marks table holds for one account, so a test can say what a press left
    /// behind rather than what an answer said about it.
    async fn starred(pool: &SqlitePool, who: i64) -> Vec<(i64, Option<String>)> {
        sqlx::query_as(
            "SELECT track_id, starred_at FROM user_track_stats
              WHERE user_id = ? ORDER BY track_id",
        )
        .bind(who)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// The heart, both ways, on all three kinds.
    #[tokio::test]
    async fn marking_and_unmarking_are_the_same_address_either_way() {
        let pool = a_collection().await;

        for (kind, id) in [
            (Kind::Tracks, "t1"),
            (Kind::Albums, "al1"),
            (Kind::Artists, "ar1"),
        ] {
            let marked = mark(
                asking(1),
                State(pool.clone()),
                UrlPath((kind, id.to_string())),
            )
            .await
            .unwrap();
            assert_eq!(marked, StatusCode::NO_CONTENT);
        }

        let Json(held) = counts(asking(1), State(pool.clone())).await.unwrap();
        assert_eq!((held.tracks, held.albums, held.artists), (1, 1, 1));

        for (kind, id) in [
            (Kind::Tracks, "t1"),
            (Kind::Albums, "al1"),
            (Kind::Artists, "ar1"),
        ] {
            unmark(
                asking(1),
                State(pool.clone()),
                UrlPath((kind, id.to_string())),
            )
            .await
            .unwrap();
        }

        let Json(after) = counts(asking(1), State(pool.clone())).await.unwrap();
        assert_eq!((after.tracks, after.albums, after.artists), (0, 0, 0));
    }

    /// Pressing the heart twice leaves it marked once, and marked when it first was.
    ///
    /// Which is what `coalesce` in that upsert is for: two devices doing the same thing
    /// must not move a row up a listing ordered by when it was marked.
    #[tokio::test]
    async fn marking_what_is_already_marked_does_not_move_it() {
        let pool = a_collection().await;

        star(
            &pool,
            "INSERT INTO user_track_stats (user_id, track_id, starred_at)
             VALUES (1, 1, '2026-01-05T00:00:00Z')",
        )
        .await;

        mark(
            asking(1),
            State(pool.clone()),
            UrlPath((Kind::Tracks, "t1".to_string())),
        )
        .await
        .unwrap();

        assert_eq!(
            starred(&pool, 1).await,
            [(1, Some("2026-01-05T00:00:00Z".to_string()))],
            "still the moment it was first marked"
        );
    }

    /// Unmarking keeps the row, because the row is not only the mark.
    ///
    /// Deleting it would throw away a play count and a rating, and unmarking a song is
    /// not forgetting that it was ever played.
    #[tokio::test]
    async fn taking_a_mark_off_keeps_what_else_the_row_holds() {
        let pool = a_collection().await;

        star(
            &pool,
            "INSERT INTO user_track_stats (user_id, track_id, starred_at, rating, play_count)
             VALUES (1, 1, '2026-01-05T00:00:00Z', 5, 12)",
        )
        .await;

        unmark(
            asking(1),
            State(pool.clone()),
            UrlPath((Kind::Tracks, "t1".to_string())),
        )
        .await
        .unwrap();

        let kept: (Option<i64>, i64, Option<String>) = sqlx::query_as(
            "SELECT rating, play_count, starred_at FROM user_track_stats
              WHERE user_id = 1 AND track_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(kept, (Some(5), 12, None));
    }

    /// Unmarking something never marked is not a mistake.
    #[tokio::test]
    async fn taking_off_a_mark_that_was_never_there_is_no_error() {
        let pool = a_collection().await;

        let answered = unmark(
            asking(1),
            State(pool.clone()),
            UrlPath((Kind::Tracks, "t1".to_string())),
        )
        .await
        .unwrap();

        assert_eq!(answered, StatusCode::NO_CONTENT);
        assert!(starred(&pool, 1).await.is_empty(), "and wrote nothing");
    }

    /// Nothing can be marked that could not be seen.
    ///
    /// A mark is the account's own, which is exactly the argument that would let one be
    /// written about a record in a library it was walled off from — and a 404 that
    /// turned into a 204 would be the panel confirming the thing exists.
    #[tokio::test]
    async fn nothing_out_of_reach_can_be_marked() {
        let pool = a_collection().await;
        sqlx::query("UPDATE libraries SET enabled = 0")
            .execute(&pool)
            .await
            .unwrap();

        for (kind, id) in [
            (Kind::Tracks, "t1"),
            (Kind::Albums, "al1"),
            (Kind::Artists, "ar1"),
        ] {
            let refused = mark(
                asking(1),
                State(pool.clone()),
                UrlPath((kind, id.to_string())),
            )
            .await;
            assert!(matches!(refused, Err(ApiError::NotFound)), "{id}");

            let refused = unmark(
                asking(1),
                State(pool.clone()),
                UrlPath((kind, id.to_string())),
            )
            .await;
            assert!(matches!(refused, Err(ApiError::NotFound)), "{id}");
        }

        assert!(starred(&pool, 1).await.is_empty());
    }

    /// And nothing that is not there at all.
    #[tokio::test]
    async fn a_name_nothing_answers_to_is_a_miss() {
        let pool = a_collection().await;

        let refused = mark(
            asking(1),
            State(pool),
            UrlPath((Kind::Tracks, "nothing".to_string())),
        )
        .await;

        assert!(matches!(refused, Err(ApiError::NotFound)));
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
