// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Writing down that somebody listened to something.
//!
//! Shared, because a play is a play whichever way the music came out: a phone on
//! `/rest` and the panel's own player both mean the same thing by it, and the
//! figures they feed — the Overview, the Profile, what a purge would cost — are
//! one set of figures. Two ways of counting would be two answers to how many
//! times a song has been heard.

use sqlx::SqlitePool;

/// Counts a play: one more for the tally, and the time it happened.
pub async fn record_play(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
    played_at: &str,
) -> Result<(), sqlx::Error> {
    // Through the same filter as everything else. A play counted for a track in
    // a library this account may not see would put a number in their own figures
    // that they cannot account for.
    let Some(track_id): Option<i64> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        " SELECT id FROM tracks
           WHERE public_id = ? AND library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(user_id)
    .bind(public_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(());
    };

    let mut tx = crate::db::writing(pool).await?;

    sqlx::query(
        "INSERT INTO user_track_stats (user_id, track_id, play_count, last_played)
         VALUES (?, ?, 1, ?)
         ON CONFLICT (user_id, track_id) DO UPDATE SET
             play_count = play_count + 1,
             last_played = excluded.last_played",
    )
    .bind(user_id)
    .bind(track_id)
    .bind(played_at)
    .execute(&mut *tx)
    .await?;

    // The album tally too, so a client can sort albums by how much they get
    // played without adding up their tracks every time.
    sqlx::query(
        "INSERT INTO user_album_stats (user_id, album_id, play_count, last_played)
         SELECT ?, album_id, 1, ? FROM tracks WHERE id = ? AND album_id IS NOT NULL
         ON CONFLICT (user_id, album_id) DO UPDATE SET
             play_count = play_count + 1,
             last_played = excluded.last_played",
    )
    .bind(user_id)
    .bind(played_at)
    .bind(track_id)
    .execute(&mut *tx)
    .await?;

    // A play is a play whether or not the client also announced it beforehand,
    // and the song is no longer playing once it has been scrobbled.
    sqlx::query("DELETE FROM now_playing WHERE user_id = ? AND track_id = ?")
        .bind(user_id)
        .bind(track_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await
}
