// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Bookmarks and the play queue: where somebody left off.
//!
//! Both are per user and irreplaceable, so they sit on the user side of the
//! schema. A bookmark marks a position inside one track — for an audiobook or a
//! long mix — while the play queue is what a listener had lined up, so they can
//! carry on from another device.

use super::asked::Asked;
use super::asked::Repeated;
use super::auth::Authenticated;
use super::browsing;
use super::error::ApiError;
use super::models::Child;
use super::response::{self, Empty};
use crate::db;
use crate::db::InTurn;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::error;

/// A bookmark as the database keeps it: which track, where in it, and when.
type BookmarkRow = (i64, i64, Option<String>, String, String);

/// The saved queue itself: which place in it is playing, where in that track,
/// when it was saved and by whom.
type QueueRow = (Option<i64>, i64, String, String);

/// One entry of a saved queue: its place in the list, and the track there said
/// both ways, because the two endpoints name it differently.
type EntryRow = (i64, i64, String);

#[derive(Debug, Deserialize)]
pub struct IdQuery {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBookmarkQuery {
    id: String,
    /// Milliseconds into the track.
    position: i64,
    comment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveQueueQuery {
    /// The tracks in order. Repeats, and may be empty to clear the queue.
    #[serde(default)]
    id: Vec<String>,
    /// Which of them is playing.
    current: Option<String>,
    /// Milliseconds into the current track.
    position: Option<i64>,
}

/// The same save, with the current track named by its place instead of by its id.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveByIndexQuery {
    #[serde(default)]
    id: Vec<String>,
    /// Zero based, into the list above as the client sent it. Required when there
    /// is a list, and forbidden when there is not: a save with neither is how the
    /// extension says "forget my queue", and an index into nothing is a mistake
    /// worth telling the client about rather than storing.
    current_index: Option<i64>,
    position: Option<i64>,
}

#[derive(Serialize)]
struct BookmarksBody {
    bookmarks: Bookmarks,
}

#[derive(Serialize)]
struct Bookmarks {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bookmark: Vec<Bookmark>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bookmark {
    position: i64,
    username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    created: String,
    changed: String,
    entry: Child,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayQueueBody {
    play_queue: PlayQueue,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayQueue {
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<i64>,
    username: String,
    changed: String,
    /// Which client saved it, so a listener can tell where they left off.
    changed_by: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entry: Vec<Child>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayQueueByIndexBody {
    play_queue_by_index: PlayQueueByIndex,
}

/// The same queue, with the current track named by its place in it.
///
/// A separate element and not a field added to the one above, because that is how
/// the extension defines it: a client that knows nothing of `indexBasedQueue` asks
/// the old endpoint and must get exactly what it got before.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayQueueByIndex {
    #[serde(skip_serializing_if = "Option::is_none")]
    current_index: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<i64>,
    username: String,
    changed: String,
    changed_by: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entry: Vec<Child>,
}

pub async fn get_bookmarks(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let rows: Result<Vec<BookmarkRow>, _> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT b.track_id, b.position_ms, b.comment, b.created_at, b.updated_at
           FROM bookmarks b
           JOIN tracks t ON t.id = b.track_id
          WHERE b.user_id = ? AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
          ORDER BY b.updated_at DESC"
    ))
    .bind(auth.user.id)
    .bind(auth.user.id)
    .fetch_all(&pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return internal(e, auth.format, "listing bookmarks"),
    };

    let ids: Vec<i64> = rows.iter().map(|(id, _, _, _, _)| *id).collect();
    let entries = match browsing::load_tracks_by_ids(&pool, auth.user.id, &ids).await {
        Ok(entries) => entries,
        Err(e) => return internal(e, auth.format, "loading bookmarked tracks"),
    };

    // The loader returns one entry per id in order, so zipping lines up.
    let bookmark = entries
        .into_iter()
        .zip(rows)
        .map(
            |(entry, (_, position, comment, created, changed))| Bookmark {
                position,
                username: auth.user.username.clone(),
                comment,
                created,
                changed,
                entry,
            },
        )
        .collect();

    response::ok(
        auth.format,
        BookmarksBody {
            bookmarks: Bookmarks { bookmark },
        },
    )
}

pub async fn create_bookmark(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<CreateBookmarkQuery>,
) -> Response {
    let track_id: Result<Option<i64>, _> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT id FROM tracks WHERE public_id = ? AND missing_since IS NULL
        AND library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(auth.user.id)
    .bind(&query.id)
    .fetch_optional(&pool)
    .await;

    let track_id = match track_id {
        Ok(Some(id)) => id,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => return internal(e, auth.format, "looking up a track to bookmark"),
    };

    let timestamp = db::now();
    let written = sqlx::query(
        "INSERT INTO bookmarks (user_id, track_id, position_ms, comment, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, track_id) DO UPDATE SET
             position_ms = excluded.position_ms,
             comment = excluded.comment,
             updated_at = excluded.updated_at",
    )
    .bind(auth.user.id)
    .bind(track_id)
    .bind(query.position.max(0))
    .bind(&query.comment)
    .bind(&timestamp)
    .bind(&timestamp)
    .in_turn(&pool)
    .await;

    match written {
        Ok(_) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "saving a bookmark"),
    }
}

pub async fn delete_bookmark(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Asked(query): Asked<IdQuery>,
) -> Response {
    let deleted = sqlx::query(
        "DELETE FROM bookmarks
          WHERE user_id = ?
            AND track_id = (SELECT id FROM tracks WHERE public_id = ?)",
    )
    .bind(auth.user.id)
    .bind(&query.id)
    .in_turn(&pool)
    .await;

    match deleted {
        // Deleting one that was not there is not a failure: the caller wanted it
        // gone and it is gone.
        Ok(_) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "deleting a bookmark"),
    }
}

/// A saved queue, read once for both of the ways it can be asked for.
///
/// One function because the two endpoints differ in a single word of their answer
/// and must never differ in anything else. Written twice they would drift, and the
/// drift would be a client that saves through one and reads through the other
/// getting a different song — which is the exact failure the extension exists to
/// prevent.
struct Saved {
    /// Where the current track sits in `entry`, and which track that is. Absent
    /// when nothing is playing, and also when what was playing is no longer
    /// something this account can see.
    playing: Option<Playing>,
    position_ms: i64,
    changed: String,
    changed_by: String,
    entry: Vec<Child>,
}

/// The current track said both ways at once, from one list, so the two cannot
/// disagree.
struct Playing {
    /// Its place in the queue as answered — not the place it was saved at. Those
    /// differ whenever an entry in front of it has since gone from disk or into a
    /// library this account may not look in, and the client is owed the index that
    /// is right for the list it is being handed.
    index: i64,
    id: String,
}

async fn read_queue(pool: &SqlitePool, auth: &Authenticated) -> Result<Option<Saved>, sqlx::Error> {
    let queue: Option<QueueRow> = sqlx::query_as(
        "SELECT current_position, position_ms, changed_at, changed_by
           FROM play_queues WHERE user_id = ?",
    )
    .bind(auth.user.id)
    .fetch_optional(pool)
    .await?;

    let Some((current_position, position_ms, changed, changed_by)) = queue else {
        return Ok(None);
    };

    let rows: Vec<EntryRow> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT q.position, q.track_id, t.public_id
           FROM play_queue_tracks q
           JOIN tracks t ON t.id = q.track_id
          WHERE q.user_id = ? AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
          ORDER BY q.position"
    ))
    .bind(auth.user.id)
    .bind(auth.user.id)
    .fetch_all(pool)
    .await?;

    let ids: Vec<i64> = rows.iter().map(|(_, track_id, _)| *track_id).collect();
    let entry = browsing::load_tracks_by_ids(pool, auth.user.id, &ids).await?;

    // The place the current track ended up in the answer, which is what both
    // endpoints report from. Nothing is reported at all if the two statements
    // above came back different lengths: they ask the same question of the same
    // rows and always agree, and on the day they do not, an index counted against
    // one list and handed out with the other names the wrong song. No answer beats
    // a wrong one here, because the client acts on it.
    let playing = match entry.len() == rows.len() {
        true => rows
            .iter()
            .position(|(place, ..)| Some(*place) == current_position)
            .map(|index| Playing {
                index: index as i64,
                id: rows[index].2.clone(),
            }),
        false => None,
    };

    Ok(Some(Saved {
        playing,
        position_ms,
        changed,
        changed_by,
        entry,
    }))
}

pub async fn get_play_queue(auth: Authenticated, State(pool): State<SqlitePool>) -> Response {
    let saved = match read_queue(&pool, &auth).await {
        Ok(saved) => saved,
        Err(e) => return internal(e, auth.format, "loading the play queue"),
    };

    // Nothing saved yet. An empty element rather than a 70: not having a queue is
    // a state, not a failure.
    let Some(saved) = saved else {
        return response::ok(
            auth.format,
            PlayQueueBody {
                play_queue: PlayQueue {
                    current: None,
                    position: None,
                    username: auth.user.username,
                    changed: db::now(),
                    changed_by: auth.client,
                    entry: Vec::new(),
                },
            },
        );
    };

    response::ok(
        auth.format,
        PlayQueueBody {
            play_queue: PlayQueue {
                // By its public id, which is what the client sent and what it
                // will send back.
                current: saved.playing.map(|playing| playing.id),
                position: Some(saved.position_ms),
                username: auth.user.username,
                changed: saved.changed,
                changed_by: saved.changed_by,
                entry: saved.entry,
            },
        },
    )
}

pub async fn get_play_queue_by_index(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
) -> Response {
    let saved = match read_queue(&pool, &auth).await {
        Ok(saved) => saved,
        Err(e) => return internal(e, auth.format, "loading the play queue"),
    };

    let Some(saved) = saved else {
        return response::ok(
            auth.format,
            PlayQueueByIndexBody {
                play_queue_by_index: PlayQueueByIndex {
                    current_index: None,
                    position: None,
                    username: auth.user.username,
                    changed: db::now(),
                    changed_by: auth.client,
                    entry: Vec::new(),
                },
            },
        );
    };

    response::ok(
        auth.format,
        PlayQueueByIndexBody {
            play_queue_by_index: PlayQueueByIndex {
                current_index: saved.playing.map(|playing| playing.index),
                position: Some(saved.position_ms),
                username: auth.user.username,
                changed: saved.changed,
                changed_by: saved.changed_by,
                entry: saved.entry,
            },
        },
    )
}

pub async fn save_play_queue(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<SaveQueueQuery>,
) -> Response {
    let named = Current::Named(query.current.as_deref());

    match write_queue(&pool, &auth, &query.id, named, query.position).await {
        Ok(()) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "saving the play queue"),
    }
}

pub async fn save_play_queue_by_index(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Repeated(query): Repeated<SaveByIndexQuery>,
) -> Response {
    let placed = match placed_current(&query) {
        Ok(placed) => placed,
        Err(refused) => return refused.in_format(auth.format).into_response(),
    };

    match write_queue(&pool, &auth, &query.id, placed, query.position).await {
        Ok(()) => response::ok(auth.format, Empty {}),
        Err(e) => internal(e, auth.format, "saving the play queue"),
    }
}

/// Reads the index the extension asks for, or says why it cannot be used.
///
/// Checked here and not in the writer because these are answers about the request
/// rather than about the queue, and a request that contradicts itself should be
/// told so before anything is written.
fn placed_current(query: &SaveByIndexQuery) -> Result<Current<'_>, ApiError> {
    match (query.id.is_empty(), query.current_index) {
        // Clearing the queue, which is what a save with no tracks means.
        (true, None) => Ok(Current::Nothing),
        (true, Some(_)) => Err(ApiError::UnreadableParameter(
            "Parameter currentIndex names a place in a queue that has no tracks".into(),
        )),
        // The extension makes it required as soon as there is a list, and it is
        // right to: a queue with songs in it and nothing playing is not a state a
        // client meant to describe.
        (false, None) => Err(ApiError::MissingParameter("currentIndex")),
        (false, Some(index)) => match index >= 0 && (index as usize) < query.id.len() {
            true => Ok(Current::Placed(index)),
            false => Err(ApiError::UnreadableParameter(format!(
                "Parameter currentIndex must be between 0 and {}",
                query.id.len() - 1
            ))),
        },
    }
}

/// Which entry of the list just sent is the one playing, in the words of whichever
/// endpoint sent it.
enum Current<'a> {
    /// A track id, which is what the original endpoint takes. A queue holding that
    /// track twice cannot say which of the two, and the first is the answer — that
    /// ambiguity is the whole reason the other form exists.
    Named(Option<&'a str>),
    /// A place in the list as the client sent it. What gets stored is the position
    /// the entry there ended up at, which is a different number as soon as an id
    /// in front of it named nothing.
    Placed(i64),
    /// Nothing is playing.
    Nothing,
}

async fn write_queue(
    pool: &SqlitePool,
    auth: &Authenticated,
    ids: &[String],
    current: Current<'_>,
    position_ms: Option<i64>,
) -> Result<(), sqlx::Error> {
    let mut tx = crate::db::writing(pool).await?;

    // The queue row first, because the entries carry a foreign key to it: the
    // entries cannot exist before the queue they belong to. The current place is
    // filled in afterwards, once it is known which position it landed at.
    sqlx::query(
        "INSERT INTO play_queues (user_id, position_ms, changed_at, changed_by)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET
             position_ms = excluded.position_ms,
             changed_at = excluded.changed_at,
             changed_by = excluded.changed_by",
    )
    .bind(auth.user.id)
    .bind(position_ms.unwrap_or(0).max(0))
    .bind(db::now())
    .bind(&auth.client)
    .execute(&mut **tx)
    .await?;

    // Rewritten whole, like a playlist and for the same reason: the key is
    // (user, position), and shifting positions in place violates it.
    sqlx::query("DELETE FROM play_queue_tracks WHERE user_id = ?")
        .bind(auth.user.id)
        .execute(&mut **tx)
        .await?;

    let mut current_position = None;
    let mut position = 0i64;

    for (sent, public_id) in ids.iter().enumerate() {
        let id: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE public_id = ?")
            .bind(public_id)
            .fetch_optional(&mut **tx)
            .await?;

        // An id naming nothing is left out, and the entries behind it move up. So
        // the place a client counted in its own list is not the place stored, which
        // is why the two forms are told apart before this loop rather than after
        // it: by the time anything is stored, both mean the same thing.
        let Some(id) = id else { continue };

        let is_current = match current {
            Current::Named(Some(named)) => named == public_id.as_str(),
            Current::Placed(index) => sent as i64 == index,
            Current::Named(None) | Current::Nothing => false,
        };

        // The first match, and no later one replaces it: a track sent twice with
        // the id form names its first appearance.
        if is_current && current_position.is_none() {
            current_position = Some(position);
        }

        sqlx::query("INSERT INTO play_queue_tracks (user_id, position, track_id) VALUES (?, ?, ?)")
            .bind(auth.user.id)
            .bind(position)
            .bind(id)
            .execute(&mut **tx)
            .await?;

        position += 1;
    }

    sqlx::query("UPDATE play_queues SET current_position = ? WHERE user_id = ?")
        .bind(current_position)
        .bind(auth.user.id)
        .execute(&mut **tx)
        .await?;

    tx.commit().await
}

fn internal(error: sqlx::Error, format: response::Format, doing: &str) -> Response {
    error!("{doing}: {error}");
    ApiError::Internal.in_format(format).into_response()
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    use crate::user::User;

    /// Three songs in one library, and somebody to own a queue.
    ///
    /// A folder because a track is loaded through one; no album, because a track
    /// filed under none still belongs in a queue and the loader left joins it.
    async fn a_listener() -> (SqlitePool, Authenticated) {
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
             VALUES (1, 'fold', 1, 'music', '', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (id, name) in [(1, "one"), (2, "two"), (3, "three")] {
            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                     file_modified_at, content_type, suffix, title,
                                     last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, 1, 1, ?, 1, ?, 'audio/wav', 'wav', ?, 1, ?, ?)",
            )
            .bind(id)
            .bind(format!("trk{id}"))
            .bind(format!("/music/{name}.wav"))
            .bind(&at)
            .bind(format!("Song {name}"))
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&at)
        .bind(&at)
        .fetch_one(&pool)
        .await
        .unwrap();

        let auth = Authenticated {
            user: User {
                id: user_id,
                username: "ana".into(),
                is_admin: false,
            },
            format: response::Format::Json,
            client: "the test".into(),
        };

        (pool, auth)
    }

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// The whole point of the extension: the same song twice, and the second one
    /// is the one sounding.
    ///
    /// Saved by id this cannot be said at all — `current=trk1` names the first of
    /// them and there is nothing else to name. Saved by index it can, and what
    /// comes back has to be the second: an index of 0 here would restart a record
    /// somebody was halfway through the second time round.
    #[tokio::test]
    async fn the_same_song_twice_is_told_apart_by_its_place() {
        let (pool, auth) = a_listener().await;

        write_queue(
            &pool,
            &auth,
            &ids(&["trk1", "trk2", "trk1"]),
            Current::Placed(2),
            Some(4_000),
        )
        .await
        .unwrap();

        let saved = read_queue(&pool, &auth).await.unwrap().unwrap();
        let playing = saved.playing.expect("something is playing");

        assert_eq!(playing.index, 2, "the second copy, not the first");
        assert_eq!(playing.id, "trk1", "and it is that song");
        assert_eq!(saved.entry.len(), 3, "the queue holds the repeat");
        assert_eq!(saved.position_ms, 4_000);
    }

    /// An id naming nothing shifts everything behind it, and the index still lands
    /// on the song the client meant.
    ///
    /// This is what makes the two forms different to store. The client counted 2
    /// in its own list of three; what gets stored is position 1, because the first
    /// entry resolved to nothing and was left out. Storing the number as sent
    /// would hand back the wrong song, and nothing else in the answer would look
    /// wrong.
    #[tokio::test]
    async fn an_id_that_names_nothing_does_not_move_what_is_playing() {
        let (pool, auth) = a_listener().await;

        write_queue(
            &pool,
            &auth,
            &ids(&["gone", "trk2", "trk3"]),
            Current::Placed(2),
            None,
        )
        .await
        .unwrap();

        let saved = read_queue(&pool, &auth).await.unwrap().unwrap();
        let playing = saved.playing.expect("something is playing");

        assert_eq!(
            saved.entry.len(),
            2,
            "the one that resolved to nothing is out"
        );
        assert_eq!(playing.id, "trk3", "the song the client pointed at");
        assert_eq!(playing.index, 1, "at the place it actually occupies now");
    }

    /// Saved one way, read the other, and both name the same song.
    ///
    /// The two endpoints answer from one reading precisely so this holds: a client
    /// that saves with the old call and a client that reads with the new one are
    /// two clients of the same person, and a queue that resumed somewhere else
    /// depending on which one asked would be worse than no queue at all.
    #[tokio::test]
    async fn saved_by_id_and_read_by_place_agree() {
        let (pool, auth) = a_listener().await;

        write_queue(
            &pool,
            &auth,
            &ids(&["trk1", "trk2", "trk3"]),
            Current::Named(Some("trk2")),
            None,
        )
        .await
        .unwrap();

        let saved = read_queue(&pool, &auth).await.unwrap().unwrap();
        let playing = saved.playing.expect("something is playing");

        assert_eq!(playing.id, "trk2");
        assert_eq!(playing.index, 1);
    }

    /// What was playing has gone from disk: it leaves the queue, and no index is
    /// reported rather than one naming whatever moved up into its place.
    #[tokio::test]
    async fn a_current_track_that_went_away_leaves_no_index_behind() {
        let (pool, auth) = a_listener().await;

        write_queue(
            &pool,
            &auth,
            &ids(&["trk1", "trk2", "trk3"]),
            Current::Placed(1),
            None,
        )
        .await
        .unwrap();

        sqlx::query("UPDATE tracks SET missing_since = ? WHERE public_id = 'trk2'")
            .bind(db::now())
            .execute(&pool)
            .await
            .unwrap();

        let saved = read_queue(&pool, &auth).await.unwrap().unwrap();

        assert_eq!(saved.entry.len(), 2, "the file that went is not offered");
        assert!(
            saved.playing.is_none(),
            "and nothing is said to be playing, rather than the song that moved up"
        );
    }

    /// A queue nobody has saved is a state and not a failure, and the reader says
    /// so by finding nothing at all.
    #[tokio::test]
    async fn no_queue_saved_is_not_an_empty_queue() {
        let (pool, auth) = a_listener().await;

        assert!(read_queue(&pool, &auth).await.unwrap().is_none());
    }

    /// The names that travel, both ways: the parameter a client sends and the
    /// element it reads back.
    ///
    /// Pinned because getting one wrong fails silently and has already happened
    /// twice in this project. `currentIndex` arrives camel cased and the field is
    /// snake cased, so without the rename serde looks for `current_index`, never
    /// finds it, and every save is refused for a missing index while the client
    /// swears it sent one — which is exactly how `apiKey` went wrong.
    #[test]
    fn the_names_a_client_uses_are_the_names_that_are_read() {
        let asked: SaveByIndexQuery =
            serde_html_form::from_str("id=trk1&id=trk2&currentIndex=1&position=9000").unwrap();

        assert_eq!(asked.id, ["trk1", "trk2"], "repeats arrive in order");
        assert_eq!(asked.current_index, Some(1));
        assert_eq!(asked.position, Some(9_000));

        let body = serde_json::to_value(PlayQueueByIndexBody {
            play_queue_by_index: PlayQueueByIndex {
                current_index: Some(1),
                position: Some(9_000),
                username: "ana".into(),
                changed: "now".into(),
                changed_by: "the test".into(),
                entry: Vec::new(),
            },
        })
        .unwrap();

        assert!(body.get("playQueueByIndex").is_some(), "got {body}");
        assert_eq!(body["playQueueByIndex"]["currentIndex"], 1);

        // Nothing playing leaves the field out rather than sending a null, which is
        // what the specification means by optional.
        let empty = serde_json::to_value(PlayQueueByIndexBody {
            play_queue_by_index: PlayQueueByIndex {
                current_index: None,
                position: None,
                username: "ana".into(),
                changed: "now".into(),
                changed_by: "the test".into(),
                entry: Vec::new(),
            },
        })
        .unwrap();

        assert!(empty["playQueueByIndex"].get("currentIndex").is_none());
        assert!(empty["playQueueByIndex"].get("entry").is_none());
    }

    /// The rules the extension puts on `currentIndex`, which are about the request
    /// and are answered before anything is written.
    #[test]
    fn an_index_is_checked_against_the_list_it_indexes() {
        let asked = |id: &[&str], current_index: Option<i64>| SaveByIndexQuery {
            id: ids(id),
            current_index,
            position: None,
        };

        // Nothing at all clears the queue, which is what the extension says a save
        // with no tracks means.
        assert!(matches!(
            placed_current(&asked(&[], None)),
            Ok(Current::Nothing)
        ));

        // A place in a queue with no tracks is not a place.
        assert!(matches!(
            placed_current(&asked(&[], Some(0))),
            Err(ApiError::UnreadableParameter(_))
        ));

        // Required as soon as there is a list.
        assert!(matches!(
            placed_current(&asked(&["trk1"], None)),
            Err(ApiError::MissingParameter("currentIndex"))
        ));

        assert!(matches!(
            placed_current(&asked(&["trk1", "trk2"], Some(1))),
            Ok(Current::Placed(1))
        ));

        // Past the end, and below the beginning.
        for out in [2, -1] {
            assert!(
                matches!(
                    placed_current(&asked(&["trk1", "trk2"], Some(out))),
                    Err(ApiError::UnreadableParameter(_))
                ),
                "{out} is not a place in a queue of two"
            );
        }
    }
}
