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

use crate::fixtures::write_wav;

/// Writes tags onto a file already on disk, in this module's own vocabulary.
///
/// The words rather than lofty's keys, because every test here reads better for it,
/// and ID3v2 because that is what a RIFF container will hold.
fn tag(path: &Path, items: &[(&str, &str)]) {
    use lofty::prelude::ItemKey;

    let items: Vec<(ItemKey, &str)> = items
        .iter()
        .map(|(key, value)| {
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

            (key, *value)
        })
        .collect();

    crate::fixtures::tag_file(path, lofty::tag::TagType::Id3v2, &items);
}

fn temp_root(name: &str) -> PathBuf {
    crate::fixtures::temp_root(&format!("scan-{name}"))
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
    // Relative to the library root, which is the whole point of storing it that
    // way: the row says where the file is inside the library and not which
    // directory the library happened to be in.
    assert_eq!(rows[0].2, "Right Folder/Album/01 song.wav");
    assert!(
        !rows[0].2.starts_with('/'),
        "a stored path that is absolute would tie the row to this machine"
    );
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

/// The shape most hand-tagged music arrives in: an artist on every track and no
/// album artist anywhere. The record used to come out signed by nobody, which a
/// listing has nothing to print for and sorts under no name at all.
#[tokio::test]
async fn an_album_with_no_album_artist_is_credited_to_its_artist() {
    let root = temp_root("no-album-artist");
    let one = root.join("Album/01.wav");
    let two = root.join("Album/02.wav");
    write_wav(&one);
    write_wav(&two);

    for path in [&one, &two] {
        tag(
            path,
            &[
                ("album", "Selected Ambient Works"),
                ("artist", "Aphex Twin"),
            ],
        );
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let credited: Vec<String> = sqlx::query_scalar(
        "SELECT ar.name FROM album_artists aa
           JOIN artists ar ON ar.id = aa.artist_id
          WHERE aa.role = 'albumartist'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    // One row and not two: both tracks credit the same name, and the record is
    // signed once however many tracks arrive saying so.
    assert_eq!(
        credited,
        ["Aphex Twin".to_string()],
        "the name on the files signs the record"
    );

    // And it can be found by that name, which is the other half of a credit.
    let found = count(
        &pool,
        "SELECT count(*) FROM albums_fts WHERE albums_fts MATCH 'Aphex'",
    )
    .await;
    assert_eq!(found, 1, "the record answers to who made it");
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
    progress.cancel();

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
    progress.cancel();
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

#[tokio::test]
async fn cancelling_one_scan_does_not_stop_the_next() {
    let root = temp_root("cancel-then-scan");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    library(&pool, &root).await;

    // Through scan_all, because the flag is cleared by the guard that ends a
    // scan and that guard is what scan_all holds. A cancelled run must not leave
    // the server refusing to scan again.
    let progress = Progress::default();
    progress.cancel();
    assert!(
        scan_all(&pool, Mode::Incremental, &progress)
            .await
            .unwrap()
            .is_none(),
        "the first scan gave up"
    );

    let outcome = scan_all(&pool, Mode::Incremental, &progress)
        .await
        .unwrap()
        .expect("the second scan ran to the end");

    assert_eq!(outcome.tracks, 5);
    assert_eq!(count(&pool, "SELECT count(*) FROM tracks").await, 5);
}

#[tokio::test]
async fn a_finished_scan_reports_what_it_did() {
    let root = temp_root("snapshot");
    for n in 0..3 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    library(&pool, &root).await;

    let progress = Progress::default();
    scan_all(&pool, Mode::Incremental, &progress).await.unwrap();

    let snapshot = progress.snapshot();
    assert!(!snapshot.scanning);
    assert!(!snapshot.cancelled);
    assert_eq!(snapshot.tracks, 3);
    // The root counts as a folder as well as the album inside it.
    assert_eq!(snapshot.folders, 2);
    assert!(snapshot.started_at.is_some());
    assert!(snapshot.finished_at.is_some());
    // Cleared on the way out: there is no library being walked any more.
    assert!(snapshot.library.is_none());
}

#[tokio::test]
async fn a_cancelled_scan_says_so() {
    let root = temp_root("snapshot-cancelled");
    write_wav(&root.join("Album/one.wav"));

    let pool = database().await;
    library(&pool, &root).await;

    let progress = Progress::default();
    progress.cancel();
    scan_all(&pool, Mode::Incremental, &progress).await.unwrap();

    assert!(progress.snapshot().cancelled, "the panel can tell");
}

#[tokio::test]
async fn the_environment_adds_libraries_without_removing_others() {
    let pool = database().await;

    // One that somebody added through the API, which the environment knows
    // nothing about.
    let timestamp = db::now();
    sqlx::query(
        "INSERT INTO libraries (name, path, enabled, created_at, updated_at)
         VALUES ('from the panel', '/srv/added', 1, ?, ?)",
    )
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();

    sync_libraries(&pool, &[PathBuf::from("/srv/configured")])
        .await
        .unwrap();

    let enabled: Vec<String> =
        sqlx::query_scalar("SELECT path FROM libraries WHERE enabled = 1 ORDER BY path")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(
        enabled,
        vec!["/srv/added".to_string(), "/srv/configured".to_string()],
        "the variable adds and enables; it does not decide what else may exist"
    );
}

#[tokio::test]
async fn the_environment_re_enables_what_it_names() {
    let pool = database().await;

    let timestamp = db::now();
    sqlx::query(
        "INSERT INTO libraries (name, path, enabled, created_at, updated_at)
         VALUES ('turned off', '/srv/music', 0, ?, ?)",
    )
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();

    sync_libraries(&pool, &[PathBuf::from("/srv/music")])
        .await
        .unwrap();

    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM libraries WHERE path = '/srv/music'")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert!(enabled, "naming it in the variable turns it back on");
}

/// Moving a library is one row, and nothing else has to be touched.
///
/// This is what relative paths are for. Stored absolute, every track would name
/// the old directory and only a rescan could reconcile them one file at a time;
/// stored relative, the root is named once and changing it is the whole move.
#[tokio::test]
async fn moving_a_library_needs_no_rescan() {
    let root = std::env::temp_dir().join("tocata-scan-relocated");
    let elsewhere = std::env::temp_dir().join("tocata-scan-relocated-new");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&elsewhere);

    write_wav(&root.join("Artist/Album/01 song.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let stored: String = sqlx::query_scalar("SELECT path FROM tracks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, "Artist/Album/01 song.wav");

    // The whole move: the directory on disk, and one column.
    fs::rename(&root, &elsewhere).unwrap();
    sqlx::query("UPDATE libraries SET path = ? WHERE id = ?")
        .bind(elsewhere.to_string_lossy().as_ref())
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    // No scan in between: the file is findable at once, because where it is has
    // always been "inside the library" and only the library moved.
    let composed: String = sqlx::query_scalar(
        "SELECT l.path || '/' || t.path FROM tracks t JOIN libraries l ON l.id = t.library_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        composed,
        elsewhere.join("Artist/Album/01 song.wav").to_string_lossy()
    );
    assert!(
        std::path::Path::new(&composed).exists(),
        "the composed path has to be the file that is actually there"
    );

    fs::remove_dir_all(&elsewhere).unwrap();
}

/// Reading everything again has to forget that a cover was looked for and not
/// found, or a cover added to a directory afterwards would never be seen: the
/// server remembers the answer and stops opening the files.
///
/// And the quick scan has to leave that memory alone, since the whole point of it
/// is to skip work.
#[tokio::test]
async fn reading_everything_again_forgets_the_covers_that_were_not_found() {
    let pool = database().await;

    for (id, found) in [(1, 0), (2, 1), (3, 0)] {
        sqlx::query(
            "INSERT INTO artwork_lookups (entity_type, entity_id, source, attempted_at, found)
             VALUES ('album', ?, 'local', ?, ?)",
        )
        .bind(id)
        .bind(db::now())
        .bind(found)
        .execute(&pool)
        .await
        .unwrap();
    }

    // No library, so neither scan has a file to open. What is being tested
    // happens before the walk.
    scan_all(&pool, Mode::Incremental, &Progress::default())
        .await
        .unwrap();

    assert_eq!(lookups(&pool).await, 3, "a quick scan remembers everything");

    scan_all(&pool, Mode::Full, &Progress::default())
        .await
        .unwrap();

    assert_eq!(lookups(&pool).await, 1, "only the one that found a cover");
}

async fn lookups(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM artwork_lookups")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A file that will not open, and the two ways that used to go wrong.
///
/// Found on a real server: the tags of some files were edited, the scan that
/// followed could not read them — their permissions had gone — and afterwards they
/// sat in the listing with nothing on them. Giving the permissions back and scanning
/// again changed nothing, because from outside the files had not changed.
mod a_file_that_will_not_open {
    use super::*;

    /// Everything about the track, as a test wants to read it.
    async fn told(pool: &SqlitePool) -> (String, Option<i64>, Option<i64>, Option<String>) {
        sqlx::query_as("SELECT title, album_id, duration_ms, unreadable_since FROM tracks LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Takes away the permission to read a file, and gives it back.
    fn readable(path: &Path, may: bool) {
        use std::os::unix::fs::PermissionsExt;

        let mode = if may { 0o644 } else { 0o000 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The damage that was being done: a track scanned correctly, then scanned again
    /// while unreadable, lost its title to its file name and everything else to null.
    /// A scan was destroying the answer it could no longer see.
    #[tokio::test]
    async fn it_keeps_what_was_already_known_about_it() {
        let root = temp_root("unreadable-keeps");
        let path = root.join("Album/one.wav");
        write_wav(&path);
        tag(&path, &[("title", "Trains"), ("album", "In Absentia")]);

        let pool = database().await;
        let id = library(&pool, &root).await;
        scan(&pool, id, &root, Mode::Incremental).await.unwrap();

        let (title, album, length, _) = told(&pool).await;
        assert_eq!(title, "Trains");
        assert!(album.is_some() && length.is_some());

        readable(&path, false);
        let second = scan(&pool, id, &root, Mode::Full).await.unwrap();
        readable(&path, true);

        assert_eq!(second.failed, 1, "it could not be read");

        let (title, album_again, length_again, since) = told(&pool).await;
        assert_eq!(title, "Trains", "not the file name");
        assert_eq!(album_again, album, "still on its record");
        assert_eq!(length_again, length);
        assert!(since.is_some(), "and it says it could not be read");
    }

    /// The other half, and the one that made it permanent: size and modification
    /// time are how the file looks from outside, and from outside nothing changed
    /// when the permissions came back. A quick scan skipped it for ever.
    #[tokio::test]
    async fn it_is_read_again_by_the_next_quick_scan() {
        let root = temp_root("unreadable-retried");
        let path = root.join("Album/one.wav");
        write_wav(&path);

        let pool = database().await;
        let id = library(&pool, &root).await;

        readable(&path, false);
        scan(&pool, id, &root, Mode::Incremental).await.unwrap();
        readable(&path, true);

        let (_, _, _, since) = told(&pool).await;
        assert!(since.is_some(), "the first scan could not read it");

        // Nothing about the file has changed. Only the note on it says otherwise.
        let again = scan(&pool, id, &root, Mode::Incremental).await.unwrap();

        assert_eq!(again.unchanged, 0, "it was reopened rather than skipped");
        assert_eq!(again.failed, 0, "and this time it could be read");

        let (_, _, length, since) = told(&pool).await;
        assert!(length.is_some(), "so its length is known now");
        assert_eq!(since, None, "and the note is gone");
    }

    /// And the optimisation it must not cost: a file that was read fine and has not
    /// changed is still not opened again. Losing this would mean rereading a whole
    /// library on every quick scan.
    #[tokio::test]
    async fn a_file_that_was_read_fine_is_still_skipped() {
        let root = temp_root("unreadable-not-everything");
        let path = root.join("Album/one.wav");
        write_wav(&path);

        let pool = database().await;
        let id = library(&pool, &root).await;
        scan(&pool, id, &root, Mode::Incremental).await.unwrap();

        let again = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
        assert_eq!(again.unchanged, 1);
    }
}
