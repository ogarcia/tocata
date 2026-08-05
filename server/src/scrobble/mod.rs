// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Passing on what somebody listened to.
//!
//! Tocata counts every play in its own database, and that is what the Overview
//! and the Profile are made of. This is the other thing: handing the same listen
//! to a service somebody keeps their listening history on, which is theirs and
//! not ours.
//!
//! **It goes through a queue, and the queue is the point.** A scrobbling service
//! is somebody else's machine — down for an afternoon, behind a domain that
//! stopped resolving, or simply slower than the song that is already over. A
//! listen sent straight out and lost when it failed would be a listen nobody
//! could get back, because the moment it happened has passed. So a play writes a
//! row and comes back at once, and the sending is somebody else's problem: the
//! task in [`sending`], every minute, for as long as it takes.
//!
//! **What is sounding now does not go through the queue**, and that is not an
//! inconsistency. A now playing notification is a claim about the present: held
//! for ten minutes and delivered late it would say something false, so it is
//! offered once and dropped if it does not land.
//!
//! One protocol, several services. The dialect is [`listenbrainz`]; what a service
//! adds to it is a name, an address and the path it answers under.

pub mod listenbrainz;
pub mod sending;

use crate::db;
use crate::net::Net;
use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tracing::{info, warn};

/// A service listens can be sent to.
///
/// An enum and not a table, the way a maintenance job is not one: what a service
/// amounts to is an address and a dialect, and both of those are code. Adding one
/// is a line here; what somebody configured survives it, because the row names the
/// service by the same name this does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Service {
    /// The hosted one at listenbrainz.org.
    ListenBrainz,
    /// <https://koito.io>, self hosted, which answers the ListenBrainz calls under
    /// a path of its own.
    Koito,
    /// <https://github.com/krateng/maloja>, self hosted, and the same again.
    Maloja,
}

/// Every service there is, in the order a screen should offer them: the hosted one
/// first, because it is the one somebody has probably already got an account with.
pub const EVERY: [Service; 3] = [Service::ListenBrainz, Service::Koito, Service::Maloja];

/// The service that answers to a name, or nothing if none does.
pub fn named(name: &str) -> Option<Service> {
    EVERY.into_iter().find(|service| service.name() == name)
}

impl Service {
    /// What the database and the API call it.
    pub fn name(self) -> &'static str {
        match self {
            Self::ListenBrainz => "listenbrainz",
            Self::Koito => "koito",
            Self::Maloja => "maloja",
        }
    }

    /// What a person calls it. Not translated and never will be: these are the
    /// names their authors gave them, and a panel in Spanish still says Koito.
    pub fn shown(self) -> &'static str {
        match self {
            Self::ListenBrainz => "ListenBrainz",
            Self::Koito => "Koito",
            Self::Maloja => "Maloja",
        }
    }

    /// The address, for a service that has one everybody uses. `None` means it is
    /// somebody's own machine and only they know where it is.
    pub fn home(self) -> Option<&'static str> {
        match self {
            Self::ListenBrainz => Some("https://api.listenbrainz.org"),
            Self::Koito | Self::Maloja => None,
        }
    }

    /// What these calls hang under, below whatever address was given. The hosted
    /// one *is* the API; the others are a scrobbler with a web site of its own that
    /// keeps a ListenBrainz shaped door in one corner.
    pub fn beneath(self) -> &'static str {
        match self {
            Self::ListenBrainz => "",
            Self::Koito | Self::Maloja => "/apis/listenbrainz",
        }
    }

    /// The root every call is built from: what was stored, plus the path this
    /// service answers under.
    pub fn root(self, url: &str) -> String {
        format!("{}{}", url.trim_end_matches('/'), self.beneath())
    }
}

/// A listen, as it was when it happened.
///
/// Read out of the queue rather than out of the collection, which is what makes it
/// survive the collection changing underneath it — see the queue's own note in the
/// schema. `at` is seconds since the epoch, because that is what goes on the wire;
/// nothing stores it that way.
pub struct Listen {
    pub at: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub mbid_recording: Option<String>,
    pub mbid_release: Option<String>,
    pub mbid_artist: Option<String>,
    pub isrc: Option<String>,
    pub track_number: Option<i64>,
    pub duration_ms: Option<i64>,
}

/// Where somebody's listens are going: the row, without the parts nobody sending
/// needs.
struct Destination {
    user_id: i64,
    service: Service,
    url: String,
    token: String,
}

/// The leading credit of a track, as a subquery: the artist if it has one and the
/// album artist if that is all there is.
///
/// A macro because sqlx takes no SQL assembled at runtime, and this is wanted twice
/// in one statement — once for the name and once for the identifier — with nothing
/// differing but the column.
///
/// Ordered so that `artist` wins over `albumartist` and, within a role, the order
/// the tag credited them in: "A feat. B" scrobbles as A, which is what every other
/// client does with it.
macro_rules! credited {
    ($column:literal) => {
        concat!(
            "(SELECT ar.",
            $column,
            "   FROM track_artists ta JOIN artists ar ON ar.id = ta.artist_id
               WHERE ta.track_id = t.id AND ta.role IN ('artist', 'albumartist')
               ORDER BY ta.role = 'albumartist', ta.position, ar.name LIMIT 1)"
        )
    };
}

/// Writes a listen down for every service this person has switched on.
///
/// Called from the one place a play is counted, so that a play from a phone and a
/// play from the panel are passed on alike.
///
/// Nothing here fails loudly. A listen that could not be queued is a listen that
/// will not be sent, which is a shame and is not a reason to fail the request that
/// counted it: what the person asked for was for their play to be counted, and it
/// was. It is warned about instead, because a database that cannot take this row
/// is a database with something else wrong with it.
pub async fn queue(pool: &SqlitePool, user_id: i64, track_id: i64, played_at: &str) {
    if let Err(e) = enqueue(pool, user_id, track_id, played_at).await {
        warn!("a listen could not be queued for sending: {e:#}");
    }
}

/// The statement behind [`queue`].
///
/// One insert, reading from the same database it writes to. Which is what makes
/// this atomic and short: no row is built in this process and no decision is taken
/// here — the join decides who it goes to, and the where decides whether it goes
/// at all.
///
/// A song with no credit at all is not queued, and the inner join is what does it.
/// The far end insists on an artist name, so there would be nothing to send;
/// putting the file's name in, or the word "Unknown", would file a listen in
/// somebody's history against a band that does not exist.
async fn enqueue(pool: &SqlitePool, user_id: i64, track_id: i64, played_at: &str) -> Result<()> {
    let at = db::now();

    let queued = sqlx::query(concat!(
        "INSERT INTO scrobble_queue
              (user_id, service, played_at, title, artist, album, mbid_recording,
               mbid_release, mbid_artist, isrc, track_number, duration_ms,
               attempts, next_try_at, created_at)
         SELECT s.user_id, s.service, ?, t.title, ",
        credited!("name"),
        ", al.name, t.mbid_recording, al.mbid_release, ",
        credited!("mbid"),
        ", t.isrc, t.track_number, t.duration_ms, 0, ?, ?
           FROM scrobblers s
           JOIN users u  ON u.id = s.user_id
           JOIN tracks t ON t.id = ?
           LEFT JOIN albums al ON al.id = t.album_id
          WHERE s.user_id = ?
            AND s.enabled = 1
            AND u.scrobbling_enabled = 1
            AND ",
        credited!("name"),
        " IS NOT NULL"
    ))
    .bind(played_at)
    // Due at once: the sender takes what is due, and a listen is due the moment
    // it happened.
    .bind(&at)
    .bind(&at)
    .bind(track_id)
    .bind(user_id)
    .execute(pool)
    .await
    .context("queueing a listen")?;

    if queued.rows_affected() > 0 {
        info!(
            "queued a listen for {} destination(s)",
            queued.rows_affected()
        );
    }

    Ok(())
}

/// Makes everything waiting for one destination due now, forgetting how badly it
/// has been going.
///
/// Called when a destination is set up again or switched back on, and it matters
/// more than it looks. The wait grows to six hours after a handful of failures, and
/// the commonest reason for those failures is a token that was wrong — so without
/// this, correcting the token would be answered by nothing happening until the
/// evening. What the waiting was about no longer exists.
///
/// The reason for the last failure goes with it, for the same reason: it describes
/// a setup that has just been replaced.
pub async fn due_again(pool: &SqlitePool, user_id: i64, service: Service) -> Result<()> {
    sqlx::query(
        "UPDATE scrobble_queue
            SET next_try_at = ?, attempts = 0, last_error = NULL
          WHERE user_id = ? AND service = ?",
    )
    .bind(db::now())
    .bind(user_id)
    .bind(service.name())
    .execute(pool)
    .await
    .context("making a destination's listens due again")?;

    Ok(())
}

/// Tells every service this person has switched on that a song has started.
///
/// Offered once and forgotten. It carries no time and means "now", so there is
/// nothing worth retrying: by the time a retry went out it would be announcing a
/// song that had already finished.
///
/// Spawned rather than awaited by whoever calls it, for the same reason: a client
/// that said "this is playing" is not waiting to hear what somebody else's server
/// thought of it.
pub async fn announce(net: &Net, pool: &SqlitePool, user_id: i64, track_id: i64) {
    let listen = match sounding(pool, user_id, track_id).await {
        Ok(Some(listen)) => listen,
        // Nothing to announce: no credit, or a track that has gone since.
        Ok(None) => return,
        Err(e) => return warn!("could not read what is sounding: {e:#}"),
    };

    let destinations = match switched_on(pool, Some(user_id)).await {
        Ok(destinations) => destinations,
        Err(e) => return warn!("could not read where to announce: {e:#}"),
    };

    for destination in destinations {
        let json = match listenbrainz::playing_now(&listen) {
            Ok(json) => json,
            Err(e) => {
                warn!("could not write a now playing notification: {e}");
                continue;
            }
        };

        let url = listenbrainz::submitting(&destination.service.root(&destination.url));

        match net.post(&url, &destination.token, json).await {
            Ok(answer) if answer.ok() => {}
            Ok(answer) => warn!(
                "{} would not take what is sounding: {} {}",
                destination.service.name(),
                answer.status,
                answer.body.trim()
            ),
            Err(e) => warn!(
                "could not tell {} what is sounding: {e:#}",
                destination.service.name()
            ),
        }
    }
}

/// A track as a listen, for announcing one that is only just starting.
///
/// Reads the collection, unlike everything else here, because this is not a
/// listen that has happened yet: there is nothing in the queue to read, and by
/// the time it mattered the file was still playing.
///
/// Through the same library filter a counted play goes through. Announcing is not
/// reading a track's details, but it does put a song's name on somebody's public
/// profile, and a song from a library this account cannot see has no business
/// getting there.
async fn sounding(pool: &SqlitePool, user_id: i64, track_id: i64) -> Result<Option<Listen>> {
    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );

    let row: Option<Row> = sqlx::query_as(concat!(
        visible_libraries!(),
        " SELECT t.title, ",
        credited!("name"),
        ", al.name, t.mbid_recording, al.mbid_release, ",
        credited!("mbid"),
        ", t.isrc, t.track_number, t.duration_ms
           FROM tracks t
           LEFT JOIN albums al ON al.id = t.album_id
          WHERE t.id = ?
            AND t.library_id IN (SELECT id FROM visible_libraries)
            AND ",
        credited!("name"),
        " IS NOT NULL"
    ))
    .bind(user_id)
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .context("reading a track to announce")?;

    Ok(row.map(
        |(
            title,
            artist,
            album,
            mbid_recording,
            mbid_release,
            mbid_artist,
            isrc,
            track_number,
            duration_ms,
        )| Listen {
            // Nothing reads it: a now playing notification carries no time.
            at: 0,
            title,
            artist,
            album,
            mbid_recording,
            mbid_release,
            mbid_artist,
            isrc,
            track_number,
            duration_ms,
        },
    ))
}

/// The destinations that are switched on, for one person or for everybody.
///
/// Both switches are read here: the account's own — the one OpenSubsonic has
/// always carried and nothing until now consulted — and the destination's. Either
/// one off means nothing is sent and nothing is queued.
async fn switched_on(pool: &SqlitePool, whose: Option<i64>) -> Result<Vec<Destination>> {
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT s.user_id, s.service, s.url, s.token
           FROM scrobblers s
           JOIN users u ON u.id = s.user_id
          WHERE s.enabled = 1
            AND u.scrobbling_enabled = 1
            AND (? IS NULL OR s.user_id = ?)",
    )
    .bind(whose)
    .bind(whose)
    .fetch_all(pool)
    .await
    .context("reading where listens go")?;

    Ok(rows
        .into_iter()
        .filter_map(|(user_id, service, url, token)| {
            Some(Destination {
                user_id,
                // A row naming a service this version does not have. Skipped
                // rather than failed, the way an old job run is: what is in the
                // program decides what can be sent to.
                service: named(&service)?,
                url,
                token,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is queued, as the columns that matter to a test.
    type Waiting = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );

    /// A collection with one song on one record, credited to one artist, and one
    /// person to listen to it. Everything a listen is made of, and nothing else.
    async fn collection() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();

        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'media', '/media', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'media', '/media', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // The compilation is named so that it comes first by every tie breaker the
        // subquery has — lower id, same position, earlier name — which is what makes
        // the test below test the role and not the insertion order.
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, mbid, created_at, updated_at)
             VALUES (2, 'a1', 'Porcupine Tree', 'art-1', ?, ?),
                    (1, 'a2', 'Best of the 2000s', NULL, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO albums (id, public_id, name, mbid_release, created_at, updated_at)
             VALUES (1, 'al1', 'In Absentia', 'rel-1', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                 file_size, file_modified_at, content_type, suffix, title,
                                 track_number, duration_ms, mbid_recording, isrc,
                                 last_seen_scan, created_at, updated_at)
             VALUES (1, 'trk1', 1, 1, 1, 'trains.flac', 1, ?, 'audio/flac', 'flac',
                     'Trains', 2, 351000, 'rec-1', 'GBAAA0000001', 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role) VALUES (1, 2, 'artist')",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    /// Somebody who listens, scrobbling switched on unless told otherwise.
    async fn listener(pool: &SqlitePool, name: &str, scrobbling: bool) -> i64 {
        let at = db::now();

        sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, scrobbling_enabled, created_at, updated_at)
             VALUES (?, 'x', ?, ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(i64::from(scrobbling))
        .bind(&at)
        .bind(&at)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn destination(pool: &SqlitePool, user_id: i64, service: Service, enabled: bool) {
        let at = db::now();

        sqlx::query(
            "INSERT INTO scrobblers (user_id, service, url, token, enabled, created_at, updated_at)
             VALUES (?, ?, 'https://example.test', 'tok', ?, ?, ?)",
        )
        .bind(user_id)
        .bind(service.name())
        .bind(i64::from(enabled))
        .bind(&at)
        .bind(&at)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn queued(pool: &SqlitePool) -> Vec<Waiting> {
        sqlx::query_as(
            "SELECT service, title, artist, album, mbid_recording, mbid_artist,
                    track_number, duration_ms
               FROM scrobble_queue ORDER BY service",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// The whole point of a row per service: two destinations means the same listen
    /// waiting twice, because one of them may take it and the other may not.
    #[tokio::test]
    async fn a_play_is_queued_once_for_every_destination() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;
        destination(&pool, ana, Service::Maloja, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        let waiting = queued(&pool).await;
        assert_eq!(waiting.len(), 2);
        assert_eq!(waiting[0].0, "listenbrainz");
        assert_eq!(waiting[1].0, "maloja");
    }

    /// Everything a listen needs to be matched to the right recording rather than
    /// to a song of the same name, taken at the moment it was heard.
    #[tokio::test]
    async fn what_is_queued_is_the_song_and_not_a_pointer_to_it() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        let waiting = queued(&pool).await;
        assert_eq!(waiting[0].1, "Trains");
        assert_eq!(waiting[0].2, "Porcupine Tree");
        assert_eq!(waiting[0].3.as_deref(), Some("In Absentia"));
        assert_eq!(waiting[0].4.as_deref(), Some("rec-1"));
        assert_eq!(waiting[0].5.as_deref(), Some("art-1"));
        assert_eq!(waiting[0].6, Some(2));
        assert_eq!(waiting[0].7, Some(351_000));
    }

    /// And the reason it is a copy: the file can go — purged, or its library
    /// removed — and a listen that already happened is still owed to whoever heard
    /// it. Reading the track at sending time would either lose it or send the
    /// wrong song.
    #[tokio::test]
    async fn a_queued_listen_survives_the_track_going_away() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        sqlx::query("DELETE FROM tracks WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        let waiting = queued(&pool).await;
        assert_eq!(waiting.len(), 1, "it is still owed");
        assert_eq!(waiting[0].1, "Trains");
        assert_eq!(waiting[0].2, "Porcupine Tree");
    }

    /// The account's own switch, which OpenSubsonic has carried from the start and
    /// which until now nothing read. Off means nothing is passed on, however many
    /// destinations are set up.
    #[tokio::test]
    async fn the_account_switch_stops_it_before_the_destination_does() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", false).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        assert!(queued(&pool).await.is_empty());
    }

    /// A destination switched off keeps what it already has and takes nothing new,
    /// which is what makes it different from removing it.
    #[tokio::test]
    async fn a_destination_switched_off_is_not_queued_for() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, false).await;
        destination(&pool, ana, Service::Koito, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        let waiting = queued(&pool).await;
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].0, "koito");
    }

    /// The common case by far: nobody has set any of this up. It has to cost a
    /// statement and write nothing, since it runs on every play there is.
    #[tokio::test]
    async fn nobody_scrobbling_anywhere_queues_nothing() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;

        queue(&pool, ana, 1, &db::now()).await;

        assert!(queued(&pool).await.is_empty());
    }

    /// One person's listens are not another's, even when both are switched on.
    #[tokio::test]
    async fn a_listen_goes_only_where_the_listener_sends_it() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        let bea = listener(&pool, "bea", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;
        destination(&pool, bea, Service::Koito, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        let waiting: Vec<(i64, String)> =
            sqlx::query_as("SELECT user_id, service FROM scrobble_queue")
                .fetch_all(&pool)
                .await
                .unwrap();

        assert_eq!(waiting, vec![(ana, "listenbrainz".to_string())]);
    }

    /// A track credited to somebody as the artist and to somebody else as the album
    /// artist scrobbles as the artist: what a compilation credits the record to is
    /// not who played the song.
    #[tokio::test]
    async fn the_artist_wins_over_the_album_artist() {
        let pool = collection().await;
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role) VALUES (1, 1, 'albumartist')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        assert_eq!(queued(&pool).await[0].2, "Porcupine Tree");
    }

    /// And with only an album artist, that is who it was: better a listen credited
    /// to the record's artist than no listen at all.
    #[tokio::test]
    async fn an_album_artist_is_used_when_it_is_all_there_is() {
        let pool = collection().await;
        sqlx::query("DELETE FROM track_artists")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role) VALUES (1, 1, 'albumartist')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        assert_eq!(queued(&pool).await[0].2, "Best of the 2000s");
    }

    /// A file with no credit at all is counted here and sent nowhere. The far end
    /// insists on an artist name, and the alternative is filing somebody's listen
    /// against a band that does not exist.
    #[tokio::test]
    async fn a_song_with_nobody_credited_is_not_queued() {
        let pool = collection().await;
        sqlx::query("DELETE FROM track_artists")
            .execute(&pool)
            .await
            .unwrap();

        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        assert!(queued(&pool).await.is_empty());
    }

    /// Correcting a destination has to undo the punishment for its being wrong.
    ///
    /// Found by doing it rather than by thinking about it: a token typed wrongly
    /// pushes the queue out — five minutes, then fifteen, then hours — and without
    /// this, fixing the token is answered by nothing happening for the rest of the
    /// old wait, which reads exactly like the fix not having worked.
    #[tokio::test]
    async fn setting_a_destination_up_again_makes_its_listens_due() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        // As six failed attempts would have left it.
        let long_from_now = db::from_now(chrono::Duration::hours(3));
        sqlx::query(
            "UPDATE scrobble_queue
                SET next_try_at = ?, attempts = 6, last_error = 'the token was refused'",
        )
        .bind(&long_from_now)
        .execute(&pool)
        .await
        .unwrap();

        due_again(&pool, ana, Service::ListenBrainz).await.unwrap();

        let (next, attempts, last_error): (String, i64, Option<String>) =
            sqlx::query_as("SELECT next_try_at, attempts, last_error FROM scrobble_queue")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert!(next < long_from_now, "it is due now and not in three hours");
        assert_eq!(attempts, 0, "and the count that caused the wait is gone");
        assert_eq!(last_error, None, "as is the reason, which no longer holds");
    }

    /// One person correcting their own destination does not disturb anybody else's,
    /// nor their other destinations.
    #[tokio::test]
    async fn making_one_destination_due_leaves_the_others_waiting() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;
        destination(&pool, ana, Service::Koito, true).await;

        queue(&pool, ana, 1, &db::now()).await;

        let later = db::from_now(chrono::Duration::hours(6));
        sqlx::query("UPDATE scrobble_queue SET next_try_at = ?, attempts = 7")
            .bind(&later)
            .execute(&pool)
            .await
            .unwrap();

        due_again(&pool, ana, Service::Koito).await.unwrap();

        let waiting: Vec<(String, i64)> =
            sqlx::query_as("SELECT service, attempts FROM scrobble_queue ORDER BY service")
                .fetch_all(&pool)
                .await
                .unwrap();

        assert_eq!(waiting[0], ("koito".to_string(), 0));
        assert_eq!(waiting[1], ("listenbrainz".to_string(), 7), "left alone");
    }

    /// Removing a destination takes its unsent listens with it, which is what the
    /// composite foreign key is for: a queue for somewhere nobody is configured to
    /// reach would wait for ever.
    #[tokio::test]
    async fn removing_a_destination_empties_its_queue() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;
        destination(&pool, ana, Service::Koito, true).await;

        queue(&pool, ana, 1, &db::now()).await;
        assert_eq!(queued(&pool).await.len(), 2);

        sqlx::query("DELETE FROM scrobblers WHERE user_id = ? AND service = 'koito'")
            .bind(ana)
            .execute(&pool)
            .await
            .unwrap();

        let waiting = queued(&pool).await;
        assert_eq!(waiting.len(), 1, "and only that one's");
        assert_eq!(waiting[0].0, "listenbrainz");
    }

    /// The time a listen happened is the time it happened, not the time it was
    /// handed over: a client uploading yesterday's cached plays says when, and that
    /// is what has to reach the far end.
    #[tokio::test]
    async fn a_late_play_keeps_the_moment_it_happened() {
        let pool = collection().await;
        let ana = listener(&pool, "ana", true).await;
        destination(&pool, ana, Service::ListenBrainz, true).await;

        let yesterday = db::from_now(chrono::Duration::days(-1));
        queue(&pool, ana, 1, &yesterday).await;

        let (played_at, due): (String, String) =
            sqlx::query_as("SELECT played_at, next_try_at FROM scrobble_queue")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(played_at, yesterday);
        assert!(due > yesterday, "and it is due now rather than then");
    }

    /// The catalogue is read back by name, because that is what the database holds
    /// and what the API sends. A name nothing answers to is nothing.
    #[test]
    fn a_service_answers_to_the_name_it_is_stored_under() {
        assert_eq!(named("listenbrainz"), Some(Service::ListenBrainz));
        assert_eq!(named("koito"), Some(Service::Koito));
        assert_eq!(
            named("ListenBrainz"),
            None,
            "it is the stored name, exactly"
        );
        assert_eq!(named("lastfm"), None);
    }

    /// The hosted service is the API; the self hosted ones keep it in a corner of
    /// their own site. Both end up as one root the dialect can build paths on.
    #[test]
    fn where_a_service_is_asked_depends_on_which_it_is() {
        assert_eq!(
            Service::ListenBrainz.root("https://api.listenbrainz.org"),
            "https://api.listenbrainz.org"
        );
        assert_eq!(
            Service::Koito.root("http://kitchen.lan:4110/"),
            "http://kitchen.lan:4110/apis/listenbrainz"
        );
        assert_eq!(
            Service::Maloja.root("https://maloja.example.org"),
            "https://maloja.example.org/apis/listenbrainz"
        );
    }
}
