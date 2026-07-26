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

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("tocata-scan-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
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

    let first = scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
    assert_eq!(first.tracks, 5);
    assert_eq!(first.unchanged, 0, "nothing was known yet");

    let second = scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
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

    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
    let full = scan_library(&pool, id, &root, Mode::Full).await.unwrap();

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
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

    // Growing the file changes its size, which is half of what is compared.
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(&[0u8; 64]);
    fs::write(&path, bytes).unwrap();

    let second = scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
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
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

    fs::remove_file(root.join("Album/0.wav")).unwrap();

    let second = scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
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
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

    fs::remove_file(&path).unwrap();
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        1
    );

    write_wav(&path);
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
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
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

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

    let outcome = scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
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
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

    // The mount point is there but empty, which is what a failed mount looks
    // like from up here.
    fs::remove_dir_all(root.join("Album")).unwrap();

    let outcome = scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

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
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

    fs::remove_file(root.join("Album/one.wav")).unwrap();
    fs::remove_file(root.join("Album/two.wav")).unwrap();

    let outcome = scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();
    assert_eq!(outcome.gone, 2);
}

#[tokio::test]
async fn a_folder_that_goes_away_is_marked_too() {
    let root = temp_root("folder-gone");
    write_wav(&root.join("Keep/one.wav"));
    write_wav(&root.join("Remove/two.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

    fs::remove_dir_all(root.join("Remove")).unwrap();
    scan_library(&pool, id, &root, Mode::Incremental)
        .await
        .unwrap();

    let marked: Vec<String> =
        sqlx::query_scalar("SELECT name FROM folders WHERE missing_since IS NOT NULL")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(marked, vec!["Remove".to_string()]);
}
