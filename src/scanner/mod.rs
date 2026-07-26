// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Turning what is on disk into rows.

mod album_key;
mod tags;
mod walker;

use crate::db;
use album_key::AlbumKey;
use anyhow::{Context, Result};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tags::Metadata;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use walker::Entry;

/// How many scanned entries wait in the channel. Reading tags is slower than
/// inserting rows, so this only has to be deep enough that the writer never
/// idles between files.
const CHANNEL_DEPTH: usize = 256;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub folders: u64,
    pub tracks: u64,
    pub failed: u64,
}

/// One entry, already read from disk. Metadata is absent when the file could
/// not be parsed; the track is still worth recording.
enum Scanned {
    Folder {
        path: PathBuf,
        modified: Option<i64>,
    },
    Track {
        path: PathBuf,
        extension: String,
        size: u64,
        modified: i64,
        metadata: Option<Box<Metadata>>,
    },
}

/// Scans every enabled library.
pub async fn scan_all(pool: &SqlitePool) -> Result<Outcome> {
    let libraries: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, path FROM libraries WHERE enabled = 1 ORDER BY id")
            .fetch_all(pool)
            .await
            .context("listing libraries")?;

    let mut total = Outcome::default();
    for (id, path) in libraries {
        let outcome = scan_library(pool, id, Path::new(&path)).await?;
        info!(
            "scanned '{}': {} folders, {} tracks, {} failed",
            path, outcome.folders, outcome.tracks, outcome.failed
        );
        total.folders += outcome.folders;
        total.tracks += outcome.tracks;
        total.failed += outcome.failed;
    }

    Ok(total)
}

/// Walking and tag reading happen on a blocking thread and arrive through a
/// channel; writing happens here. Neither side waits for the other: reading a
/// file is I/O bound and inserting a row is not.
async fn scan_library(pool: &SqlitePool, library_id: i64, root: &Path) -> Result<Outcome> {
    let (tx, mut rx) = mpsc::channel(CHANNEL_DEPTH);
    let root_owned = root.to_path_buf();

    let reader = tokio::task::spawn_blocking(move || {
        for entry in walker::walk(&root_owned) {
            let scanned = match entry {
                Entry::Directory { path, modified } => Scanned::Folder { path, modified },
                Entry::Audio {
                    path,
                    extension,
                    size,
                    modified,
                } => {
                    let metadata = match tags::read(&path) {
                        Ok(metadata) => Some(Box::new(metadata)),
                        Err(e) => {
                            warn!("could not read tags from {}: {e:#}", path.display());
                            None
                        }
                    };
                    Scanned::Track {
                        path,
                        extension,
                        size,
                        modified,
                        metadata,
                    }
                }
            };

            // A closed receiver means the writer gave up; stop reading files.
            if tx.blocking_send(scanned).is_err() {
                break;
            }
        }
    });

    let mut state = State::new(library_id);
    let mut outcome = Outcome::default();
    let mut tx_db = pool
        .begin()
        .await
        .context("starting the scan transaction")?;

    while let Some(scanned) = rx.recv().await {
        match scanned {
            Scanned::Folder { path, modified } => {
                state.insert_folder(&mut tx_db, &path, modified).await?;
                outcome.folders += 1;
            }
            Scanned::Track {
                path,
                extension,
                size,
                modified,
                metadata,
            } => {
                if metadata.is_none() {
                    outcome.failed += 1;
                }
                state
                    .insert_track(&mut tx_db, &path, &extension, size, modified, metadata)
                    .await?;
                outcome.tracks += 1;
            }
        }
    }

    tx_db.commit().await.context("committing the scan")?;
    reader.await.context("the file reader panicked")?;

    Ok(outcome)
}

/// Everything the writer has to remember while a scan runs.
///
/// These maps are what keep the scan to one pass: without them, every track
/// would need a SELECT per artist, album and genre to find out whether it
/// exists yet.
struct State {
    library_id: i64,
    folders: HashMap<PathBuf, i64>,
    artists: HashMap<String, i64>,
    albums: HashMap<AlbumKey, i64>,
    genres: HashMap<String, i64>,
    timestamp: String,
}

impl State {
    fn new(library_id: i64) -> Self {
        Self {
            library_id,
            folders: HashMap::new(),
            artists: HashMap::new(),
            albums: HashMap::new(),
            genres: HashMap::new(),
            timestamp: db::now(),
        }
    }

    async fn insert_folder(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        path: &Path,
        modified: Option<i64>,
    ) -> Result<()> {
        // The walk yields parents first, so this lookup finds the parent for
        // everything except a library root's immediate children.
        let parent_id = path.parent().and_then(|p| self.folders.get(p)).copied();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO folders (public_id, library_id, parent_id, name, path, modified_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, path) DO UPDATE SET
                 parent_id = excluded.parent_id,
                 name = excluded.name,
                 modified_at = excluded.modified_at
             RETURNING id",
        )
        .bind(db::public_id()?)
        .bind(self.library_id)
        .bind(parent_id)
        .bind(&name)
        .bind(path.to_string_lossy().as_ref())
        .bind(modified.map(epoch_to_iso8601))
        .fetch_one(&mut **tx)
        .await
        .with_context(|| format!("inserting folder {}", path.display()))?;

        self.folders.insert(path.to_path_buf(), id);
        Ok(())
    }

    async fn insert_track(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        path: &Path,
        extension: &str,
        size: u64,
        modified: i64,
        metadata: Option<Box<Metadata>>,
    ) -> Result<()> {
        let Some(folder_id) = path.parent().and_then(|p| self.folders.get(p)).copied() else {
            // Only reachable if the walk handed us a file before its directory,
            // which it does not do.
            debug!("skipping {}: its folder is unknown", path.display());
            return Ok(());
        };

        let metadata = metadata.map(|m| *m).unwrap_or_default();

        let album_id = match AlbumKey::of(&metadata) {
            Some(key) => Some(self.album_id(tx, key, &metadata).await?),
            None => None,
        };

        // A file with no readable title is still a track: the file name is a
        // better answer than nothing, and it is what the user sees.
        let title = metadata.title.clone().unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string())
        });

        let track_id: i64 = sqlx::query_scalar(
            "INSERT INTO tracks (
                 public_id, library_id, folder_id, album_id, path, file_size,
                 file_modified_at, content_type, suffix, title, sort_title,
                 track_number, disc_number, year, duration_ms, bit_rate,
                 bit_depth, sampling_rate, channel_count, bpm, comment,
                 mbid_recording, mbid_track, isrc, rg_track_gain, rg_track_peak,
                 created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                       ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, path) DO UPDATE SET
                 folder_id = excluded.folder_id,
                 album_id = excluded.album_id,
                 file_size = excluded.file_size,
                 file_modified_at = excluded.file_modified_at,
                 content_type = excluded.content_type,
                 suffix = excluded.suffix,
                 title = excluded.title,
                 sort_title = excluded.sort_title,
                 track_number = excluded.track_number,
                 disc_number = excluded.disc_number,
                 year = excluded.year,
                 duration_ms = excluded.duration_ms,
                 bit_rate = excluded.bit_rate,
                 bit_depth = excluded.bit_depth,
                 sampling_rate = excluded.sampling_rate,
                 channel_count = excluded.channel_count,
                 bpm = excluded.bpm,
                 comment = excluded.comment,
                 mbid_recording = excluded.mbid_recording,
                 mbid_track = excluded.mbid_track,
                 isrc = excluded.isrc,
                 rg_track_gain = excluded.rg_track_gain,
                 rg_track_peak = excluded.rg_track_peak,
                 missing_since = NULL,
                 updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(db::public_id()?)
        .bind(self.library_id)
        .bind(folder_id)
        .bind(album_id)
        .bind(path.to_string_lossy().as_ref())
        .bind(size as i64)
        .bind(epoch_to_iso8601(modified))
        .bind(content_type(extension))
        .bind(extension)
        .bind(&title)
        .bind(&metadata.sort_title)
        .bind(metadata.track_number)
        .bind(metadata.disc_number)
        .bind(metadata.year)
        .bind(metadata.duration_ms)
        .bind(metadata.bit_rate)
        .bind(metadata.bit_depth)
        .bind(metadata.sampling_rate)
        .bind(metadata.channel_count)
        .bind(metadata.bpm)
        .bind(&metadata.comment)
        .bind(&metadata.mbid_recording)
        .bind(&metadata.mbid_track)
        .bind(&metadata.isrc)
        .bind(metadata.rg_track_gain)
        .bind(metadata.rg_track_peak)
        .bind(&self.timestamp)
        .bind(&self.timestamp)
        .fetch_one(&mut **tx)
        .await
        .with_context(|| format!("inserting track {}", path.display()))?;

        self.link_track(tx, track_id, &metadata).await?;

        if let Some(album_id) = album_id {
            self.link_album(tx, album_id, &metadata).await?;
        }

        Ok(())
    }

    /// Replaces the track's credits and genres wholesale. Cheaper than working
    /// out what changed, and correct when an artist is dropped from a tag.
    async fn link_track(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        track_id: i64,
        metadata: &Metadata,
    ) -> Result<()> {
        sqlx::query("DELETE FROM track_artists WHERE track_id = ?")
            .bind(track_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM track_genres WHERE track_id = ?")
            .bind(track_id)
            .execute(&mut **tx)
            .await?;

        for (position, name) in metadata.artists.iter().enumerate() {
            let artist_id = self.artist_id(tx, name).await?;
            sqlx::query(
                "INSERT INTO track_artists (track_id, artist_id, role, position)
                 VALUES (?, ?, 'artist', ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(track_id)
            .bind(artist_id)
            .bind(position as i64)
            .execute(&mut **tx)
            .await?;
        }

        for name in &metadata.genres {
            let genre_id = self.genre_id(tx, name).await?;
            sqlx::query(
                "INSERT INTO track_genres (track_id, genre_id) VALUES (?, ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(track_id)
            .bind(genre_id)
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }

    async fn link_album(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        album_id: i64,
        metadata: &Metadata,
    ) -> Result<()> {
        for (position, name) in metadata.album_artists.iter().enumerate() {
            let artist_id = self.artist_id(tx, name).await?;
            sqlx::query(
                "INSERT INTO album_artists (album_id, artist_id, role, position)
                 VALUES (?, ?, 'albumartist', ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(album_id)
            .bind(artist_id)
            .bind(position as i64)
            .execute(&mut **tx)
            .await?;
        }

        for name in &metadata.genres {
            let genre_id = self.genre_id(tx, name).await?;
            sqlx::query(
                "INSERT INTO album_genres (album_id, genre_id) VALUES (?, ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(album_id)
            .bind(genre_id)
            .execute(&mut **tx)
            .await?;
        }

        if let (Some(disc), Some(subtitle)) = (metadata.disc_number, &metadata.disc_subtitle) {
            sqlx::query(
                "INSERT INTO album_discs (album_id, disc_number, title) VALUES (?, ?, ?)
                 ON CONFLICT (album_id, disc_number) DO UPDATE SET title = excluded.title",
            )
            .bind(album_id)
            .bind(disc)
            .bind(subtitle)
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }

    async fn artist_id(&mut self, tx: &mut Transaction<'_, Sqlite>, name: &str) -> Result<i64> {
        let key = name.trim().to_lowercase();
        if let Some(id) = self.artists.get(&key) {
            return Ok(*id);
        }

        // The name is not unique in the schema on purpose: two artists can share
        // one. Within a scan the cache is what keeps them from multiplying, and
        // across scans this lookup finds the existing row.
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM artists WHERE lower(name) = ? LIMIT 1")
                .bind(&key)
                .fetch_optional(&mut **tx)
                .await
                .with_context(|| format!("looking up artist {name}"))?;

        let id = match existing {
            Some(id) => id,
            None => sqlx::query_scalar(
                "INSERT INTO artists (public_id, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?) RETURNING id",
            )
            .bind(db::public_id()?)
            .bind(name.trim())
            .bind(&self.timestamp)
            .bind(&self.timestamp)
            .fetch_one(&mut **tx)
            .await
            .with_context(|| format!("inserting artist {name}"))?,
        };

        self.artists.insert(key, id);
        Ok(id)
    }

    async fn genre_id(&mut self, tx: &mut Transaction<'_, Sqlite>, name: &str) -> Result<i64> {
        let key = name.trim().to_lowercase();
        if let Some(id) = self.genres.get(&key) {
            return Ok(*id);
        }

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO genres (name) VALUES (?)
             ON CONFLICT (name) DO UPDATE SET name = excluded.name
             RETURNING id",
        )
        .bind(name.trim())
        .fetch_one(&mut **tx)
        .await
        .with_context(|| format!("inserting genre {name}"))?;

        self.genres.insert(key, id);
        Ok(id)
    }

    async fn album_id(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        key: AlbumKey,
        metadata: &Metadata,
    ) -> Result<i64> {
        if let Some(id) = self.albums.get(&key) {
            return Ok(*id);
        }

        let name = metadata.album.clone().unwrap_or_default();
        let is_compilation = i64::from(matches!(key, AlbumKey::Compilation { .. }));

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO albums (
                 public_id, name, sort_name, year, release_date, is_compilation,
                 mbid_release, mbid_release_group, rg_album_gain, rg_album_peak,
                 created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(db::public_id()?)
        .bind(&name)
        .bind(&metadata.sort_album)
        .bind(metadata.year)
        .bind(&metadata.date)
        .bind(is_compilation)
        .bind(&metadata.mbid_release)
        .bind(&metadata.mbid_release_group)
        .bind(metadata.rg_album_gain)
        .bind(metadata.rg_album_peak)
        .bind(&self.timestamp)
        .bind(&self.timestamp)
        .fetch_one(&mut **tx)
        .await
        .with_context(|| format!("inserting album {name}"))?;

        self.albums.insert(key, id);
        Ok(id)
    }
}

/// The suffix is what the API reports; the content type is what a client gets
/// in the response header when it streams the file.
fn content_type(extension: &str) -> String {
    let subtype = match extension {
        "mp3" => "mpeg",
        "m4a" | "m4b" | "aac" => "mp4",
        "oga" => "ogg",
        "aif" => "aiff",
        other => other,
    };

    format!("audio/{subtype}")
}

fn epoch_to_iso8601(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Brings the `libraries` table in line with what the environment declares.
///
/// Paths that disappear from the configuration are disabled rather than
/// deleted: the row still owns folders and tracks, which in turn own the
/// user's favourites and playlist entries. Removing a library from the
/// configuration is not a request to lose all of that.
pub async fn sync_libraries(pool: &SqlitePool, paths: &[PathBuf]) -> Result<()> {
    let timestamp = db::now();

    for path in paths {
        let path = path.to_string_lossy();
        let name = Path::new(path.as_ref())
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());

        sqlx::query(
            "INSERT INTO libraries (name, path, enabled, created_at, updated_at)
             VALUES (?, ?, 1, ?, ?)
             ON CONFLICT (path) DO UPDATE SET enabled = 1, updated_at = excluded.updated_at",
        )
        .bind(&name)
        .bind(path.as_ref())
        .bind(&timestamp)
        .bind(&timestamp)
        .execute(pool)
        .await
        .with_context(|| format!("registering library {path}"))?;
    }

    let configured: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let mut disable = sqlx::QueryBuilder::new("UPDATE libraries SET enabled = 0 WHERE enabled = 1");
    if !configured.is_empty() {
        disable.push(" AND path NOT IN (");
        let mut separated = disable.separated(", ");
        for path in &configured {
            separated.push_bind(path);
        }
        disable.push(")");
    }

    let disabled = disable
        .build()
        .execute(pool)
        .await
        .context("disabling libraries that are no longer configured")?
        .rows_affected();

    if disabled > 0 {
        warn!("{disabled} library(ies) are no longer configured and have been disabled");
    }

    Ok(())
}
