// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Removing for good what a scan only marked.
//!
//! A scan never deletes. It sets `missing_since` on what it did not find, because
//! the usual reason a file is not there is a disk that failed to mount, and
//! deleting the row would take somebody's favourites and playlist entries with
//! it. That leaves the other case unserved: a record went into the collection,
//! was not liked, and was thrown out. This is where that gets finished.
//!
//! Two ways in. Somebody asks for it from Maintenance, and everything marked
//! goes; or the collection has been given a quarantine, and the scan that finds
//! a file still gone after that long is the one that clears it out. The
//! difference between the two is one timestamp, which is why it is a parameter
//! rather than a second module.

use crate::types::Removed;
use anyhow::{Context, Result};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::path::Path;
use tracing::{info, warn};

/// Clears out what is absent, and then the cached files nothing points at any
/// more.
///
/// `until` is the newest `missing_since` that still goes, so a quarantine is
/// expressed by passing the moment it started; `None` takes everything a scan has
/// ever marked, which is what somebody asking for a purge means.
pub async fn absent(pool: &SqlitePool, data_dir: &Path, until: Option<&str>) -> Result<Removed> {
    let (removed, orphaned_files) = sweep(pool, until).await.context("purging what is absent")?;

    // After the commit, and never as part of it: a file that will not unlink must
    // not undo a purge that is already true in the database.
    for hash in orphaned_files {
        let path = crate::artwork::cache_path(data_dir, &hash);
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("could not remove cached artwork {}: {e}", path.display());
        }
    }

    info!(
        "purged {} tracks, {} folders, {} albums, {} artists, {} artworks",
        removed.tracks, removed.folders, removed.albums, removed.artists, removed.artworks
    );

    Ok(removed)
}

/// Everything in one transaction, so a failure half way leaves the collection as
/// it was. Returns what went, and the hashes whose cached files nothing refers to
/// any more.
///
/// The two statements that read `missing_since` compare it against `until` as
/// well, and a null there means every marked row: SQLite has no bound that means
/// "no bound", so the comparison is written to be skipped instead.
async fn sweep(
    pool: &SqlitePool,
    until: Option<&str>,
) -> Result<(Removed, Vec<String>), sqlx::Error> {
    let mut tx = crate::db::writing(pool).await?;

    // The full text tables are maintained by the scanner as it writes, not by
    // triggers, so nothing removes their rows unless it is done here. Left
    // behind, they would have search returning identifiers of things that no
    // longer exist.
    sqlx::query(
        "DELETE FROM tracks_fts
          WHERE rowid IN (SELECT id FROM tracks
                           WHERE missing_since IS NOT NULL
                             AND (?1 IS NULL OR missing_since <= ?1))",
    )
    .bind(until)
    .execute(&mut *tx)
    .await?;

    let tracks = sqlx::query(
        "DELETE FROM tracks
          WHERE missing_since IS NOT NULL AND (?1 IS NULL OR missing_since <= ?1)",
    )
    .bind(until)
    .execute(&mut *tx)
    .await?
    .rows_affected() as i64;

    // Folders after tracks, and only the ones left empty. A folder cascades into
    // its tracks, so deleting one that still holds a track would take a file that
    // is on disk. The walk emits a directory before the files in it, so a folder
    // marked absent should never hold a present track — but this is an
    // irreversible operation and the cost of that reasoning being wrong is
    // somebody's music.
    //
    // Looped because a folder is only empty once its children are gone, and the
    // tree can be any depth. It ends: every pass deletes at least one row or
    // stops.
    let mut folders = 0;
    loop {
        let gone = sqlx::query(
            "DELETE FROM folders
              WHERE missing_since IS NOT NULL AND (?1 IS NULL OR missing_since <= ?1)
                AND NOT EXISTS (SELECT 1 FROM tracks t WHERE t.folder_id = folders.id)
                AND NOT EXISTS (SELECT 1 FROM folders c WHERE c.parent_id = folders.id)",
        )
        .bind(until)
        .execute(&mut *tx)
        .await?
        .rows_affected() as i64;

        folders += gone;
        if gone == 0 {
            break;
        }
    }

    sqlx::query(
        "DELETE FROM albums_fts
          WHERE rowid IN (SELECT id FROM albums a
                           WHERE NOT EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = a.id))",
    )
    .execute(&mut *tx)
    .await?;

    let albums = sqlx::query(
        "DELETE FROM albums
          WHERE NOT EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = albums.id)",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected() as i64;

    // Artists last of the three, since losing an album can be what leaves one
    // with nothing.
    sqlx::query(
        "DELETE FROM artists_fts
          WHERE rowid IN (
              SELECT id FROM artists a
               WHERE NOT EXISTS (SELECT 1 FROM track_artists ta WHERE ta.artist_id = a.id)
                 AND NOT EXISTS (SELECT 1 FROM album_artists aa WHERE aa.artist_id = a.id))",
    )
    .execute(&mut *tx)
    .await?;

    let artists = sqlx::query(
        "DELETE FROM artists
          WHERE NOT EXISTS (SELECT 1 FROM track_artists ta WHERE ta.artist_id = artists.id)
            AND NOT EXISTS (SELECT 1 FROM album_artists aa WHERE aa.artist_id = artists.id)",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected() as i64;

    let genres = sqlx::query(
        "DELETE FROM genres
          WHERE NOT EXISTS (SELECT 1 FROM track_genres tg WHERE tg.genre_id = genres.id)
            AND NOT EXISTS (SELECT 1 FROM album_genres ag WHERE ag.genre_id = genres.id)",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected() as i64;

    let moods = sqlx::query(
        "DELETE FROM moods
          WHERE NOT EXISTS (SELECT 1 FROM track_moods tm WHERE tm.mood_id = moods.id)",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected() as i64;

    let (artworks, orphaned_files) = purge_artworks(&mut tx).await?;

    tx.commit().await?;

    Ok((
        Removed {
            tracks,
            folders,
            albums,
            artists,
            genres,
            moods,
            artworks,
        },
        orphaned_files,
    ))
}

/// Removes cover art nothing points at, and reports which cached files are now
/// unreferenced.
///
/// Two rows can share a hash — the same image found beside two albums — so a file
/// is only unreferenced once no row is left holding that hash.
async fn purge_artworks(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<(i64, Vec<String>), sqlx::Error> {
    const UNREFERENCED: &str = "SELECT id, content_hash FROM artworks a
          WHERE NOT EXISTS (SELECT 1 FROM albums b WHERE b.artwork_id = a.id)
            AND NOT EXISTS (SELECT 1 FROM artists r WHERE r.artwork_id = a.id)";

    let doomed: Vec<(i64, String)> = sqlx::query_as(UNREFERENCED).fetch_all(&mut **tx).await?;

    if doomed.is_empty() {
        return Ok((0, Vec::new()));
    }

    let removed = sqlx::query(
        "DELETE FROM artworks
          WHERE NOT EXISTS (SELECT 1 FROM albums b WHERE b.artwork_id = artworks.id)
            AND NOT EXISTS (SELECT 1 FROM artists r WHERE r.artwork_id = artworks.id)",
    )
    .execute(&mut **tx)
    .await?
    .rows_affected() as i64;

    let mut orphaned = Vec::new();
    for (_, hash) in doomed {
        let still_used: i64 =
            sqlx::query_scalar("SELECT count(*) FROM artworks WHERE content_hash = ?")
                .bind(&hash)
                .fetch_one(&mut **tx)
                .await?;

        if still_used == 0 && !orphaned.contains(&hash) {
            orphaned.push(hash);
        }
    }

    Ok((removed, orphaned))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// A collection with one album by one artist, in one genre, with a cover: one
    /// track of it stays and one goes.
    async fn collection() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        sqlx::query("INSERT INTO libraries (id, name, path, created_at, updated_at) VALUES (1, 'l', '/l', ?, ?)")
            .bind(&at).bind(&at).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan) VALUES (1, 'f1', 1, 'root', '/l', 1)")
            .execute(&pool).await.unwrap();
        // Marked absent, which is what a scan does to a directory that is no
        // longer there. Only folders it marked are ever removed: an empty
        // directory that still exists on disk is a real directory.
        sqlx::query("INSERT INTO folders (id, public_id, library_id, parent_id, name, path, missing_since, last_seen_scan) VALUES (2, 'f2', 1, 1, 'gone', '/l/gone', ?, 1)")
            .bind(&at).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO artworks (id, public_id, kind, source, mime_type, content_hash, fetched_at) VALUES (1, 'w1', 'album', 'file', 'image/jpeg', 'abcd', ?)")
            .bind(&at).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO artists (id, public_id, name, created_at, updated_at) VALUES (1, 'a1', 'Solitaria', ?, ?)")
            .bind(&at).bind(&at).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO albums (id, public_id, name, artwork_id, created_at, updated_at) VALUES (1, 'b1', 'Único', 1, ?, ?)")
            .bind(&at).bind(&at).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO album_artists (album_id, artist_id, role) VALUES (1, 1, 'albumartist')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO genres (id, name) VALUES (1, 'Flamenco')")
            .execute(&pool)
            .await
            .unwrap();

        // One track on disk, one gone. Both belong to the album, so the album only
        // becomes an orphan once the surviving one is removed too.
        for (id, folder, missing) in [(1i64, 1i64, None), (2, 2, Some(at.clone()))] {
            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path,
                                     file_size, file_modified_at, content_type, suffix, title,
                                     missing_since, last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, 1, ?, 1, ?, 1, ?, 'audio/wav', 'wav', 'Canción', ?, 1, ?, ?)",
            )
            .bind(id)
            .bind(format!("t{id}"))
            .bind(folder)
            .bind(format!("/l/{id}.wav"))
            .bind(&at)
            .bind(missing)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO track_artists (track_id, artist_id, role) VALUES (?, 1, 'artist')",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO track_genres (track_id, genre_id) VALUES (?, 1)")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO tracks_fts (rowid, title, album, artists) VALUES (?, 'Canción', 'Único', 'Solitaria')")
                .bind(id).execute(&pool).await.unwrap();
        }

        sqlx::query(
            "INSERT INTO albums_fts (rowid, name, artists) VALUES (1, 'Único', 'Solitaria')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO artists_fts (rowid, name) VALUES (1, 'Solitaria')")
            .execute(&pool)
            .await
            .unwrap();

        pool
    }

    async fn count(pool: &SqlitePool, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn only_what_is_absent_goes() {
        let pool = collection().await;

        let (removed, files) = sweep(&pool, None).await.unwrap();

        assert_eq!(removed.tracks, 1);
        assert_eq!(removed.folders, 1, "the folder it was the only thing in");
        // The album still has the other track, so nothing above the track is an
        // orphan yet.
        assert_eq!(removed.albums, 0);
        assert_eq!(removed.artists, 0);
        assert_eq!(removed.genres, 0);
        assert_eq!(removed.artworks, 0);
        assert!(files.is_empty(), "the cover is still in use");

        assert_eq!(count(&pool, "SELECT count(*) FROM tracks").await, 1);
        assert_eq!(count(&pool, "SELECT count(*) FROM tracks_fts").await, 1);
        assert_eq!(count(&pool, "SELECT count(*) FROM folders").await, 1);
    }

    #[tokio::test]
    async fn what_is_left_with_nothing_in_it_goes_too() {
        let pool = collection().await;

        // Now the whole album is absent.
        sqlx::query("UPDATE tracks SET missing_since = ?")
            .bind(db::now())
            .execute(&pool)
            .await
            .unwrap();

        let (removed, files) = sweep(&pool, None).await.unwrap();

        assert_eq!(removed.tracks, 2);
        assert_eq!(removed.albums, 1, "no tracks left in it");
        assert_eq!(removed.artists, 1, "neither tracks nor albums");
        assert_eq!(removed.genres, 1, "nobody in it");
        assert_eq!(removed.artworks, 1, "no album or artist points at it");
        assert_eq!(
            files,
            vec!["abcd".to_string()],
            "its cached file is unreferenced"
        );

        // The full text tables are maintained by hand, so this is the assertion
        // that stops search returning identifiers of things that are gone.
        assert_eq!(count(&pool, "SELECT count(*) FROM tracks_fts").await, 0);
        assert_eq!(count(&pool, "SELECT count(*) FROM albums_fts").await, 0);
        assert_eq!(count(&pool, "SELECT count(*) FROM artists_fts").await, 0);
    }

    /// The quarantine: what went missing this morning stays while what went
    /// missing last month goes. Both are marked; only the date tells them apart.
    #[tokio::test]
    async fn a_threshold_spares_what_has_not_been_gone_long_enough() {
        let pool = collection().await;

        let long_ago = db::from_now(-chrono::Duration::days(30));
        sqlx::query("UPDATE tracks SET missing_since = ? WHERE id = 1")
            .bind(&long_ago)
            .execute(&pool)
            .await
            .unwrap();

        let a_week_ago = db::from_now(-chrono::Duration::days(7));
        let (removed, _) = sweep(&pool, Some(&a_week_ago)).await.unwrap();

        assert_eq!(removed.tracks, 1, "only the one that went a month ago");
        assert_eq!(
            count(&pool, "SELECT count(*) FROM tracks WHERE id = 2").await,
            1,
            "marked, but not for long enough"
        );
        assert_eq!(
            removed.folders, 0,
            "its folder still holds the track that is only recently gone"
        );
    }

    /// Zero days of quarantine is "the scan that finds it gone removes it", and
    /// it has to reach something marked seconds ago.
    #[tokio::test]
    async fn a_threshold_of_now_still_takes_what_was_just_marked() {
        let pool = collection().await;

        let (removed, _) = sweep(&pool, Some(&db::now())).await.unwrap();

        assert_eq!(removed.tracks, 1);
    }

    #[tokio::test]
    async fn a_shared_cover_file_survives_one_of_its_rows() {
        let pool = collection().await;

        // A second album with the same image beside it, which is what two rows
        // sharing a content hash means.
        let at = db::now();
        sqlx::query("INSERT INTO artworks (id, public_id, kind, source, mime_type, content_hash, fetched_at) VALUES (2, 'w2', 'album', 'file', 'image/jpeg', 'abcd', ?)")
            .bind(&at).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO albums (id, public_id, name, artwork_id, created_at, updated_at) VALUES (2, 'b2', 'Otro', 2, ?, ?)")
            .bind(&at).bind(&at).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO tracks (id, public_id, library_id, folder_id, album_id, path, file_size, file_modified_at, content_type, suffix, title, last_seen_scan, created_at, updated_at) VALUES (3, 't3', 1, 1, 2, '/l/3.wav', 1, ?, 'audio/wav', 'wav', 'Otra', 1, ?, ?)")
            .bind(&at).bind(&at).bind(&at).execute(&pool).await.unwrap();

        sqlx::query("UPDATE tracks SET missing_since = ? WHERE id IN (1, 2)")
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

        let (removed, files) = sweep(&pool, None).await.unwrap();

        assert_eq!(removed.artworks, 1, "only the row nothing points at");
        assert!(
            files.is_empty(),
            "the file stays: the other row still names that hash"
        );
    }
}
