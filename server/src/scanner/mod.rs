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

/// When a scan lets go of the write lock, which is the whole of what keeps the rest
/// of the server working while one is running.
#[cfg(test)]
mod letting_go_tests {
    use super::{AT_A_TIME, HOLDING, time_to_let_go};
    use std::time::Duration;

    /// The case this exists for. A library of large files spends its time reading
    /// tags, so the loop sits on the channel with a barely started transaction and
    /// the lock in its hand — and a count of rows would let it sit there for the
    /// length of the scan. That is the shape of the failure: eleven thousand files,
    /// and every login, cover and play in the server timing out against it.
    #[test]
    fn a_transaction_held_too_long_goes_however_little_is_in_it() {
        assert!(time_to_let_go(1, HOLDING));
        assert!(time_to_let_go(0, HOLDING + Duration::from_millis(1)));
    }

    /// And the other way: files arriving as fast as they can be written must not
    /// grow one transaction without bound either.
    #[test]
    fn a_full_transaction_goes_however_recently_it_was_opened() {
        assert!(time_to_let_go(AT_A_TIME, Duration::ZERO));
    }

    /// In between it keeps going, because a commit is an fsync and one per file
    /// would make a scan pay for this several thousand times over.
    #[test]
    fn a_young_and_half_full_transaction_carries_on() {
        assert!(!time_to_let_go(AT_A_TIME / 2, HOLDING / 2));
        assert!(!time_to_let_go(0, Duration::ZERO));
    }

    /// The limits are what they claim to be. Both are read by whoever tunes this,
    /// and a HOLDING anywhere near the busy timeout would defeat the point.
    #[test]
    fn the_limits_leave_room_under_the_busy_timeout() {
        assert!(
            HOLDING * 4 < crate::db::BUSY_TIMEOUT,
            "a scan may hold the lock for {HOLDING:?} against a timeout of {:?}",
            crate::db::BUSY_TIMEOUT
        );
    }
}
mod walker;

use crate::db;
use crate::db::InTurn;
use album_key::AlbumKey;
use anyhow::{Context, Result};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tags::Metadata;
pub use tags::read as read_tags;
pub use tags::read_artist_picture;
pub use tags::read_every as read_every_tag;
pub use tags::read_with_cover_art as read_tags_with_cover_art;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};
use walker::Entry;

/// An album already recorded: the year it was filed under, and its row.
type RecordedAlbum = (Option<String>, i64);

/// Albums reachable by the key that says which record they are, each with the
/// years already filed under it. What lets a track with no year join the album
/// its siblings belong to, and what lets a second scan find the record the first
/// one made instead of making it again.
type AlbumsByKey = HashMap<String, Vec<RecordedAlbum>>;

/// What a scan already knows about a file it is about to look at: how it looked from
/// outside last time, and whether looking inside worked.
struct Recorded {
    modified: String,
    size: i64,
    unreadable: bool,
}

/// How many scanned entries wait in the channel. Reading tags is slower than
/// inserting rows, so this only has to be deep enough that the writer never
/// idles between files.
const CHANNEL_DEPTH: usize = 256;

/// How many rows one transaction of a scan holds before it is committed.
///
/// SQLite has one writer, so for as long as a scan holds a transaction open,
/// nothing else in the server can write: no login recorded, no cover cached, no
/// play counted. A scan is minutes and everything else is milliseconds, which is
/// why the scan is the one that gives way.
///
/// A thousand because a commit is an fsync and the point is to be interruptible
/// rather than to be small: eleven thousand files come to a dozen commits, which
/// costs nothing measurable and leaves eleven gaps for everybody else to write in.
const AT_A_TIME: usize = 1_000;

/// And how long it may hold that transaction open whatever it has written, which
/// is the limit that actually binds. See where it is used.
const HOLDING: Duration = Duration::from_millis(500);

/// How long the scan keeps its hands off the database between transactions.
///
/// Ten milliseconds against five hundred: two per cent of a scan, and the whole of
/// what makes the lock reachable by anybody else. Without it, committing more often
/// only meant asking for the lock more often.
const STANDING_BACK: Duration = Duration::from_millis(10);

/// Whether the scan should commit what it has and let go of the write lock.
///
/// Either limit is enough on its own, and they are there for different reasons: the
/// count keeps a transaction from growing without bound when files are arriving as
/// fast as they can be written, and the elapsed time is what bounds the wait for
/// everybody else — because this loop spends most of a scan waiting on the reader,
/// and it waits holding the lock.
fn time_to_let_go(written: usize, held: Duration) -> bool {
    written >= AT_A_TIME || held >= HOLDING
}

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
            .in_turn(pool)
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
    .in_turn(pool)
    .await
    .context("recording the start of the scan")?;

    // What is already recorded, so the reader can skip files that have not
    // changed without asking the database once per file.
    //
    // Joined back onto the root on the way in: the rows are relative, and every
    // path the walker produces is absolute. Composing here rather than
    // relativising a million times in the loop below.
    let known: HashMap<PathBuf, Recorded> = sqlx::query_as::<_, (String, String, i64, bool)>(
        "SELECT path, file_modified_at, file_size, unreadable_since IS NOT NULL
           FROM tracks WHERE library_id = ?",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await
    .context("loading the tracks already recorded")?
    .into_iter()
    .map(|(path, modified, size, unreadable)| {
        (
            root.join(path),
            Recorded {
                modified,
                size,
                unreadable,
            },
        )
    })
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
                    //
                    // And never a file we could not read last time, whatever its size
                    // and time say. Those two are how the file looked from outside,
                    // and from outside nothing changed: the permissions came back, the
                    // disk started answering, and the file is exactly as it was. It
                    // would be skipped for ever and stay as bare as the failed scan
                    // left it, which is what happened.
                    let unchanged = mode == Mode::Incremental
                        && recorded.is_some_and(|recorded| {
                            !recorded.unreadable
                                && recorded.size == size as i64
                                && recorded.modified == epoch_to_iso8601(modified)
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
    let mut written = 0usize;
    let mut held = Instant::now();

    while let Some(scanned) = rx.recv().await {
        // What has been committed stays committed, and that is the change: this
        // used to be one transaction for the whole library.
        //
        // A scan of eleven thousand files held the write lock for as long as it
        // ran, and SQLite has one writer — so every other write in the server
        // waited on it and then failed: a login could not be recorded, a cover
        // could not be cached, and the panel reported the first as a wrong
        // password. Readers were never the problem, since the journal is WAL.
        //
        // What that single transaction bought was a scan being all or nothing.
        // That mattered for one reason — a scan stopped half way through must not
        // sweep everything it had not reached yet into missing — and the sweep is
        // one statement at the end. So the sweep is what has to be all or nothing,
        // and it still is: it runs after the walk finishes and never after an
        // interrupted one. What a stopped scan leaves behind is rows for the files
        // it did get to, which the next scan reads again and stamps with its own
        // number.
        if progress.should_stop() {
            info!("scan interrupted; nothing was swept and what it read is kept");
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

        written += 1;

        // Two limits rather than one, and the second is the one that does the work.
        // A count alone would bound nothing: this loop spends most of its life
        // waiting on the channel for the reader to open the next file, and it was
        // waiting with the write lock in its hand — so on a library of big files it
        // would sit on a half full transaction for minutes without writing a row.
        if time_to_let_go(written, held.elapsed()) {
            tx_db
                .commit()
                .await
                .context("committing part of the scan")?;

            // And then stand back, which is not the same as committing.
            //
            // Committing alone left a request still waiting 2.2 seconds for the
            // lock, because SQLite hands it to whoever asks and not to whoever has
            // waited: this loop commits and asks again in the same breath, so it can
            // win the lock over and over while somebody else's single write times
            // out beside it. The gap is what makes the waiting end.
            tokio::time::sleep(STANDING_BACK).await;

            tx_db = db::writing(pool)
                .await
                .context("carrying on with the scan")?;
            written = 0;
            held = Instant::now();
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
    .in_turn(pool)
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
    /// Artists this scan has seen, by name folded for comparison, each with whether
    /// a MusicBrainz id has been written onto it yet. Without that second half, a
    /// file naming somebody already seen would go straight past the chance to
    /// identify them.
    artists: HashMap<String, (i64, bool)>,
    /// The albums this scan has looked up so far. A key that is present has been
    /// answered for — from the database on the first track of a record, from here
    /// on every track after it — so a missing key means "not asked yet" and an
    /// empty list means "asked, and there is no such record".
    albums: AlbumsByKey,
    /// The albums this scan has written, so a record it found rather than made is
    /// brought up to date once — by the first track of it that comes through —
    /// and not once per track, where the last file read would be the one that
    /// decided what the whole record says.
    rewritten: HashSet<i64>,
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
            albums: AlbumsByKey::new(),
            rewritten: HashSet::new(),
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

        // A file that would not open keeps whatever was already known about it.
        //
        // Everything below this writes the tags it read, and with nothing read that
        // means writing blanks: a track that was scanned correctly last week, and
        // whose permissions were taken away since, had its title replaced by its
        // file name and its album, length and numbering set to null. The scan was
        // destroying the answer it could no longer see.
        //
        // What is written instead is what can be seen without opening the file — how
        // big it is, when it changed — and the note that it could not be read, which
        // is what brings the next quick scan back to it.
        let unreadable = metadata.is_none();

        if unreadable {
            let kept = sqlx::query(
                "UPDATE tracks
                    SET file_size = ?, file_modified_at = ?, content_type = ?, suffix = ?,
                        missing_since = NULL,
                        unreadable_since = coalesce(unreadable_since, ?),
                        last_seen_scan = ?, updated_at = ?
                  WHERE library_id = ? AND path = ?",
            )
            .bind(size as i64)
            .bind(epoch_to_iso8601(modified))
            .bind(content_type(extension))
            .bind(extension)
            // The first scan that could not read it, kept through the ones after:
            // "unreadable since Tuesday" is worth more than "unreadable just now".
            .bind(&self.timestamp)
            .bind(self.scan)
            .bind(&self.timestamp)
            .bind(self.library_id)
            .bind(self.relative(path))
            .execute(&mut **tx)
            .await
            .with_context(|| format!("keeping what was known about {}", path.display()))?;

            if kept.rows_affected() > 0 {
                return Ok(());
            }

            // Nothing was known: it is a new file that will not open. It still gets a
            // row — it is there, and a listing that left it out would be a listing
            // that hides a problem — with its name for a title and the note on it.
        }

        let metadata = metadata.map(|m| *m).unwrap_or_default();

        // The key is kept beside the record it found rather than consumed: it
        // decides which record this is, and it also decides who that record is
        // credited to — see `AlbumKey::credited`. Kept as one value because there
        // is no such thing as a record without a key or a key without a record.
        let album = match AlbumKey::of(&metadata) {
            Some(key) => {
                let id = self.album_id(tx, &key, &metadata).await?;
                Some((id, key))
            }
            None => None,
        };

        let title = resolve_title(path, &metadata);

        let track_id: i64 = sqlx::query_scalar(
            "INSERT INTO tracks (
                 public_id, library_id, folder_id, album_id, path, file_size,
                 file_modified_at, content_type, suffix, title, sort_title,
                 artist_credit, track_number, disc_number, year, duration_ms, bit_rate,
                 bit_depth, sampling_rate, channel_count, bpm, comment,
                 mbid_recording, mbid_track, isrc, rg_track_gain, rg_track_peak,
                 unreadable_since, last_seen_scan, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                       ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, path) DO UPDATE SET
                 folder_id = excluded.folder_id,
                 album_id = excluded.album_id,
                 file_size = excluded.file_size,
                 file_modified_at = excluded.file_modified_at,
                 content_type = excluded.content_type,
                 suffix = excluded.suffix,
                 title = excluded.title,
                 sort_title = excluded.sort_title,
                 artist_credit = excluded.artist_credit,
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
                 -- Read this time, so whatever was wrong with it before is over.
                 unreadable_since = excluded.unreadable_since,
                 last_seen_scan = excluded.last_seen_scan,
                 updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(db::public_id()?)
        .bind(self.library_id)
        .bind(folder_id)
        .bind(album.as_ref().map(|(id, _)| *id))
        .bind(self.relative(path))
        .bind(size as i64)
        .bind(epoch_to_iso8601(modified))
        .bind(content_type(extension))
        .bind(extension)
        .bind(&title)
        .bind(&metadata.sort_title)
        // Only when it says something the names do not, which is what decides it —
        // see `Metadata::credited_as`.
        .bind(metadata.credited_as())
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
        // Only reached with nothing read when the file is new: everything already
        // known took the shorter road above.
        .bind(unreadable.then(|| self.timestamp.clone()))
        .bind(self.scan)
        .bind(&self.timestamp)
        .bind(&self.timestamp)
        .fetch_one(&mut **tx)
        .await
        .with_context(|| format!("inserting track {}", path.display()))?;

        self.link_track(tx, track_id, &metadata).await?;
        self.index_track(tx, track_id, &title, &metadata).await?;

        if let Some((album_id, key)) = &album {
            self.link_album(tx, *album_id, key, &metadata).await?;
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

        for (position, (name, mbid)) in
            tags::identified(&metadata.artists, &metadata.mbid_artists).enumerate()
        {
            let artist_id = self.artist_id(tx, name, mbid).await?;
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
        key: &AlbumKey,
        metadata: &Metadata,
    ) -> Result<()> {
        for (position, (name, mbid)) in key.credited(metadata).enumerate() {
            let artist_id = self.artist_id(tx, name, mbid).await?;
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

    /// The row for a name, and the MusicBrainz id written onto it the first time one
    /// arrives with it.
    ///
    /// The name is still what identifies an artist here — an id is a fact about them,
    /// not the way they are found — so a file that names somebody already known adds
    /// its id to the row they already have.
    async fn artist_id(
        &mut self,
        tx: &mut Transaction<'_, Sqlite>,
        name: &str,
        mbid: Option<&str>,
    ) -> Result<i64> {
        let key = name.trim().to_lowercase();
        if let Some((id, identified)) = self.artists.get(&key).copied() {
            // Known already, and now named with an id it did not have: the first file
            // of a record may credit somebody the tagger had not yet looked up.
            if !identified && mbid.is_some() {
                self.identify(tx, id, mbid).await?;
                self.artists.insert(key, (id, true));
            }

            return Ok(id);
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

        self.identify(tx, id, mbid).await?;

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

        self.artists.insert(key, (id, mbid.is_some()));
        Ok(id)
    }

    /// Writes a MusicBrainz id onto an artist that has none.
    ///
    /// Never over one that is already there: an artist whose files disagree about
    /// which person they are has a tagging mistake, and the last file read is no
    /// better an answer than the first.
    ///
    /// And never where another artist already holds it. The schema makes the id
    /// unique — two rows claiming to be the same person is not a state worth
    /// keeping — so without that condition this statement would fail, and a failed
    /// statement here fails the scan. It happens: two spellings of one name, both
    /// tagged with the same id, is the ordinary shape of a library somebody has been
    /// tidying up.
    async fn identify(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        id: i64,
        mbid: Option<&str>,
    ) -> Result<()> {
        let Some(mbid) = mbid else { return Ok(()) };

        sqlx::query(
            "UPDATE artists SET mbid = ?1, updated_at = ?2
               WHERE id = ?3
                 AND mbid IS NULL
                 AND NOT EXISTS (SELECT 1 FROM artists WHERE mbid = ?1)",
        )
        .bind(mbid)
        .bind(&self.timestamp)
        .bind(id)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("identifying artist {id} as {mbid}"))?;

        Ok(())
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
        key: &AlbumKey,
        metadata: &Metadata,
    ) -> Result<i64> {
        let slot = key.grouping_key();

        // The records already filed under this key, whoever filed them. Asked
        // once and remembered, which is what keeps the second track of a record
        // from going back to the database — and asked at all, which is what
        // keeps a scan that rereads a known file from recording the record it
        // already has a second time.
        if !self.albums.contains_key(&slot) {
            let recorded: Vec<(Option<i64>, i64)> =
                sqlx::query_as("SELECT year, id FROM albums WHERE grouping_key = ?")
                    .bind(&slot)
                    .fetch_all(&mut **tx)
                    .await
                    .with_context(|| format!("looking for an album already recorded as {slot}"))?;

            self.albums.insert(
                slot.clone(),
                recorded
                    .into_iter()
                    .map(|(year, id)| (year.map(|year| year.to_string()), id))
                    .collect(),
            );
        }

        // A year nobody tagged must not split an album.
        //
        // The year is compared here rather than being part of the key so that an
        // original and its remaster stay apart, but that only holds when both are
        // actually tagged. An album with the year missing from some of its tracks
        // is far more common than two editions of one album in the same library,
        // and without this it would come out as two albums of one track each.
        //
        // A release id takes no year at all: it has already said which record
        // this is, so whatever year is filed under it is this record's year.
        let date = key.grouping().and_then(|(_, _, date)| date);

        let compatible = self
            .albums
            .entry(slot.clone())
            .or_default()
            .iter()
            .find(|(year, _)| match (year, date) {
                (Some(recorded), Some(incoming)) => recorded == incoming,
                // Either side unknown: same album as far as anyone can tell.
                _ => true,
            })
            .map(|(year, id)| (year.is_none(), *id));

        if let Some((undated, id)) = compatible {
            // Fill in a year the album did not have, so the next track to arrive
            // matches on it directly.
            if undated && date.is_some() {
                sqlx::query("UPDATE albums SET year = ?, release_date = ? WHERE id = ?")
                    .bind(metadata.year)
                    .bind(&metadata.date)
                    .bind(id)
                    .execute(&mut **tx)
                    .await
                    .context("filling in the year of an album")?;

                if let Some(entry) = self
                    .albums
                    .get_mut(&slot)
                    .and_then(|c| c.iter_mut().find(|(_, existing)| *existing == id))
                {
                    entry.0 = date.map(str::to_string);
                }
            }

            // A record the scan found rather than made was written the last time
            // somebody read these files, and the tags may have been corrected
            // since. So the first track of it this scan reads writes the record
            // again — the same track that would have created it, so a record that
            // already existed and one that did not end up saying the same thing.
            if self.rewritten.insert(id) {
                self.rewrite_album(tx, id, key, metadata).await?;
            }

            return Ok(id);
        }

        let name = metadata.album.clone().unwrap_or_default();
        let is_compilation = i64::from(matches!(key, AlbumKey::Compilation { .. }));

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO albums (
                 public_id, grouping_key, name, sort_name, year, release_date,
                 is_compilation, mbid_release, mbid_release_group, label,
                 rg_album_gain, rg_album_peak, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(db::public_id()?)
        .bind(&slot)
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

        self.index_album(tx, id, &name, key, metadata).await?;

        self.albums
            .entry(slot)
            .or_default()
            .push((date.map(str::to_string), id));
        self.rewritten.insert(id);

        Ok(id)
    }

    /// Writes what the tags say about a record onto the row that already holds
    /// it, leaving the row itself alone.
    ///
    /// Everything here is a fact the files carry and can be corrected in them:
    /// the name as it is written, how it sorts, the label, the release date, the
    /// identifiers, the album gain. What is not here is what the row means to
    /// whoever has it: its public id, when it was first seen, the cover chosen
    /// for it, and the plays, ratings and stars hanging off it.
    ///
    /// The year is not here either, because a different year is a different
    /// record — a remaster, filed apart on purpose — and the one case where it
    /// should be written is the album that had none, which is handled above.
    async fn rewrite_album(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        id: i64,
        key: &AlbumKey,
        metadata: &Metadata,
    ) -> Result<()> {
        let name = metadata.album.clone().unwrap_or_default();

        sqlx::query(
            "UPDATE albums SET
                 name = ?, sort_name = ?, release_date = ?, mbid_release = ?,
                 mbid_release_group = ?, label = ?, rg_album_gain = ?,
                 rg_album_peak = ?, updated_at = ?
               WHERE id = ?",
        )
        .bind(&name)
        .bind(&metadata.sort_album)
        .bind(&metadata.date)
        .bind(&metadata.mbid_release)
        .bind(&metadata.mbid_release_group)
        .bind(&metadata.label)
        .bind(metadata.rg_album_gain)
        .bind(metadata.rg_album_peak)
        .bind(&self.timestamp)
        .bind(id)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("bringing album {name} up to date"))?;

        self.index_album(tx, id, &name, key, metadata).await
    }

    /// Puts a record into the search index under the name it goes by and whoever
    /// it is credited to, replacing whatever was indexed for it before.
    async fn index_album(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        id: i64,
        name: &str,
        key: &AlbumKey,
        metadata: &Metadata,
    ) -> Result<()> {
        sqlx::query("DELETE FROM albums_fts WHERE rowid = ?")
            .bind(id)
            .execute(&mut **tx)
            .await
            .context("clearing the old index entry of an album")?;

        sqlx::query("INSERT INTO albums_fts (rowid, name, artists) VALUES (?, ?, ?)")
            .bind(id)
            .bind(name)
            // Whoever the record is credited to, which for an album with no album
            // artist tagged is its track artists — otherwise searching an album by
            // the one name written on it would not find it.
            .bind(
                key.credited(metadata)
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>()
                    .join(" "),
            )
            .execute(&mut **tx)
            .await
            .context("indexing an album")?;

        Ok(())
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
        .in_turn(pool)
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
