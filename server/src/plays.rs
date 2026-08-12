// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Writing down that somebody listened to something.
//!
//! Shared, because a play is a play whichever way the music came out: a phone on
//! `/rest` and the panel's own player both mean the same thing by it, and the
//! figures they feed — the Overview, the Profile, what a purge would cost — are
//! one set of figures. Two ways of counting would be two answers to how many
//! times a song has been heard.

use crate::db::InTurn;
use sqlx::SqlitePool;

/// The listen after which a song counts as played, where somebody has to decide.
///
/// Four minutes, or half the song if it is shorter than eight — which is the rule
/// every scrobbling service settled on, and settled on for good reasons: half is
/// too much of an hour-long mix and four minutes is more than all of a punk single.
///
/// Nobody needed this until [`crate::subsonic`] grew `reportPlayback`. Every other
/// way in counts a play when it is asked to: a client that says "this was played"
/// has been keeping its own clock and is the only thing that knows whether the
/// listener was in the room. That endpoint is the one where the client reports a
/// timeline and leaves the judgement here, so here is where the judgement is
/// written down — once, in the module that owns what a play is, rather than in
/// whichever handler needed it first.
const LONG_ENOUGH_MS: i64 = 4 * 60 * 1000;

/// Whether a listen of this length, of a song of that length, is a play.
///
/// A song of unknown length is judged by the four minutes alone: it is the only
/// half of the rule that can be applied without knowing.
pub fn counts_as_played(position_ms: i64, duration_ms: Option<i64>) -> bool {
    let half = duration_ms.map(|duration| duration / 2).unwrap_or(i64::MAX);

    position_ms >= LONG_ENOUGH_MS.min(half)
}

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
    .execute(&mut **tx)
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
    .execute(&mut **tx)
    .await?;

    // A play is a play whether or not the client also announced it beforehand,
    // and the song is no longer playing once it has been scrobbled.
    sqlx::query("DELETE FROM now_playing WHERE user_id = ? AND track_id = ?")
        .bind(user_id)
        .bind(track_id)
        .execute(&mut **tx)
        .await?;

    tx.commit().await?;

    // After the commit, and deliberately: what belongs to this server is counted
    // first, and passing it on is a second thing that must not be able to undo the
    // first. It writes a row of its own and comes straight back — nothing is sent
    // from here, because a phone waiting on a reply has no business waiting on
    // somebody else's website.
    crate::scrobble::queue(pool, user_id, track_id, played_at).await;

    Ok(())
}

/// Records that a song is playing right now, without counting it as played.
///
/// What is written here expires on its own once the song could no longer be
/// playing, so an announcement nobody ever follows up on stops being one.
///
/// Answers with the track it wrote down, so that whoever called can pass the same
/// announcement on to a scrobbling service. Nothing is sent from in here: this is
/// a database write that a client is waiting on.
pub async fn record_now_playing(
    pool: &SqlitePool,
    user_id: i64,
    client: &str,
    public_id: &str,
    started_at: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let Some(track_id): Option<i64> =
        sqlx::query_scalar("SELECT id FROM tracks WHERE public_id = ?")
            .bind(public_id)
            .fetch_optional(pool)
            .await?
    else {
        return Ok(None);
    };

    // Keyed by user and client, so the same person listening on their phone and
    // their desktop shows up as two things playing rather than one overwriting
    // the other.
    sqlx::query(
        "INSERT INTO now_playing (user_id, client, track_id, started_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id, client) DO UPDATE SET
             track_id = excluded.track_id,
             started_at = excluded.started_at",
    )
    .bind(user_id)
    .bind(client)
    .bind(track_id)
    .bind(started_at)
    .in_turn(pool)
    .await?;

    // A client that keeps announcing replaces its own row and leaves nothing
    // behind, but the name it announces under is whatever it sent in `c=`, so a
    // client that varies it is a client that adds a row every time. Reading is
    // already protected — a stale row is filtered out there — and this is what
    // keeps the table from being a place to write to. Only this account's rows,
    // on this account's own request.
    sqlx::query(concat!(
        "DELETE FROM now_playing
          WHERE user_id = ?
            AND NOT EXISTS (SELECT 1 FROM tracks t
                             WHERE t.id = now_playing.track_id
                               AND ",
        still_playing!("now_playing.started_at", "t.duration_ms"),
        ")"
    ))
    .bind(user_id)
    .in_turn(pool)
    .await?;

    Ok(Some(track_id))
}

/// Stops saying that this client is playing anything.
///
/// For the listener who pressed stop on something they had barely started: the
/// play does not count, and the announcement should not outlive it either. Only
/// this client's row, because the same person on another device is another row and
/// still playing.
pub async fn forget_now_playing(
    pool: &SqlitePool,
    user_id: i64,
    client: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM now_playing WHERE user_id = ? AND client = ?")
        .bind(user_id)
        .bind(client)
        .in_turn(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod now_playing_tests {
    use super::*;
    use crate::db;

    /// One song of three minutes, and somebody to play it from wherever.
    async fn a_song() -> (SqlitePool, i64) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'music', '/music', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'music', '/music', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                 file_modified_at, content_type, suffix, title, duration_ms,
                                 last_seen_scan, created_at, updated_at)
             VALUES (1, 'trk1', 1, 1, '/one.wav', 1, ?, 'audio/wav', 'wav', 'One', 180000,
                     1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&at)
        .bind(&at)
        .fetch_one(&pool)
        .await
        .unwrap();

        (pool, user)
    }

    async fn clients(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT client FROM now_playing ORDER BY client")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// Two devices, two rows. Announcing from the second must not take the first
    /// one's place.
    #[tokio::test]
    async fn one_player_does_not_replace_another() {
        let (pool, ana) = a_song().await;

        record_now_playing(&pool, ana, "Phone", "trk1", &db::now())
            .await
            .unwrap();
        record_now_playing(&pool, ana, "Desktop", "trk1", &db::now())
            .await
            .unwrap();

        assert_eq!(clients(&pool).await, ["Desktop", "Phone"]);
    }

    /// Announcing again from the same client moves its own row rather than adding
    /// one, which is what makes the name in `c=` the identity of a player.
    #[tokio::test]
    async fn the_same_player_keeps_one_row() {
        let (pool, ana) = a_song().await;

        for _ in 0..3 {
            record_now_playing(&pool, ana, "Phone", "trk1", &db::now())
                .await
                .unwrap();
        }

        assert_eq!(clients(&pool).await, ["Phone"]);
    }

    /// The name a player announces under is whatever it chose to send, so rows
    /// nothing will ever come back for have to be swept rather than merely
    /// filtered out when read. Otherwise a client varying `c=` is a client writing
    /// to the database as often as it likes.
    #[tokio::test]
    async fn announcing_sweeps_what_can_no_longer_be_playing() {
        let (pool, ana) = a_song().await;

        for old in ["Gone", "Also gone"] {
            sqlx::query(
                "INSERT INTO now_playing (user_id, client, track_id, started_at)
                 VALUES (?, ?, 1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-1 hour'))",
            )
            .bind(ana)
            .bind(old)
            .execute(&pool)
            .await
            .unwrap();
        }

        record_now_playing(&pool, ana, "Phone", "trk1", &db::now())
            .await
            .unwrap();

        assert_eq!(clients(&pool).await, ["Phone"], "the stale ones are gone");
    }

    /// Sweeping is on the account's own behalf. Somebody else's stale row is
    /// somebody else's to lose, and it is invisible in the meantime anyway.
    #[tokio::test]
    async fn sweeping_stops_at_the_account_doing_it() {
        let (pool, ana) = a_song().await;

        let leo: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('leo', 'x', 0, '', '') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO now_playing (user_id, client, track_id, started_at)
             VALUES (?, 'Leo phone', 1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-1 hour'))",
        )
        .bind(leo)
        .execute(&pool)
        .await
        .unwrap();

        record_now_playing(&pool, ana, "Phone", "trk1", &db::now())
            .await
            .unwrap();

        assert_eq!(clients(&pool).await, ["Leo phone", "Phone"]);
    }
    /// Stopping short takes this player off and leaves the other one where it was:
    /// the same person on two devices is two rows, and only one of them stopped.
    #[tokio::test]
    async fn forgetting_takes_off_the_player_that_stopped_and_no_other() {
        let (pool, ana) = a_song().await;

        record_now_playing(&pool, ana, "Phone", "trk1", &db::now())
            .await
            .unwrap();
        record_now_playing(&pool, ana, "Desktop", "trk1", &db::now())
            .await
            .unwrap();

        forget_now_playing(&pool, ana, "Phone").await.unwrap();

        assert_eq!(clients(&pool).await, ["Desktop"]);
    }

    /// What a listen has to reach to be a play, which is the one rule this server
    /// applies on somebody's behalf rather than on their word — see `reportPlayback`.
    #[test]
    fn four_minutes_or_half_of_it_whichever_comes_first() {
        let three_minutes = Some(180_000);
        assert!(
            counts_as_played(90_000, three_minutes),
            "half of a short one"
        );
        assert!(!counts_as_played(89_000, three_minutes));

        let an_hour = Some(3_600_000);
        assert!(
            counts_as_played(240_000, an_hour),
            "four minutes of a long one"
        );
        assert!(!counts_as_played(239_000, an_hour));

        assert!(
            counts_as_played(240_000, None),
            "and of one that never said"
        );
        assert!(!counts_as_played(1, None));
    }
}
