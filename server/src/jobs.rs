// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The jobs somebody runs when something is off.
//!
//! None of these happens on its own and none of them is part of serving
//! anything: they are the answers to "the disk is fuller than it should be",
//! "the covers I added are not showing up", "something is wrong and I do not
//! know what". A server nobody touches never needs any of them.
//!
//! Every job says what it would do before it does it, and every run is written
//! down. Both for the same reason: a button that acts on an unknown number of
//! things, and then says nothing about what it did, is a button nobody presses
//! twice.
//!
//! They run in the request that asked for them rather than in a task of their
//! own. A scan is minutes and has a whole progress stream to itself; these are
//! seconds, and waiting for the answer is what the caller wants.

use crate::db::InTurn;
use crate::db::now;
use crate::types::{Job, Run};
use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Every job there is, in the order a screen should offer them: what somebody
/// most often wants first, and the one that only reports last.
pub const EVERY: [Job; 5] = [
    Job::Purge,
    Job::Covers,
    Job::Compact,
    Job::Check,
    Job::Forget,
];

/// The job that answers to a name, or nothing if none does.
pub fn named(name: &str) -> Option<Job> {
    EVERY.into_iter().find(|job| job.name() == name)
}

/// How many problems of a check are worth keeping. Whatever is wrong with a
/// database will be wrong in the first few lines as well as in the last
/// thousand, and the row that holds this is read by a person.
const PROBLEMS_KEPT: usize = 20;

/// What a run came to.
struct Outcome {
    /// In whatever the job counts.
    affected: i64,
    /// What it has to say beyond the number, which only the check ever has.
    note: Option<String>,
}

impl Outcome {
    fn of(affected: i64) -> Self {
        Self {
            affected,
            note: None,
        }
    }
}

/// How much this job would affect if it ran now, or `None` for one that changes
/// nothing and so has nothing to warn about.
pub async fn pending(pool: &SqlitePool, data_dir: &Path, job: Job) -> Result<Option<i64>> {
    let count = match job {
        Job::Purge => {
            sqlx::query_scalar("SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL")
                .fetch_one(pool)
                .await
                .context("counting what a purge would remove")?
        }
        Job::Compact => reclaimable(pool).await?,
        Job::Covers => orphaned(pool, data_dir, crate::artwork::CACHE_DIRECTORY)
            .await?
            .len() as i64,
        Job::Forget => fetched(pool).await?,
        // It reads and reports. There is nothing to say in advance beyond that.
        Job::Check => return Ok(None),
    };

    Ok(Some(count))
}

/// What this job would look at again rather than take away, which only one of
/// them does at all.
///
/// Apart from [`pending`] because the two are not the same kind of thing, and
/// the sum of them was a number that meant nothing: a server that had been
/// scanned once and browsed for five minutes reported "12 things the cache
/// should not be holding", of which six were true answers that had simply had
/// time to go stale.
pub async fn revisiting(pool: &SqlitePool, job: Job) -> Result<Option<i64>> {
    match job {
        Job::Covers => unfound(pool).await.map(Some),
        _ => Ok(None),
    }
}

/// Runs one, and writes down that it ran.
///
/// A job that fails comes back as a run carrying the reason rather than as an
/// error: the caller asked for the job to be attempted, and it was. Only a
/// failure to write the history down is a failure of this call.
pub async fn run(pool: &SqlitePool, data_dir: &Path, job: Job) -> Result<Run> {
    let at = now();
    let id: i64 =
        sqlx::query_scalar("INSERT INTO job_runs (job, started_at) VALUES (?, ?) RETURNING id")
            .bind(job.name())
            .bind(&at)
            .in_turn(pool)
            .await
            .context("recording that a job started")?;

    let done = match job {
        Job::Purge => purge(pool, data_dir).await,
        Job::Compact => compact(pool).await,
        Job::Covers => covers(pool, data_dir).await,
        Job::Forget => forget(pool, data_dir).await,
        Job::Check => check(pool).await,
    };

    let (affected, error) = match done {
        Ok(outcome) => {
            info!("{} finished: {}", job.name(), outcome.affected);
            (outcome.affected, outcome.note)
        }
        Err(e) => {
            warn!("{} failed: {e:#}", job.name());
            (0, Some(format!("{e:#}")))
        }
    };

    sqlx::query("UPDATE job_runs SET finished_at = ?, affected = ?, error = ? WHERE id = ?")
        .bind(now())
        .bind(affected)
        .bind(&error)
        .bind(id)
        .in_turn(pool)
        .await
        .context("recording that a job finished")?;

    Ok(Run {
        job,
        at,
        finished: true,
        affected,
        error,
    })
}

/// A row of `job_runs` as it comes back: the job's name, when it started, when
/// it finished if it did, what it found and what went wrong.
type Row = (String, String, Option<String>, i64, Option<String>);

/// The last run of every job that has ever run, newest first.
pub async fn latest(pool: &SqlitePool) -> Result<Vec<Run>> {
    // One row per job: the greatest start, and the rest of that row with it.
    // SQLite picks the values from the row that answered `max`, which is what
    // makes a bare group by right here rather than merely accepted.
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT job, max(started_at), finished_at, affected, error
           FROM job_runs GROUP BY job",
    )
    .fetch_all(pool)
    .await
    .context("reading when each job last ran")?;

    Ok(rows.into_iter().filter_map(read).collect())
}

/// The last few runs of anything, newest first, for the screen's own history.
pub async fn history(pool: &SqlitePool, limit: i64) -> Result<Vec<Run>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT job, started_at, finished_at, affected, error
           FROM job_runs ORDER BY started_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("reading what has been run lately")?;

    Ok(rows.into_iter().filter_map(read).collect())
}

/// A row as a run, or nothing at all for a job this version no longer has.
///
/// The history outlives the list of jobs on purpose — a run that happened
/// happened — and a name nothing answers to is simply not shown.
fn read(row: Row) -> Option<Run> {
    let (job, at, finished_at, affected, error) = row;

    let job = named(&job)?;

    Some(Run {
        job,
        at,
        finished: finished_at.is_some(),
        affected,
        error,
    })
}

/// Everything a scan marked, and the empty shells that leaves behind. Counted in
/// tracks, which is the loss that cannot be scanned back.
async fn purge(pool: &SqlitePool, data_dir: &Path) -> Result<Outcome> {
    let removed = crate::purge::absent(pool, data_dir, None).await?;

    Ok(Outcome::of(removed.tracks))
}

/// Gives back the space deleting things left behind, counted in bytes.
async fn compact(pool: &SqlitePool) -> Result<Outcome> {
    let before = occupied(pool).await?;

    // Not in a transaction, and it takes the database to itself for as long as
    // it runs. Both are SQLite's rules for VACUUM rather than choices here.
    sqlx::query("VACUUM")
        .execute(pool)
        .await
        .context("compacting the database")?;

    let after = occupied(pool).await?;

    Ok(Outcome::of((before - after).max(0)))
}

/// Puts the cover cache in order, in both directions.
///
/// It caches two things: the covers it found, as files, and the fact that it
/// looked and found none, as rows. Both go stale the same way — a library removed
/// leaves files nothing names, and a cover.jpg added afterwards leaves an answer
/// that is no longer true — and both are this cache, so both are this job.
///
/// Counted together, as things that were in the cache and should not have been.
/// The two halves are not the same kind of thing, but neither is worth a job of
/// its own, and a job that cleaned half a cache would be a job somebody has to
/// remember to run twice.
async fn covers(pool: &SqlitePool, data_dir: &Path) -> Result<Outcome> {
    let doomed = orphaned(pool, data_dir, crate::artwork::CACHE_DIRECTORY).await?;
    let mut gone = 0;

    for path in doomed {
        match std::fs::remove_file(&path) {
            Ok(()) => gone += 1,
            Err(e) => warn!("could not remove {}: {e}", path.display()),
        }

        // The two-character directory the cache fans out into, once the last
        // file in it has gone. Left behind they are ours and they are rubbish,
        // and `remove_dir` refuses one that still holds anything, which is the
        // whole check.
        if let Some(fan) = path.parent() {
            let _ = std::fs::remove_dir(fan);
        }
    }

    // Only what was looked for on this disk. Forgetting that is free: the next
    // request opens the files again and the answer costs nothing to have back.
    //
    // The remote ones are not this job's to forget. Behind each of those is a
    // walk to somebody else's server at one request a second, and throwing the
    // answer away would mean paying that walk again to be told the same thing.
    // They grow stale on their own clock instead — see `crate::portraits`.
    let forgotten = sqlx::query("DELETE FROM artwork_lookups WHERE found = 0 AND source = 'local'")
        .in_turn(pool)
        .await
        .context("forgetting the covers that were not found")?
        .rows_affected();

    Ok(Outcome::of(gone + forgotten as i64))
}

/// Throws away what was fetched from the network, and the memory of having
/// asked.
///
/// Both halves, because half of it would be a collection that cannot get back to
/// where it was: the pictures without the lookups leaves every artist without one
/// and unable to be looked up again for three months, and the lookups without the
/// pictures leaves files nothing names.
///
/// The artists are let go of first. A row nothing points at is what the sweep
/// afterwards goes on, and the file has to outlive the row long enough for its
/// hash to be read out of it.
async fn forget(pool: &SqlitePool, data_dir: &Path) -> Result<Outcome> {
    let mut tx = crate::db::writing(pool).await?;

    sqlx::query(
        "UPDATE artists SET artwork_id = NULL
          WHERE artwork_id IN (SELECT id FROM artworks WHERE source = ?)",
    )
    .bind(crate::artwork::FROM_COMMONS)
    .execute(&mut **tx)
    .await
    .context("letting go of the pictures that were fetched")?;

    let gone = sqlx::query("DELETE FROM artworks WHERE source = ?")
        .bind(crate::artwork::FROM_COMMONS)
        .execute(&mut **tx)
        .await
        .context("forgetting the pictures that were fetched")?
        .rows_affected() as i64;

    // The memory of having asked goes with them. Left behind, every artist would
    // be without a picture and out of reach of another look for three months —
    // which is the opposite of what somebody pressing this asked for.
    sqlx::query("DELETE FROM artwork_lookups WHERE source = ?")
        .bind(crate::portraits::SOURCE)
        .execute(&mut **tx)
        .await
        .context("forgetting where it had looked")?;

    tx.commit().await?;

    // After the commit, like every other deletion of a file here: one that will
    // not unlink must not undo what the database already says.
    //
    // By the same walk the covers job uses, over the other directory. Nothing
    // names anything in there now, so this takes the lot — including whatever a
    // download that died between writing the file and writing the row left
    // behind, which is the one kind of rubbish that collects in there.
    for path in orphaned(pool, data_dir, crate::artwork::ACQUIRED_DIRECTORY).await? {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("could not remove {}: {e}", path.display());
        }

        if let Some(fan) = path.parent() {
            let _ = std::fs::remove_dir(fan);
        }
    }

    Ok(Outcome::of(gone))
}

/// How many pictures came off the network, which is what forgetting them would
/// throw away.
async fn fetched(pool: &SqlitePool) -> Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM artworks WHERE source = ?")
        .bind(crate::artwork::FROM_COMMONS)
        .fetch_one(pool)
        .await
        .context("counting the pictures that were fetched")
}

/// Albums the server looked in, found no cover, and stopped looking in.
async fn unfound(pool: &SqlitePool) -> Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM artwork_lookups WHERE found = 0 AND source = 'local'")
        .fetch_one(pool)
        .await
        .context("counting the covers that were not found")
}

/// Reads the database through. Counted in problems, and the problems themselves
/// come back as the note, since this is the one job whose answer is prose.
async fn check(pool: &SqlitePool) -> Result<Outcome> {
    let mut problems = Vec::new();

    // Answers with one row saying "ok" when there is nothing to say.
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(pool)
        .await
        .context("checking the database")?;

    problems.extend(integrity.into_iter().filter(|line| line != "ok"));

    // One row per row that points at something that is not there. Empty is the
    // good answer here, since a clean database has no such rows to report.
    let dangling: Vec<(String, Option<i64>, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(pool)
            .await
            .context("checking what the rows point at")?;

    problems.extend(
        dangling
            .into_iter()
            .map(|(table, rowid, parent, _)| match rowid {
                Some(rowid) => format!("{table} row {rowid} points at no {parent}"),
                None => format!("{table} points at no {parent}"),
            }),
    );

    let found = problems.len() as i64;
    problems.truncate(PROBLEMS_KEPT);

    Ok(Outcome {
        affected: found,
        note: (!problems.is_empty()).then(|| problems.join("\n")),
    })
}

/// Bytes the database file takes, as SQLite counts them.
async fn occupied(pool: &SqlitePool) -> Result<i64> {
    let pages: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(pool)
        .await
        .context("measuring the database")?;

    Ok(pages * page_size(pool).await?)
}

/// Bytes sitting in pages the database has stopped using and has not given back.
async fn reclaimable(pool: &SqlitePool) -> Result<i64> {
    let free: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(pool)
        .await
        .context("measuring what the database could give back")?;

    Ok(free * page_size(pool).await?)
}

async fn page_size(pool: &SqlitePool) -> Result<i64> {
    sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(pool)
        .await
        .context("reading the page size")
}

/// Cached cover files no row names.
///
/// The cache is keyed by the hash of what is in the file, and the row that wants
/// it holds the same hash — so this is one directory walk against one column.
/// Files are left behind by anything that removes an artwork row without going
/// through the purge, which deleting a library does: it cascades into the tracks
/// and leaves the covers of albums nobody can reach.
async fn orphaned(pool: &SqlitePool, data_dir: &Path, directory: &str) -> Result<Vec<PathBuf>> {
    let wanted: HashSet<String> = sqlx::query_scalar("SELECT DISTINCT content_hash FROM artworks")
        .fetch_all(pool)
        .await
        .context("reading which covers are still wanted")?
        .into_iter()
        .collect();

    let cache = data_dir.join(directory);

    // Walking a directory is the filesystem's business rather than the
    // executor's, and a cache with thousands of covers in it would hold a thread
    // that should be answering requests.
    tokio::task::spawn_blocking(move || walk(&cache, &wanted))
        .await
        .context("looking through the artwork cache")
}

/// The walk itself, off the executor.
///
/// A missing cache directory is not a problem to report: it means nothing has
/// ever been cached, and nothing is what there is to clean.
fn walk(cache: &Path, wanted: &HashSet<String>) -> Vec<PathBuf> {
    walkdir::WalkDir::new(cache)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| match hashed(entry.path(), cache) {
            Some(hash) => !wanted.contains(&hash),
            // Something in the cache that is not laid out like a cached cover.
            // Not ours to delete.
            None => false,
        })
        .map(|entry| entry.into_path())
        .collect()
}

/// The hash a cached file stands for, read back out of where it sits: the two
/// characters of its directory and the rest of its name.
fn hashed(path: &Path, cache: &Path) -> Option<String> {
    let inside = path.strip_prefix(cache).ok()?;
    let mut parts = inside.components();

    let prefix = parts.next()?.as_os_str().to_str()?;
    let rest = parts.next()?.as_os_str().to_str()?;

    // Anything deeper is not one of ours.
    parts.next().is_none().then(|| format!("{prefix}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn empty() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    /// A cover row and its cached file, plus a file nothing names.
    fn cached(data_dir: &Path, hash: &str) -> PathBuf {
        let path = crate::artwork::cache_path(data_dir, hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not really an image").unwrap();
        path
    }

    #[tokio::test]
    async fn a_run_is_written_down_with_what_it_found() {
        let pool = empty().await;
        let data_dir = tempdir();

        let done = run(&pool, &data_dir, Job::Check).await.unwrap();

        assert_eq!(done.job, Job::Check);
        assert_eq!(done.affected, 0);
        assert!(done.finished);
        assert_eq!(done.error, None);

        let written = latest(&pool).await.unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].at, done.at);
    }

    /// The file whose hash a row still names has to survive, and the one nothing
    /// names has to go. Getting this backwards deletes covers that are in use.
    #[tokio::test]
    async fn a_cover_nothing_names_is_the_one_that_goes() {
        let pool = empty().await;
        let data_dir = tempdir();

        sqlx::query(
            "INSERT INTO artworks (public_id, kind, source, mime_type, content_hash, fetched_at)
             VALUES ('w1', 'album_front', 'file', 'image/jpeg', 'aabbcc', ?)",
        )
        .bind(db::now())
        .execute(&pool)
        .await
        .unwrap();

        let kept = cached(&data_dir, "aabbcc");
        let doomed = cached(&data_dir, "ddeeff");

        assert_eq!(
            pending(&pool, &data_dir, Job::Covers).await.unwrap(),
            Some(1)
        );

        let done = run(&pool, &data_dir, Job::Covers).await.unwrap();

        assert_eq!(done.affected, 1);
        assert!(kept.exists(), "a row still names it");
        assert!(!doomed.exists());
    }

    /// Forgetting what was fetched puts the collection back where it was before
    /// anybody went looking — which means both halves, and only those.
    ///
    /// Half of it would be worse than none. The pictures without the memory of
    /// having asked leaves every artist without one and out of reach of another
    /// look for three months; the memory without the pictures leaves files
    /// nothing names. And a picture that came off the user's own disk is not
    /// this job's to touch at all.
    #[tokio::test]
    async fn forgetting_what_was_fetched_puts_it_back_where_it_started() {
        let pool = empty().await;
        let data_dir = tempdir();
        let at = db::now();

        for (id, public, source, hash) in [
            (1, "w1", "commons", "aabbcc"),
            (2, "w2", "local_file", "ddeeff"),
        ] {
            sqlx::query(
                "INSERT INTO artworks (id, public_id, kind, source, mime_type, content_hash,
                                       fetched_at)
                 VALUES (?, ?, 'artist', ?, 'image/jpeg', ?, ?)",
            )
            .bind(id)
            .bind(public)
            .bind(source)
            .bind(hash)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO artists (id, public_id, name, artwork_id, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("ar{id}"))
            .bind(format!("Artist {id}"))
            .bind(id)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Where each one's bytes live, which is the whole of the difference.
        let bought = crate::artwork::acquired_path(&data_dir, "aabbcc");
        std::fs::create_dir_all(bought.parent().unwrap()).unwrap();
        std::fs::write(&bought, b"a portrait that cost a walk").unwrap();
        let theirs = cached(&data_dir, "ddeeff");

        // And a download that died between writing the file and writing the row,
        // which is the one kind of rubbish that collects in there.
        let half = crate::artwork::acquired_path(&data_dir, "001122");
        std::fs::create_dir_all(half.parent().unwrap()).unwrap();
        std::fs::write(&half, b"most of a picture").unwrap();

        sqlx::query(
            "INSERT INTO artwork_lookups (entity_type, entity_id, source, attempted_at, found)
             VALUES ('artist', 1, 'commons', ?, 1)",
        )
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            pending(&pool, &data_dir, Job::Forget).await.unwrap(),
            Some(1),
            "one picture came off the network"
        );

        let done = run(&pool, &data_dir, Job::Forget).await.unwrap();
        assert_eq!(done.affected, 1);

        assert!(!bought.exists(), "the fetched one goes");
        assert!(!half.exists(), "and so does what a dead download left");
        assert!(theirs.exists(), "and their own picture is untouched");

        let left: Vec<String> = sqlx::query_scalar("SELECT source FROM artworks")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left, vec!["local_file".to_string()]);

        // The artist it belonged to is without a picture rather than pointing at
        // a row that is gone, and can be looked up again — which is the whole
        // point of pressing this.
        let pointing: Vec<Option<i64>> =
            sqlx::query_scalar("SELECT artwork_id FROM artists ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(pointing, vec![None, Some(2)]);

        let remembered: i64 = sqlx::query_scalar("SELECT count(*) FROM artwork_lookups")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remembered, 0, "or it could not be looked up again");
    }

    /// What came off the network is not in this job's reach, in either half.
    ///
    /// The sweep works by hash over a directory, which cannot tell a portrait
    /// that cost two requests at one a second from a file nothing wants — so the
    /// fetched ones are in a directory of their own and this never walks it. And
    /// the memory of having asked and been told no stays too: throwing it away
    /// would mean walking to somebody else's server again for the same no.
    #[tokio::test]
    async fn what_the_network_cost_is_out_of_this_jobs_reach() {
        let pool = empty().await;
        let data_dir = tempdir();

        // Nothing names either file, which is the whole of what this job goes on.
        let cached = cached(&data_dir, "aabbcc");
        let fetched = crate::artwork::acquired_path(&data_dir, "ddeeff");
        std::fs::create_dir_all(fetched.parent().unwrap()).unwrap();
        std::fs::write(&fetched, b"a portrait that cost a walk").unwrap();

        for (source, found) in [("local", 0), ("commons", 0)] {
            sqlx::query(
                "INSERT INTO artwork_lookups (entity_type, entity_id, source, attempted_at, found)
                 VALUES ('artist', 1, ?, ?, ?)",
            )
            .bind(source)
            .bind(db::now())
            .bind(found)
            .execute(&pool)
            .await
            .unwrap();
        }

        // One cached file to delete, and one local answer of "nothing here" to
        // check again. The fetched pair is in neither figure, because it is not
        // on offer to be touched at all.
        assert_eq!(
            pending(&pool, &data_dir, Job::Covers).await.unwrap(),
            Some(1)
        );
        assert_eq!(revisiting(&pool, Job::Covers).await.unwrap(), Some(1));

        let done = run(&pool, &data_dir, Job::Covers).await.unwrap();

        assert_eq!(done.affected, 2);
        assert!(!cached.exists(), "the cheap one goes");
        assert!(fetched.exists(), "and the one that cost a walk stays");

        let left: Vec<String> =
            sqlx::query_scalar("SELECT source FROM artwork_lookups ORDER BY source")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(left, vec!["commons".to_string()]);
    }

    /// The other half of the same cache: what was remembered about finding no
    /// cover at all. Only the answers that found nothing are forgotten, since
    /// forgetting the others would mean opening those files again for no reason.
    #[tokio::test]
    async fn the_covers_that_were_not_found_are_forgotten_too() {
        let pool = empty().await;
        let data_dir = tempdir();

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

        // Counted apart from the files, and not as rubbish: these are true
        // answers that have had time to go stale, and there is nothing here for
        // the sweep to take away.
        assert_eq!(
            pending(&pool, &data_dir, Job::Covers).await.unwrap(),
            Some(0),
            "nothing in the cache is going anywhere"
        );
        assert_eq!(
            revisiting(&pool, Job::Covers).await.unwrap(),
            Some(2),
            "and two places are worth looking in again"
        );

        let done = run(&pool, &data_dir, Job::Covers).await.unwrap();

        assert_eq!(done.affected, 2);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM artwork_lookups")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 1, "the one that found a cover stays");
    }

    /// Nothing cached yet is not a problem to report: it is a cache with nothing
    /// in it, which is exactly as clean as one can be.
    #[tokio::test]
    async fn an_empty_cache_is_nothing_to_clean() {
        let pool = empty().await;
        let data_dir = tempdir();

        assert_eq!(
            pending(&pool, &data_dir, Job::Covers).await.unwrap(),
            Some(0)
        );
    }

    /// The pages a delete leaves behind are the whole point of the job, so the
    /// figure it offers beforehand and the one it reports after both have to be
    /// real rather than always zero.
    #[tokio::test]
    async fn compacting_gives_back_what_deleting_left() {
        let pool = empty().await;
        let data_dir = tempdir();

        let at = db::now();
        for id in 0..2_000 {
            sqlx::query(
                "INSERT INTO artwork_lookups (entity_type, entity_id, source, attempted_at, found)
                 VALUES ('album', ?, 'local', ?, 0)",
            )
            .bind(id)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("DELETE FROM artwork_lookups")
            .execute(&pool)
            .await
            .unwrap();

        let free = pending(&pool, &data_dir, Job::Compact).await.unwrap();
        assert!(free > Some(0), "the pages are still held: {free:?}");

        let done = run(&pool, &data_dir, Job::Compact).await.unwrap();

        assert_eq!(done.error, None);
        assert!(done.affected > 0, "nothing was given back");
        assert_eq!(
            pending(&pool, &data_dir, Job::Compact).await.unwrap(),
            Some(0),
            "and there is nothing left to give back"
        );
    }

    /// A database nobody has broken says so, and says it as a count rather than
    /// as an absence.
    #[tokio::test]
    async fn a_sound_database_reports_no_problems() {
        let pool = empty().await;
        let data_dir = tempdir();

        assert_eq!(pending(&pool, &data_dir, Job::Check).await.unwrap(), None);

        let done = run(&pool, &data_dir, Job::Check).await.unwrap();

        assert_eq!(done.affected, 0);
        assert_eq!(done.error, None);
    }

    /// Two runs of the same job leave two rows, and the newer one is what a
    /// screen asking "when did this last run" gets.
    ///
    /// **This is the one that hangs the CI**, every time it has happened: "has been
    /// running for over 60 seconds" on a test that takes 0.15 seconds here, including
    /// starting cargo. It has not been reproduced — eleven runs of the module, three of
    /// the whole binary at two threads — so what is written here is what was found and
    /// what was done about it, for whoever meets it next.
    ///
    /// Two things about it were wrong on their own terms and are fixed. It wrote to the
    /// pool rather than in its turn, which is the one thing this program does not do
    /// anywhere else — the queue in `db` exists precisely so that two ways into one
    /// database do not race for the lock — and it went unseen because the test that
    /// guards against that deliberately exempts test code. And it never closed the pool,
    /// so the runtime was left to shut down over connections still being handed back;
    /// SQLite statements run on blocking threads, and a `Runtime` drop waits for those.
    ///
    /// The steps are timed out so that if it happens again the CI says which one it hung
    /// on instead of dying quietly at sixty seconds. Fifteen seconds is far longer than
    /// any step here needs and shorter than the aggregate wait that produced the message.
    #[tokio::test]
    async fn the_last_run_of_a_job_is_the_last_one() {
        use std::time::Duration;
        use tokio::time::timeout;

        /// Long enough that a slow machine is not a failure, short enough to fail before
        /// the runner gives up on the whole binary.
        const PATIENCE: Duration = Duration::from_secs(15);

        let pool = empty().await;
        let data_dir = tempdir();

        timeout(PATIENCE, run(&pool, &data_dir, Job::Check))
            .await
            .expect("the first run hung")
            .unwrap();

        // Timestamps are to the second, so two rows written in the same second
        // would be indistinguishable. Aged by hand, which is also the case worth
        // testing: a run from years ago and one from now.
        let long_ago = "2020-01-01T00:00:00Z";
        timeout(
            PATIENCE,
            sqlx::query("UPDATE job_runs SET started_at = ?")
                .bind(long_ago)
                .in_turn(&pool),
        )
        .await
        .expect("ageing the first run hung")
        .unwrap();

        let second = timeout(PATIENCE, run(&pool, &data_dir, Job::Check))
            .await
            .expect("the second run hung")
            .unwrap();

        let latest = latest(&pool).await.unwrap();
        assert_eq!(latest.len(), 1, "one row per job");
        assert_eq!(latest[0].at, second.at, "the newer one answers for the job");

        let all = history(&pool, 10).await.unwrap();
        assert_eq!(all.len(), 2, "both are kept");
        assert_eq!(all[1].at, long_ago, "oldest last");

        // Handed back rather than left to the runtime's own shutdown.
        pool.close().await;
    }

    /// A directory of its own per test, since these ones write files.
    fn tempdir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("tocata-jobs-{}", db::public_id().unwrap()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
