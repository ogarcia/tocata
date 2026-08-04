// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Turning what is on disk into rows.

mod album_key;
mod tags;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod overlap_tests {
    use super::overlaps;
    use std::path::Path;

    #[test]
    fn a_library_inside_another_overlaps_it() {
        assert!(overlaps(Path::new("/music"), Path::new("/music/rock")));
        assert!(overlaps(Path::new("/music/rock"), Path::new("/music")));
        assert!(overlaps(Path::new("/music"), Path::new("/music")));
    }

    /// Compared by components and not as text, which is the whole reason this is a
    /// function: `/musicians` starts with the letters of `/music` and is not
    /// inside it.
    #[test]
    fn a_name_that_merely_starts_the_same_does_not() {
        assert!(!overlaps(Path::new("/music"), Path::new("/musicians")));
        assert!(!overlaps(Path::new("/srv/music"), Path::new("/srv/music2")));
    }

    #[test]
    fn unrelated_directories_do_not() {
        assert!(!overlaps(Path::new("/srv/music"), Path::new("/mnt/vinyl")));
    }
}
mod walker;

use crate::db;
use album_key::AlbumKey;
use anyhow::{Context, Result};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tags::Metadata;
pub use tags::read as read_tags;
pub use tags::read_every as read_every_tag;
pub use tags::read_with_cover_art as read_tags_with_cover_art;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};
use walker::Entry;

/// An album already recorded in this scan: the year it was filed under, and its
/// row.
type RecordedAlbum = (Option<String>, i64);

/// Albums reachable by artist and name, which is how a track with no year finds
/// the album its siblings already created.
type AlbumsByName = HashMap<(String, String), Vec<RecordedAlbum>>;

/// How many scanned entries wait in the channel. Reading tags is slower than
/// inserting rows, so this only has to be deep enough that the writer never
/// idles between files.
const CHANNEL_DEPTH: usize = 256;

/// Fraction of a library that has to vanish before the sweep refuses to run.
///
/// A whole library disappearing is not a deletion, it is a disk that did not
/// mount. Nothing would be lost — tracks are marked, not removed — but the user
/// would open a client and find an empty server, which is a fright nobody
/// needs. Ninety per cent catches the failed mount without standing in the way
/// of somebody genuinely clearing out a library.
const VANISHED_FRACTION_LIMIT: f64 = 0.9;

/// Below this many tracks the fraction above means nothing, so the sweep runs
/// regardless.
const VANISHED_MINIMUM: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Skips files whose size and modification time are unchanged.
    Incremental,
    /// Reads every file again. For when tags were edited with their timestamps
    /// preserved, or when our own extraction improved.
    Full,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub folders: u64,
    pub tracks: u64,
    pub unchanged: u64,
    pub failed: u64,
    pub gone: u64,
}

/// One entry, already read from disk. Metadata is absent when the file could
/// not be parsed; the track is still worth recording.
enum Scanned {
    Folder {
        path: PathBuf,
        modified: Option<i64>,
    },
    /// Same size and modification time as last time, so its tags were not even
    /// opened. All it needs is to be counted as seen.
    Unchanged { path: PathBuf },
    Track {
        /// False when no row existed at this path, which is when a moved file
        /// is worth looking for.
        known: bool,
        path: PathBuf,
        extension: String,
        size: u64,
        modified: i64,
        metadata: Option<Box<Metadata>>,
    },
}

/// Scans every enabled library.
///
/// Returns `None` without doing anything when a scan is already in flight,
/// which is what a second `startScan` gets instead of a second scan.
pub async fn scan_all(
    pool: &SqlitePool,
    mode: Mode,
    progress: &Progress,
) -> Result<Option<Outcome>> {
    let Some(_running) = progress.begin() else {
        debug!("a scan is already running; ignoring the request for another");
        return Ok(None);
    };

    let libraries: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, path, name FROM libraries WHERE enabled = 1 ORDER BY id")
            .fetch_all(pool)
            .await
            .context("listing libraries")?;

    // Reading everything again means trusting nothing that was known before, and
    // one of the things known before is that an album had no cover anywhere. That
    // answer is remembered so the server does not open the same twenty files on
    // every request — which is right until somebody puts a cover.jpg beside the
    // music, and then it is the reason nothing changes. Only on a full scan: the
    // quick one exists to skip work, and this is work.
    if mode == Mode::Full {
        let forgotten = sqlx::query("DELETE FROM artwork_lookups WHERE found = 0")
            .execute(pool)
            .await
            .context("forgetting the covers that were not found")?
            .rows_affected();

        if forgotten > 0 {
            info!("will look again for {forgotten} covers that were not found before");
        }
    }

    let mut total = Outcome::default();
    for (id, path, name) in libraries {
        progress.entering(&name);

        let Some(outcome) = scan_library(pool, id, Path::new(&path), mode, progress).await? else {
            return Ok(None);
        };
        info!(
            "scanned '{}': {} folders, {} tracks ({} unchanged), {} failed, {} gone",
            path, outcome.folders, outcome.tracks, outcome.unchanged, outcome.failed, outcome.gone
        );
        total.folders += outcome.folders;
        total.tracks += outcome.tracks;
        total.unchanged += outcome.unchanged;
        total.failed += outcome.failed;
        total.gone += outcome.gone;
    }

    Ok(Some(total))
}

/// Walking and tag reading happen on a blocking thread and arrive through a
/// channel; writing happens here. Neither side waits for the other: reading a
/// file is I/O bound and inserting a row is not.
///
/// `None` when the scan was asked to stop before it finished.
async fn scan_library(
    pool: &SqlitePool,
    library_id: i64,
    root: &Path,
    mode: Mode,
    progress: &Progress,
) -> Result<Option<Outcome>> {
    // The run's own id is what marks the rows this scan touches.
    let scan: i64 = sqlx::query_scalar(
        "INSERT INTO scan_runs (library_id, started_at, full_scan)
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(library_id)
    .bind(db::now())
    .bind(i64::from(mode == Mode::Full))
    .fetch_one(pool)
    .await
    .context("recording the start of the scan")?;

    // What is already recorded, so the reader can skip files that have not
    // changed without asking the database once per file.
    //
    // Joined back onto the root on the way in: the rows are relative, and every
    // path the walker produces is absolute. Composing here rather than
    // relativising a million times in the loop below.
    let known: HashMap<PathBuf, (String, i64)> = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT path, file_modified_at, file_size FROM tracks WHERE library_id = ?",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await
    .context("loading the tracks already recorded")?
    .into_iter()
    .map(|(path, modified, size)| (root.join(path), (modified, size)))
    .collect();

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
                    let recorded = known.get(&path);

                    // Size and modification time together, not either alone: a
                    // tagger writing within the padding of an existing tag can
                    // leave the size untouched, and one asked to preserve
                    // timestamps leaves the other untouched.
                    let unchanged = mode == Mode::Incremental
                        && recorded.is_some_and(|(recorded_modified, recorded_size)| {
                            *recorded_size == size as i64
                                && recorded_modified == &epoch_to_iso8601(modified)
                        });

                    if unchanged {
                        Scanned::Unchanged { path }
                    } else {
                        let metadata = match tags::read(&path) {
                            Ok(metadata) => Some(Box::new(metadata)),
                            Err(e) => {
                                warn!("could not read tags from {}: {e:#}", path.display());
                                None
                            }
                        };
                        Scanned::Track {
                            known: recorded.is_some(),
                            path,
                            extension,
                            size,
                            modified,
                            metadata,
                        }
                    }
                }
            };

            // A closed receiver means the writer gave up; stop reading files.
            if tx.blocking_send(scanned).is_err() {
                break;
            }
        }
    });

    let mut state = State::new(library_id, root.to_path_buf(), scan);
    let mut outcome = Outcome::default();
    let mut tx_db = db::writing(pool)
        .await
        .context("starting the scan transaction")?;

    while let Some(scanned) = rx.recv().await {
        // Leaving without committing is the point. A scan stopped half way
        // through would sweep everything it had not reached yet into missing, so
        // the transaction is dropped instead: the database goes back to what it
        // was, and the next start scans again.
        if progress.should_stop() {
            info!("scan interrupted; what it had written is discarded");
            return Ok(None);
        }

        match scanned {
            Scanned::Folder { path, modified } => {
                state.insert_folder(&mut tx_db, &path, modified).await?;
                outcome.folders += 1;
                progress.observed(Item::Folder, &path);
            }
            Scanned::Unchanged { path } => {
                state.touch_track(&mut tx_db, &path).await?;
                outcome.tracks += 1;
                outcome.unchanged += 1;
                progress.observed(Item::Unchanged, &path);
            }
            Scanned::Track {
                known,
                path,
                extension,
                size,
                modified,
                metadata,
            } => {
                let readable = metadata.is_some();
                if !readable {
                    outcome.failed += 1;
                }
                if !known {
                    state
                        .reclaim_moved(&mut tx_db, &path, size, &metadata)
                        .await?;
                }
                state
                    .insert_track(&mut tx_db, &path, &extension, size, modified, metadata)
                    .await?;
                outcome.tracks += 1;
                progress.observed(Item::Track { readable }, &path);
            }
        }
    }

    outcome.gone = state.sweep(&mut tx_db).await?;
    progress.swept(outcome.gone);

    tx_db.commit().await.context("committing the scan")?;
    reader.await.context("the file reader panicked")?;

    sqlx::query(
        "UPDATE scan_runs SET finished_at = ?, tracks_seen = ?, tracks_added = ?
          WHERE id = ?",
    )
    .bind(db::now())
    .bind(outcome.tracks as i64)
    .bind((outcome.tracks - outcome.unchanged) as i64)
    .bind(scan)
    .execute(pool)
    .await
    .context("recording the end of the scan")?;

    Ok(Some(outcome))
}

/// Everything the writer has to remember while a scan runs.
///
/// These maps are what keep the scan to one pass: without them, every track
/// would need a SELECT per artist, album and genre to find out whether it
/// exists yet.
struct State {
    library_id: i64,
    /// The library's own directory. Every path written from here down is relative
    /// to it.
    root: PathBuf,
    scan: i64,
    folders: HashMap<PathBuf, i64>,
    artists: HashMap<String, i64>,
    albums: HashMap<AlbumKey, i64>,
    /// Albums indexed by artist and name, each with the year it was recorded
    /// under. What lets a track with no year join an album that has one.
    albums_by_name: AlbumsByName,
    genres: HashMap<String, i64>,
    timestamp: String,
}

impl State {
    fn new(library_id: i64, root: PathBuf, scan: i64) -> Self {
        Self {
            library_id,
            root,
            scan,
            folders: HashMap::new(),
            artists: HashMap::new(),
            albums: HashMap::new(),
            albums_by_name: AlbumsByName::new(),
            genres: HashMap::new(),
            timestamp: db::now(),
        }
    }

    /// A path as the database holds it: relative to the library's own root.
    ///
    /// Storing them absolute is what made moving a library expensive — every row
    /// would have named the old place, and only a rescan could reconcile them.
    /// Relative, the root is named once, in one row, and moving a library is
    /// changing that one row.
    ///
    /// Everything above this line works in absolute paths, because that is what
    /// reading a file needs. The conversion belongs here, at the edge where paths
    /// stop being places on a disk and become rows.
    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
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
            "INSERT INTO folders (
                 public_id, library_id, parent_id, name, path, modified_at,
                 last_seen_scan
             ) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, path) DO UPDATE SET
                 parent_id = excluded.parent_id,
                 name = excluded.name,
                 modified_at = excluded.modified_at,
                 missing_since = NULL,
                 last_seen_scan = excluded.last_seen_scan
             RETURNING id",
        )
        .bind(db::public_id()?)
        .bind(self.library_id)
        .bind(parent_id)
        .bind(&name)
        .bind(self.relative(path))
        .bind(modified.map(epoch_to_iso8601))
        .bind(self.scan)
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

        let title = resolve_title(path, &metadata);

        let track_id: i64 = sqlx::query_scalar(
            "INSERT INTO tracks (
                 public_id, library_id, folder_id, album_id, path, file_size,
                 file_modified_at, content_type, suffix, title, sort_title,
                 track_number, disc_number, year, duration_ms, bit_rate,
                 bit_depth, sampling_rate, channel_count, bpm, comment,
                 mbid_recording, mbid_track, isrc, rg_track_gain, rg_track_peak,
                 last_seen_scan, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                       ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                 last_seen_scan = excluded.last_seen_scan,
                 updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(db::public_id()?)
        .bind(self.library_id)
        .bind(folder_id)
        .bind(album_id)
        .bind(self.relative(path))
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
        .bind(self.scan)
        .bind(&self.timestamp)
        .bind(&self.timestamp)
        .fetch_one(&mut **tx)
        .await
        .with_context(|| format!("inserting track {}", path.display()))?;

        self.link_track(tx, track_id, &metadata).await?;
        self.index_track(tx, track_id, &title, &metadata).await?;

        if let Some(album_id) = album_id {
            self.link_album(tx, album_id, &metadata).await?;
        }

        Ok(())
    }

    /// Records that an unchanged file is still there. The whole point of the
    /// incremental scan is that this is all it costs.
    async fn touch_track(&mut self, tx: &mut Transaction<'_, Sqlite>, path: &Path) -> Result<()> {
        sqlx::query(
            "UPDATE tracks SET last_seen_scan = ?, missing_since = NULL
              WHERE library_id = ? AND path = ?",
        )
        .bind(self.scan)
        .bind(self.library_id)
        .bind(self.relative(path))
        .execute(&mut **tx)
        .await
        .with_context(|| format!("marking {} as seen", path.display()))?;

        Ok(())
    }

    /// Looks for a file that went missing and has turned up here under another
    /// path, and moves that row to the new one.
    ///
    /// This is what makes the opaque public identifiers hold up: the row keeps
    /// its id, so the favourites, ratings, play counts and playlist entries
    /// hanging off it survive a reorganisation of the library. The subsequent
    /// upsert then finds the row by path and refreshes the rest.
    ///
    /// Matching is on a MusicBrainz recording id when the tag has one, and
    /// otherwise on size, duration and title together. No hashing: reading every
    /// byte to be slightly more certain is not worth an hour of disk.
    ///
    /// The candidate is anything this scan has not seen yet, not just what is
    /// already marked as missing: the sweep runs at the end, so when a moved
    /// file turns up the old row is still unmarked. Rows already marked are
    /// preferred, since a row this scan simply has not reached yet might still
    /// be there.
    async fn reclaim_moved(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        path: &Path,
        size: u64,
        metadata: &Option<Box<Metadata>>,
    ) -> Result<()> {
        let Some(metadata) = metadata else {
            return Ok(());
        };

        // The title from the tag, and only from the tag. For a file with no
        // title tag the stored title is its file name, and a rename changes
        // that, so matching on it would defeat the whole point. When there is
        // no tag, size and duration carry the match on their own: two different
        // files agreeing on both, to the byte and the millisecond, are the same
        // audio in all but name.
        let tagged_title = metadata.title.as_deref();

        let candidate: Option<i64> = match metadata.mbid_recording.as_deref() {
            Some(mbid) => {
                sqlx::query_scalar(
                    "SELECT id FROM tracks
                      WHERE library_id = ? AND last_seen_scan < ?
                        AND mbid_recording = ?
                      ORDER BY missing_since IS NULL, id
                      LIMIT 1",
                )
                .bind(self.library_id)
                .bind(self.scan)
                .bind(mbid)
                .fetch_optional(&mut **tx)
                .await
            }
            None => {
                sqlx::query_scalar(
                    "SELECT id FROM tracks
                      WHERE library_id = ? AND last_seen_scan < ?
                        AND file_size = ? AND duration_ms IS ?
                        AND (? IS NULL OR title = ?)
                      ORDER BY missing_since IS NULL, id
                      LIMIT 1",
                )
                .bind(self.library_id)
                .bind(self.scan)
                .bind(size as i64)
                .bind(metadata.duration_ms)
                .bind(tagged_title)
                .bind(tagged_title)
                .fetch_optional(&mut **tx)
                .await
            }
        }
        .context("looking for a moved file")?;

        if let Some(id) = candidate {
            debug!("{} is a file that moved; keeping its row", path.display());
            sqlx::query("UPDATE tracks SET path = ? WHERE id = ?")
                .bind(self.relative(path))
                .bind(id)
                .execute(&mut **tx)
                .await
                .context("moving a reclaimed track to its new path")?;
        }

        Ok(())
    }

    /// Marks everything this scan did not touch, unless so much of the library
    /// vanished at once that a failed mount is the likelier explanation.
    async fn sweep(&mut self, tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE library_id = ?")
            .bind(self.library_id)
            .fetch_one(&mut **tx)
            .await
            .context("counting the library")?;

        let vanished: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM tracks
              WHERE library_id = ? AND last_seen_scan < ? AND missing_since IS NULL",
        )
        .bind(self.library_id)
        .bind(self.scan)
        .fetch_one(&mut **tx)
        .await
        .context("counting what the scan did not see")?;

        if vanished == 0 {
            return Ok(0);
        }

        if total >= VANISHED_MINIMUM
            && (vanished as f64) / (total as f64) >= VANISHED_FRACTION_LIMIT
        {
            warn!(
                "not marking anything: {vanished} of {total} tracks vanished at once, \
                 which looks like a filesystem that did not mount rather than a deletion"
            );
            return Ok(0);
        }

        // Written out rather than looped over a table name: sqlx refuses
        // dynamically built SQL, and it is right to.
        sqlx::query(
            "UPDATE tracks SET missing_since = ?
              WHERE library_id = ? AND last_seen_scan < ? AND missing_since IS NULL",
        )
        .bind(db::now())
        .bind(self.library_id)
        .bind(self.scan)
        .execute(&mut **tx)
        .await
        .context("marking absent tracks")?;

        sqlx::query(
            "UPDATE folders SET missing_since = ?
              WHERE library_id = ? AND last_seen_scan < ? AND missing_since IS NULL",
        )
        .bind(db::now())
        .bind(self.library_id)
        .bind(self.scan)
        .execute(&mut **tx)
        .await
        .context("marking absent folders")?;

        Ok(vanished as u64)
    }

    /// Writes the track into the full text index.
    ///
    /// Delete then insert, because an FTS5 table that keeps its own content has
    /// no upsert: a retagged track would otherwise be found under both its old
    /// and its new title.
    async fn index_track(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        track_id: i64,
        title: &str,
        metadata: &Metadata,
    ) -> Result<()> {
        sqlx::query("DELETE FROM tracks_fts WHERE rowid = ?")
            .bind(track_id)
            .execute(&mut **tx)
            .await
            .context("clearing the old index entry of a track")?;

        sqlx::query("INSERT INTO tracks_fts (rowid, title, album, artists) VALUES (?, ?, ?, ?)")
            .bind(track_id)
            .bind(title)
            .bind(metadata.album.as_deref().unwrap_or_default())
            .bind(metadata.artists.join(" "))
            .execute(&mut **tx)
            .await
            .context("indexing a track")?;

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

        sqlx::query("DELETE FROM artists_fts WHERE rowid = ?")
            .bind(id)
            .execute(&mut **tx)
            .await
            .context("clearing the old index entry of an artist")?;
        sqlx::query("INSERT INTO artists_fts (rowid, name) VALUES (?, ?)")
            .bind(id)
            .bind(name.trim())
            .execute(&mut **tx)
            .await
            .context("indexing an artist")?;

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
            let id = *id;
            return Ok(id);
        }

        // A year nobody tagged must not split an album.
        //
        // The year is in the key so that an original and its remaster stay
        // apart, but that only holds when both are actually tagged. An album
        // with the year missing from some of its tracks is far more common than
        // two editions of one album in the same library, and without this it
        // would come out as two albums of one track each.
        if let Some((artist, name, date)) = key.grouping() {
            let slot = (artist.to_string(), name.to_string());
            if let Some(candidates) = self.albums_by_name.get(&slot) {
                let compatible = candidates.iter().find(|(year, _)| match (year, date) {
                    (Some(recorded), Some(incoming)) => recorded == incoming,
                    // Either side unknown: same album as far as anyone can tell.
                    _ => true,
                });

                if let Some((recorded, id)) = compatible {
                    let id = *id;
                    // Fill in a year the album did not have, so the next track
                    // to arrive matches on it directly.
                    if recorded.is_none() && date.is_some() {
                        sqlx::query("UPDATE albums SET year = ?, release_date = ? WHERE id = ?")
                            .bind(metadata.year)
                            .bind(&metadata.date)
                            .bind(id)
                            .execute(&mut **tx)
                            .await
                            .context("filling in the year of an album")?;

                        if let Some(entry) = self
                            .albums_by_name
                            .get_mut(&slot)
                            .and_then(|c| c.iter_mut().find(|(_, existing)| *existing == id))
                        {
                            entry.0 = date.map(str::to_string);
                        }
                    }

                    self.albums.insert(key.clone(), id);
                    return Ok(id);
                }
            }
        }

        let name = metadata.album.clone().unwrap_or_default();
        let is_compilation = i64::from(matches!(key, AlbumKey::Compilation { .. }));

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO albums (
                 public_id, name, sort_name, year, release_date, is_compilation,
                 mbid_release, mbid_release_group, label, rg_album_gain, rg_album_peak,
                 created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(&metadata.label)
        .bind(metadata.rg_album_gain)
        .bind(metadata.rg_album_peak)
        .bind(&self.timestamp)
        .bind(&self.timestamp)
        .fetch_one(&mut **tx)
        .await
        .with_context(|| format!("inserting album {name}"))?;

        sqlx::query("DELETE FROM albums_fts WHERE rowid = ?")
            .bind(id)
            .execute(&mut **tx)
            .await
            .context("clearing the old index entry of an album")?;
        sqlx::query("INSERT INTO albums_fts (rowid, name, artists) VALUES (?, ?, ?)")
            .bind(id)
            .bind(&name)
            .bind(metadata.album_artists.join(" "))
            .execute(&mut **tx)
            .await
            .context("indexing an album")?;

        if let Some((artist, name, date)) = key.grouping() {
            self.albums_by_name
                .entry((artist.to_string(), name.to_string()))
                .or_default()
                .push((date.map(str::to_string), id));
        }

        self.albums.insert(key, id);
        Ok(id)
    }
}

/// A file with no readable title is still a track: the file name is a better
/// answer than nothing, and it is what the user ends up seeing in a client.
fn resolve_title(path: &Path, metadata: &Metadata) -> String {
    metadata.title.clone().unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string())
    })
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

/// Whether two library roots are the same place or one inside the other.
///
/// Compared by path components rather than as text, so `/music` does not look
/// like a parent of `/musicians`.
///
/// Two libraries that overlap have no meaning. Every file under the shared part
/// belongs to both, so it is scanned twice and counted twice — measured: a
/// directory registered twice this way turned 48 tracks into 96 and 4 albums into
/// 8 — and there is no answer to which library it is in, which is the question
/// every per-account permission is asked.
pub fn overlaps(one: &Path, other: &Path) -> bool {
    one.starts_with(other) || other.starts_with(one)
}

/// Brings the `libraries` table in line with what the environment declares.
///
/// Paths that disappear from the configuration are disabled rather than
/// deleted: the row still owns folders and tracks, which in turn own the
/// user's favourites and playlist entries. Removing a library from the
/// configuration is not a request to lose all of that.
pub async fn sync_libraries(pool: &SqlitePool, paths: &[PathBuf]) -> Result<()> {
    let timestamp = db::now();

    // What is registered already, to keep the environment from declaring a
    // directory that sits inside one of them — or around one.
    let mut roots: Vec<PathBuf> = sqlx::query_scalar::<_, String>("SELECT path FROM libraries")
        .fetch_all(pool)
        .await
        .context("reading the libraries already registered")?
        .into_iter()
        .map(PathBuf::from)
        .collect();

    for path in paths {
        // Refused rather than registered, and said out loud: the alternative is
        // scanning the same files twice under two names and leaving somebody to
        // work out why every album is duplicated.
        if let Some(clash) = roots
            .iter()
            .find(|root| *root != path && overlaps(path, root))
        {
            warn!(
                "not registering {}: it overlaps the library at {}",
                path.display(),
                clash.display()
            );
            continue;
        }

        roots.push(path.clone());

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

    // Nothing is disabled for being absent from the list. The environment adds
    // and enables; it does not decide what else may exist. Two reasons: a
    // library added from the panel would otherwise be undone by the next
    // restart, and a library carries references from playlists and favourites,
    // so making it disappear because a variable moved is the same mistake as
    // sweeping a disk that failed to mount. Removing one is something somebody
    // asks for.
    Ok(())
}

/// How many snapshots the channel holds for a watcher that has stopped reading.
///
/// Small on purpose. Every snapshot carries the whole state, so a watcher that
/// falls behind loses nothing by skipping to the newest one — there is no history
/// worth keeping.
const UPDATE_DEPTH: usize = 8;

/// How often a running scan tells anybody watching, at most.
///
/// Four times a second is as fast as a display is worth updating and slow enough
/// that a scan reading five thousand files a second is not mostly publishing.
const UPDATE_INTERVAL: Duration = Duration::from_millis(250);

/// One kind of thing a scan has just dealt with.
#[derive(Debug, Clone, Copy)]
pub enum Item {
    Folder,
    /// A file that was read. `readable` is false when its tags could not be
    /// understood, which still counts as a track: the file is there.
    Track {
        readable: bool,
    },
    /// A file that has not changed since the last scan, so it was not reopened.
    Unchanged,
}

/// Everything known about the scan in flight, or the last one to run.
///
/// One value rather than a stream of deltas, which is what makes it safe to drop
/// updates under load: whichever one a watcher receives is complete on its own.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub scanning: bool,
    /// Name of the library being walked, while one is.
    pub library: Option<String>,
    /// What the scan was looking at when this was taken. Sampled, not every
    /// file: it exists to show that something is happening.
    pub path: Option<String>,
    pub folders: u64,
    pub tracks: u64,
    pub unchanged: u64,
    pub failed: u64,
    pub gone: u64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Set when the last scan gave up rather than finishing, so a panel can say
    /// so instead of showing figures that were rolled back.
    pub cancelled: bool,
}

/// The parts that need a lock. They change far less often than the counters, and
/// on the hot path they are not touched at all.
#[derive(Debug, Default)]
struct Current {
    library: Option<String>,
    path: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    cancelled: bool,
    published: Option<Instant>,
}

/// Live progress of a scan: what to report, and what keeps two from running at
/// once.
#[derive(Debug)]
pub struct Progress {
    scanning: AtomicBool,
    /// Asked to give up. Cleared when the scan ends, so cancelling one scan does
    /// not stop the next.
    cancel: AtomicBool,
    folders: AtomicU64,
    tracks: AtomicU64,
    unchanged: AtomicU64,
    failed: AtomicU64,
    gone: AtomicU64,
    current: RwLock<Current>,
    updates: broadcast::Sender<Snapshot>,
}

impl Default for Progress {
    fn default() -> Self {
        let (updates, _) = broadcast::channel(UPDATE_DEPTH);
        Self {
            scanning: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            folders: AtomicU64::new(0),
            tracks: AtomicU64::new(0),
            unchanged: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            gone: AtomicU64::new(0),
            current: RwLock::new(Current::default()),
            updates,
        }
    }
}

impl Progress {
    pub fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::Relaxed)
    }

    /// Items processed by the scan in flight, or by the last one to finish.
    pub fn counted(&self) -> u64 {
        self.tracks.load(Ordering::Relaxed)
    }

    /// Follows the scan from now on. The caller gets whatever is published next;
    /// for the state as it stands, ask for a [`Progress::snapshot`] first.
    pub fn subscribe(&self) -> broadcast::Receiver<Snapshot> {
        self.updates.subscribe()
    }

    /// Everything known right now.
    pub fn snapshot(&self) -> Snapshot {
        let current = self.current.read().unwrap_or_else(|e| e.into_inner());

        Snapshot {
            scanning: self.is_scanning(),
            library: current.library.clone(),
            path: current.path.clone(),
            folders: self.folders.load(Ordering::Relaxed),
            tracks: self.tracks.load(Ordering::Relaxed),
            unchanged: self.unchanged.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            gone: self.gone.load(Ordering::Relaxed),
            started_at: current.started_at.clone(),
            finished_at: current.finished_at.clone(),
            cancelled: current.cancelled,
        }
    }

    /// Asks the scan in flight to give up, whether because somebody pressed a
    /// button or because the process is going away.
    ///
    /// A scan is the one thing here that keeps a database transaction open for
    /// minutes at a time, and nothing can close the database from under it.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    fn should_stop(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// Claims the right to scan. `None` means one is already running.
    ///
    /// The exchange is what makes this safe: two requests arriving together
    /// cannot both come away thinking they won.
    fn begin(&self) -> Option<Running<'_>> {
        self.scanning
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| {
                for counter in [
                    &self.folders,
                    &self.tracks,
                    &self.unchanged,
                    &self.failed,
                    &self.gone,
                ] {
                    counter.store(0, Ordering::Relaxed);
                }

                *self.current.write().unwrap_or_else(|e| e.into_inner()) = Current {
                    started_at: Some(db::now()),
                    ..Current::default()
                };

                self.announce();
                Running(self)
            })
    }

    /// Says which library the scan has moved on to.
    fn entering(&self, library: &str) {
        self.current
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .library = Some(library.to_string());
        self.announce();
    }

    /// Records one item, and every so often says where the scan has got to.
    ///
    /// The path is only stored when an update is due. It changes on every file,
    /// and a string that will be replaced before anybody reads it is an
    /// allocation for nothing; the counters are lock free either way.
    fn observed(&self, item: Item, path: &Path) {
        match item {
            Item::Folder => {
                self.folders.fetch_add(1, Ordering::Relaxed);
            }
            Item::Track { readable } => {
                self.tracks.fetch_add(1, Ordering::Relaxed);
                if !readable {
                    self.failed.fetch_add(1, Ordering::Relaxed);
                }
            }
            Item::Unchanged => {
                self.tracks.fetch_add(1, Ordering::Relaxed);
                self.unchanged.fetch_add(1, Ordering::Relaxed);
            }
        }

        let mut current = self.current.write().unwrap_or_else(|e| e.into_inner());
        if current
            .published
            .is_some_and(|last| last.elapsed() < UPDATE_INTERVAL)
        {
            return;
        }

        current.path = Some(path.to_string_lossy().to_string());
        current.published = Some(Instant::now());
        drop(current);

        self.announce();
    }

    fn swept(&self, gone: u64) {
        self.gone.fetch_add(gone, Ordering::Relaxed);
        self.announce();
    }

    /// Publishes the state as it stands. A send with nobody listening fails, and
    /// that is the ordinary case: no panel is open.
    fn announce(&self) {
        let _ = self.updates.send(self.snapshot());
    }
}

/// Ends the scan however it ended, panic included. Without this, one failure
/// would leave the server refusing to scan until restarted.
struct Running<'a>(&'a Progress);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let cancelled = self.0.should_stop();

        {
            let mut current = self.0.current.write().unwrap_or_else(|e| e.into_inner());
            current.finished_at = Some(db::now());
            current.library = None;
            current.path = None;
            current.cancelled = cancelled;
        }

        // Cleared here rather than by whoever asked, so cancelling one scan
        // never stops the one after it.
        self.0.cancel.store(false, Ordering::Release);
        self.0.scanning.store(false, Ordering::Release);
        self.0.announce();
    }
}
