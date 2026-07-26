// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! End to end tests of a scan, against a real database.

use super::*;
use std::fs;

/// A database with the schema applied, in memory.
async fn database() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

/// Registers a library and returns its id.
async fn library(pool: &SqlitePool, root: &Path) -> i64 {
    let timestamp = db::now();
    sqlx::query_scalar(
        "INSERT INTO libraries (name, path, created_at, updated_at)
         VALUES ('test', ?, ?, ?) RETURNING id",
    )
    .bind(root.to_string_lossy().as_ref())
    .bind(&timestamp)
    .bind(&timestamp)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A minimal but valid WAV, so lofty reads real audio properties.
fn write_wav(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    let data = [0u8; 4];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&176_400u32.to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);

    fs::write(path, bytes).unwrap();
}

/// Writes tags onto a file already on disk.
fn tag(path: &Path, items: &[(&str, &str)]) {
    use lofty::prelude::{ItemKey, TagExt};
    use lofty::tag::{ItemValue, Tag, TagItem, TagType};

    let mut tag = Tag::new(TagType::Id3v2);
    for (key, value) in items {
        let key = match *key {
            "album" => ItemKey::AlbumTitle,
            "albumartist" => ItemKey::AlbumArtist,
            "artist" => ItemKey::TrackArtist,
            "title" => ItemKey::TrackTitle,
            // Written as a recording date, which is the field that survives a
            // RIFF container.
            "year" => ItemKey::RecordingDate,
            other => panic!("unknown tag {other}"),
        };
        tag.insert(TagItem::new(key, ItemValue::Text(value.to_string())));
    }
    tag.save_to_path(path, Default::default()).unwrap();
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("tocata-scan-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// A scan expected to run to the end, which is every one of these but the last.
/// The interruption flag lives on `Progress`, so a fresh one never trips it.
async fn scan(pool: &SqlitePool, id: i64, root: &Path, mode: Mode) -> Result<Outcome> {
    Ok(scan_library(pool, id, root, mode, &Progress::default())
        .await?
        .expect("the scan ran to the end"))
}

async fn count(pool: &SqlitePool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn a_second_incremental_scan_reads_nothing_again() {
    let root = temp_root("incremental");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;

    let first = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(first.tracks, 5);
    assert_eq!(first.unchanged, 0, "nothing was known yet");

    let second = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(second.tracks, 5);
    assert_eq!(second.unchanged, 5, "no file changed, so none was reopened");
    assert_eq!(second.gone, 0);

    assert_eq!(count(&pool, "SELECT count(*) FROM tracks").await, 5);
}

#[tokio::test]
async fn a_full_scan_reads_everything_again() {
    let root = temp_root("full");
    write_wav(&root.join("Album/one.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    let full = scan(&pool, id, &root, Mode::Full).await.unwrap();

    assert_eq!(full.unchanged, 0, "a full scan skips nothing");
    assert_eq!(full.tracks, 1);
}

#[tokio::test]
async fn a_changed_file_is_read_again() {
    let root = temp_root("changed");
    let path = root.join("Album/one.wav");
    write_wav(&path);

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // Growing the file changes its size, which is half of what is compared.
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(&[0u8; 64]);
    fs::write(&path, bytes).unwrap();

    let second = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(second.unchanged, 0, "the size changed, so it was reopened");
}

#[tokio::test]
async fn a_deleted_file_is_marked_not_removed() {
    let root = temp_root("deleted");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_file(root.join("Album/0.wav")).unwrap();

    let second = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(second.gone, 1);

    // Still there, just marked.
    assert_eq!(count(&pool, "SELECT count(*) FROM tracks").await, 5);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_file_that_comes_back_is_unmarked() {
    let root = temp_root("returns");
    let path = root.join("Album/one.wav");
    write_wav(&path);

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_file(&path).unwrap();
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        1
    );

    write_wav(&path);
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        0,
        "the file is back, so the mark comes off"
    );
}

/// The point of the whole marked-not-deleted design: a library reorganisation
/// must not cost the user their data.
#[tokio::test]
async fn a_moved_file_keeps_its_identity_and_user_data() {
    let root = temp_root("moved");
    let original = root.join("Wrong Folder/song.wav");
    write_wav(&original);

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let (track_id, public_id): (i64, String) =
        sqlx::query_as("SELECT id, public_id FROM tracks LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

    // Give the user something to lose.
    let timestamp = db::now();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, created_at, updated_at)
         VALUES (1, 'listener', 'hash', ?, ?)",
    )
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_track_stats (user_id, track_id, play_count, rating, starred_at)
         VALUES (1, ?, 42, 5, ?)",
    )
    .bind(track_id)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();

    // Reorganise: same file, new home.
    let moved = root.join("Right Folder/Album/01 song.wav");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&original, &moved).unwrap();

    let outcome = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(outcome.gone, 0, "nothing actually went away");

    let rows: Vec<(i64, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, public_id, path, missing_since FROM tracks")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(rows.len(), 1, "the move must not create a second row");
    assert_eq!(rows[0].0, track_id, "same row");
    assert_eq!(rows[0].1, public_id, "same identifier for the client");
    assert_eq!(rows[0].2, moved.to_string_lossy());
    assert_eq!(rows[0].3, None, "not missing any more");

    let (plays, rating): (i64, i64) =
        sqlx::query_as("SELECT play_count, rating FROM user_track_stats WHERE track_id = ?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((plays, rating), (42, 5), "the user data came along");
}

/// An unmounted disk looks exactly like somebody deleting everything, and the
/// difference matters.
#[tokio::test]
async fn a_library_that_vanishes_wholesale_is_left_alone() {
    let root = temp_root("vanished");
    for n in 0..20 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // The mount point is there but empty, which is what a failed mount looks
    // like from up here.
    fs::remove_dir_all(root.join("Album")).unwrap();

    let outcome = scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(outcome.gone, 0, "the sweep refused to run");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        0,
        "nothing was marked"
    );
}

/// Below the minimum, the fraction means nothing: a two track library losing
/// both tracks is a deletion, not a failed mount.
#[tokio::test]
async fn a_tiny_library_is_still_swept() {
    let root = temp_root("tiny");
    write_wav(&root.join("Album/one.wav"));
    write_wav(&root.join("Album/two.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_file(root.join("Album/one.wav")).unwrap();
    fs::remove_file(root.join("Album/two.wav")).unwrap();

    let outcome = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(outcome.gone, 2);
}

#[tokio::test]
async fn a_folder_that_goes_away_is_marked_too() {
    let root = temp_root("folder-gone");
    write_wav(&root.join("Keep/one.wav"));
    write_wav(&root.join("Remove/two.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_dir_all(root.join("Remove")).unwrap();
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let marked: Vec<String> =
        sqlx::query_scalar("SELECT name FROM folders WHERE missing_since IS NOT NULL")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(marked, vec!["Remove".to_string()]);
}

#[test]
fn only_one_scan_can_hold_the_flag() {
    let progress = Progress::default();
    assert!(!progress.is_scanning());

    let running = progress.begin().expect("the first claim wins");
    assert!(progress.is_scanning());
    assert!(
        progress.begin().is_none(),
        "a second claim must be refused while the first is held"
    );

    drop(running);
    assert!(
        !progress.is_scanning(),
        "the flag clears when the scan ends"
    );
    assert!(progress.begin().is_some(), "and the next scan can claim it");
}

#[tokio::test]
async fn a_second_scan_request_while_one_runs_does_nothing() {
    let root = temp_root("concurrent");
    write_wav(&root.join("Album/one.wav"));

    let pool = database().await;
    library(&pool, &root).await;

    let progress = Progress::default();
    let _running = progress.begin().unwrap();

    // With the flag held, scan_all declines rather than queueing a second pass.
    let outcome = scan_all(&pool, Mode::Incremental, &progress).await.unwrap();
    assert!(outcome.is_none());

    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM scan_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 0, "it must not even record a run");
}

/// The case a stray retag uncovered: the year is part of the album key so an
/// original and its remaster stay apart, but a track missing the year must not
/// start an album of its own.
#[tokio::test]
async fn a_missing_year_does_not_split_an_album() {
    let root = temp_root("year");
    let with_year = root.join("Album/01.wav");
    let without = root.join("Album/02.wav");
    write_wav(&with_year);
    write_wav(&without);

    tag(
        &with_year,
        &[
            ("album", "The Wall"),
            ("albumartist", "Pink Floyd"),
            ("year", "1979"),
        ],
    );
    tag(
        &without,
        &[("album", "The Wall"), ("albumartist", "Pink Floyd")],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let albums: Vec<(String, Option<i64>)> = sqlx::query_as("SELECT name, year FROM albums")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(albums.len(), 1, "one album, not one per year: {albums:?}");
    assert_eq!(albums[0].1, Some(1979), "the known year survives");

    let counted: i64 = count(
        &pool,
        "SELECT count(*) FROM tracks WHERE album_id IS NOT NULL",
    )
    .await;
    assert_eq!(counted, 2, "both tracks belong to it");
}

/// And the other direction: the yearless track arriving first must not leave the
/// album without a year once one shows up.
#[tokio::test]
async fn a_year_arriving_late_is_filled_in() {
    let root = temp_root("year-late");
    let without = root.join("Album/01.wav");
    let with_year = root.join("Album/02.wav");
    write_wav(&without);
    write_wav(&with_year);

    tag(
        &without,
        &[("album", "Rumours"), ("albumartist", "Fleetwood Mac")],
    );
    tag(
        &with_year,
        &[
            ("album", "Rumours"),
            ("albumartist", "Fleetwood Mac"),
            ("year", "1977"),
        ],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let albums: Vec<(String, Option<i64>)> = sqlx::query_as("SELECT name, year FROM albums")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(albums.len(), 1, "still one album: {albums:?}");
    assert_eq!(
        albums[0].1,
        Some(1977),
        "the year turned up and was recorded"
    );
}

/// Two editions that both say which year they are do stay apart, which is the
/// reason the year is in the key at all.
#[tokio::test]
async fn two_tagged_years_remain_two_albums() {
    let root = temp_root("year-two");
    let original = root.join("Original/01.wav");
    let remaster = root.join("Remaster/01.wav");
    write_wav(&original);
    write_wav(&remaster);

    tag(
        &original,
        &[
            ("album", "Rumours"),
            ("albumartist", "Fleetwood Mac"),
            ("year", "1977"),
        ],
    );
    tag(
        &remaster,
        &[
            ("album", "Rumours"),
            ("albumartist", "Fleetwood Mac"),
            ("year", "2004"),
        ],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(count(&pool, "SELECT count(*) FROM albums").await, 2);
}

#[tokio::test]
async fn an_interrupted_scan_writes_nothing() {
    let root = temp_root("interrupted");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;

    let progress = Progress::default();
    progress.stop();

    let outcome = scan_library(&pool, id, &root, Mode::Incremental, &progress)
        .await
        .unwrap();

    assert!(outcome.is_none(), "the scan gave up rather than finishing");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM tracks").await,
        0,
        "the transaction was dropped, not committed"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM folders").await, 0);
}

#[tokio::test]
async fn an_interrupted_scan_marks_nothing_missing() {
    let root = temp_root("interrupted-sweep");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // The danger the rollback exists for: a scan that stopped early has not seen
    // most of the library, and sweeping on the way out would call all of it gone.
    let progress = Progress::default();
    progress.stop();
    scan_library(&pool, id, &root, Mode::Incremental, &progress)
        .await
        .unwrap();

    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        0
    );
}
