// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{
    SqliteArguments, SqliteConnectOptions, SqliteJournalMode, SqliteQueryResult, SqliteSynchronous,
};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::warn;

/// How long a writer waits for the lock before giving up.
///
/// Fifteen seconds, up from five, and the five was chosen before anybody had run a
/// scan of eleven thousand files: the scan held one transaction for its whole length
/// and every other write in the server timed out against it. That is fixed where it
/// belongs — the scan commits in batches now — and this is the net under it.
///
/// A long timeout costs nothing when nothing is contended, and what it buys is the
/// difference between a request that takes a moment and a request that fails. SQLite
/// hands the lock to whoever asks rather than to whoever has waited longest, so a
/// write that is unlucky several times running needs room to be unlucky in.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(15);

/// Begins a transaction that is going to write.
///
/// `BEGIN IMMEDIATE` rather than the plain `BEGIN` sqlx issues, and this is not
/// tuning. A deferred transaction takes a read lock and asks for the write lock
/// only when it first writes — and if another writer has committed in between,
/// SQLite refuses *immediately* with "database is locked" rather than waiting,
/// because waiting there could deadlock two transactions that each hold a read
/// lock and each want to write.
///
/// Which means [`BUSY_TIMEOUT`] does not cover it. That timeout is what a writer
/// waits with, and this is the one case where there is no waiting to be done: the
/// answer arrives at once and it is a failure. An immediate transaction asks for
/// the write lock up front, where the timeout applies again.
///
/// It was found through nine album covers asked for at once on a cold cache —
/// which is what a shelf of records does the first time it is opened. Seven came
/// back and two came back as 500s, and reloading fixed it, because by then seven
/// of them no longer needed writing. Every upsert in this program has the same
/// shape and so had the same fault waiting: a play counted while a scan writes, a
/// track starred, a bookmark moved.
pub async fn writing(pool: &SqlitePool) -> sqlx::Result<Writing> {
    let turn = turn(pool).await;
    let tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    Ok(Writing { tx, _turn: turn })
}

/// The one writer at a time, and a fair queue for it.
///
/// SQLite has a single write lock and hands it to whoever asks rather than to
/// whoever has waited longest, so two writers in this process do not take turns —
/// they race, and the same one can lose several times running. Measured at 2.2
/// seconds of losing before the scan started standing back between batches, and
/// still 0.8 after. Nothing failed, because [`BUSY_TIMEOUT`] is long, but nothing
/// bounded the wait either: the program was correct by luck.
///
/// A semaphore of one permit is what bounds it. Tokio's is FIFO, which is the
/// whole point — the wait becomes at most one other write rather than however many
/// times in a row the lock happens to go elsewhere, and the entire class of
/// "database is locked" cannot arise from inside this process.
///
/// It does nothing about another process writing to the same file, which is what
/// the busy timeout is still there for.
///
/// **One per database, and not one for the program.** The lock this stands in for
/// belongs to a file, so two pools onto two databases have no reason to wait for
/// each other — and made to, they do worse than wait: the test suite opens hundreds
/// of databases in one process, and a single global permit had them queueing behind
/// whichever one was slowest, turning four seconds into thirty-six and hanging
/// outright about one run in three.
///
/// **Keyed by the file, because the lock belongs to the file.** Two pools onto one
/// database are two ways to the same write lock and have every reason to take
/// turns; two pools onto two databases have none. A database held in memory has a
/// name of its own here too — sqlx numbers them — so the tests get a queue each
/// without asking for one.
///
/// It was keyed by the address of the pool's options before, which looked like an
/// identity and is not one: an address only names what lives there. Sixty pools
/// opened and closed in turn all landed on the same address, every time and not
/// now and then, so a database inherited the queue of one that no longer existed
/// along with whatever that queue was in the middle of. A name is a name whether
/// or not anybody is holding it.
static WRITING: Mutex<BTreeMap<PathBuf, Arc<Semaphore>>> = Mutex::new(BTreeMap::new());

/// The queue for one database, made the first time it is written to.
fn queue_for(pool: &SqlitePool) -> Arc<Semaphore> {
    let name = pool.connect_options().get_filename().to_path_buf();

    WRITING
        .lock()
        .expect("the write queue registry is never poisoned")
        .entry(name)
        .or_insert_with(|| Arc::new(Semaphore::new(1)))
        .clone()
}

/// How long a write may wait for its turn before the wait is worth mentioning.
///
/// Not a limit: the wait is unbounded on purpose, because failing here is the very
/// thing this exists to prevent. But a wait this long means a transaction is being
/// held far longer than any of them should be, and that is worth finding in a log
/// rather than by watching a request hang.
const WAITED_TOO_LONG: Duration = Duration::from_secs(5);

/// One statement that writes, run in its turn.
///
/// For the writes that are a single statement and want no transaction around them,
/// which is most of them: a session recorded, a key revoked, a preference stored.
/// Written `.in_turn(pool)` where it would otherwise say `.execute(pool)`, and the
/// difference is the whole of what this module is for.
///
/// The turn is taken inside, so it lasts exactly as long as the statement. That is
/// not tidiness: a permit held across a call to something that writes on its own
/// would wait for a turn that only it could give up, and this shape makes that
/// impossible to write by accident.
pub trait InTurn {
    /// What the statement answers with: how many rows it touched, or the one value
    /// it was written to return.
    type Out;

    fn in_turn(
        self,
        pool: &SqlitePool,
    ) -> impl std::future::Future<Output = sqlx::Result<Self::Out>> + Send;
}

impl<'q> InTurn for sqlx::query::Query<'q, sqlx::Sqlite, SqliteArguments> {
    type Out = SqliteQueryResult;

    async fn in_turn(self, pool: &SqlitePool) -> sqlx::Result<SqliteQueryResult> {
        let _turn = turn(pool).await;
        self.execute(pool).await
    }
}

/// The same for a statement that ends in `RETURNING`, which is how a row that
/// generates its own identifier says what it got.
impl<'q, O> InTurn for sqlx::query::QueryScalar<'q, sqlx::Sqlite, O, SqliteArguments>
where
    O: Send + Unpin,
    (O,): for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>,
{
    type Out = O;

    async fn in_turn(self, pool: &SqlitePool) -> sqlx::Result<O> {
        let _turn = turn(pool).await;
        self.fetch_one(pool).await
    }
}

/// Waits for the turn to write.
///
/// Held for as long as the returned guard lives. Everything in here that takes one
/// gives it up on the same statement or with the transaction it rode in on; taking
/// one by hand means taking on that rule as well.
async fn turn(pool: &SqlitePool) -> OwnedSemaphorePermit {
    let queue = queue_for(pool);
    let waiting = Instant::now();

    let turn = match tokio::time::timeout(WAITED_TOO_LONG, queue.clone().acquire_owned()).await {
        Ok(turn) => turn,
        Err(_) => {
            warn!(
                "a write has been waiting {WAITED_TOO_LONG:?} for its turn: something is \
                 holding a write transaction open"
            );
            queue.acquire_owned().await
        }
    };

    // The semaphore is never closed, so there is no failure to handle: the only
    // way `acquire` returns an error is a `close()` that nothing here calls.
    let turn = turn.expect("a write queue is never closed");

    let waited = waiting.elapsed();
    if waited > WAITED_TOO_LONG {
        warn!("a write waited {waited:?} for its turn");
    }

    turn
}

/// A transaction that is going to write, holding the turn to do it.
///
/// The permit lives in here rather than beside it so that it cannot be dropped
/// early or forgotten: the turn ends when the transaction does, whether that is a
/// commit, a rollback or a panic on the way to either.
pub struct Writing {
    tx: sqlx::Transaction<'static, sqlx::Sqlite>,
    _turn: OwnedSemaphorePermit,
}

impl Writing {
    pub async fn commit(self) -> sqlx::Result<()> {
        self.tx.commit().await
    }
}

/// Dereferences to the transaction, so everything that takes one — including every
/// function here that is handed a `&mut Transaction` — is unchanged by the permit
/// riding along with it.
impl Deref for Writing {
    type Target = sqlx::Transaction<'static, sqlx::Sqlite>;

    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}

impl DerefMut for Writing {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tx
    }
}

/// Current time in the shape the schema stores: ISO-8601, UTC, to the second.
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A moment either side of now, in the same shape. Negative offsets go back:
/// what a session's expiry is made of, and what "older than five minutes" is
/// compared against.
pub fn from_now(offset: chrono::Duration) -> String {
    (Utc::now() + offset).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A moment written by somebody else, in the shape the schema stores, or `None`
/// if it is not a moment at all.
///
/// Normalising to UTC is the whole job. Timestamps here are compared as text, so
/// `2026-08-26T11:00:00+02:00` would sort as though it were nine in the morning
/// UTC rather than the same instant as `09:00:00Z`, and a date would take effect
/// two hours late or early depending on which way the offset went.
pub fn timestamp_from(given: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(given)
        .ok()
        .map(|moment| moment.to_utc().to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// A moment given in milliseconds since the epoch, in the shape the schema
/// stores. Clients hand these over when scrobbling plays they cached offline.
pub fn from_epoch_millis(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Bytes behind a public identifier. Sixty-four bits of randomness: short
/// enough to live in a URL, wide enough that a library would need billions of
/// rows before a collision became thinkable, and the UNIQUE constraint catches
/// it if one ever does.
const PUBLIC_ID_BYTES: usize = 8;

/// Mints the opaque identifier clients see. Not derived from anything: a track
/// keeps it across renames and retags, which is what stops a corrected tag
/// from orphaning somebody's favourites.
pub fn public_id() -> Result<String> {
    let mut bytes = [0u8; PUBLIC_ID_BYTES];
    getrandom::fill(&mut bytes).context("reading from the system RNG")?;

    Ok(bytes.iter().fold(String::new(), |mut out, b| {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
        out
    }))
}

/// Opens the database, creating it if needed, and brings the schema up to
/// date. Migrations are embedded at compile time, so the binary carries its
/// own schema and needs nothing alongside it.
pub async fn connect(path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        // SQLite ships with foreign key enforcement disabled, which would
        // turn every reference in the schema into a comment.
        .foreign_keys(true)
        // Readers do not block the writer, which matters while a scan is
        // running and clients keep browsing.
        .journal_mode(SqliteJournalMode::Wal)
        // The log is flushed at checkpoints rather than on every commit, which
        // is the setting written for the mode above.
        //
        // SQLite defaults to FULL, meaning an fsync per transaction, and that is
        // the right default for a database whose every row is somebody's money.
        // Ours holds what was read off files that are still there to be read
        // again: a star, a play counted, a note that an album carries no cover.
        // Paid for at FULL, one such note cost 1.2 seconds on an old disk busy
        // reading music — for a write of one row.
        //
        // What NORMAL gives up is narrow and worth naming exactly. The database
        // is never corrupted; that guarantee belongs to the log, not to the
        // flush. Nothing is lost if this program crashes or is stopped, because
        // the writing already reached the system. Only the machine losing power
        // or the kernel going down can take the last commits with it, and what
        // they would take is a handful of seconds of the small things above.
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT);

    let pool = SqlitePool::connect_with(options)
        .await
        .with_context(|| format!("opening database at {}", path.display()))?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("applying database migrations")?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One queue per database: shared by two ways into the same one, and never
    /// shared between two different ones.
    ///
    /// Both halves matter and they pull opposite ways. Miss the first and two pools
    /// onto one file race for a lock neither knows the other wants, which is the
    /// whole thing this queue exists to prevent. Miss the second and every database
    /// in the process waits behind whichever is slowest — measured once at four
    /// seconds becoming thirty-six, with about one run in three not finishing.
    #[tokio::test]
    async fn a_write_queue_belongs_to_a_database_and_is_shared_by_every_way_into_it() {
        let directory = std::env::temp_dir().join("tocata-one-queue");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("tocata.db");

        let one = connect(&file).await.unwrap();
        let another = connect(&file).await.unwrap();

        assert!(
            Arc::ptr_eq(&queue_for(&one), &queue_for(&another)),
            "two pools onto one file are two ways to one write lock"
        );

        let elsewhere = connect(&directory.join("other.db")).await.unwrap();
        assert!(
            !Arc::ptr_eq(&queue_for(&one), &queue_for(&elsewhere)),
            "and a different file is a different lock, with nothing to wait for"
        );

        // The databases the tests use are held in memory, and sqlx gives each one a
        // name of its own — so they get a queue each without anybody arranging it.
        let hers = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let his = SqlitePool::connect("sqlite::memory:").await.unwrap();
        assert!(!Arc::ptr_eq(&queue_for(&hers), &queue_for(&his)));
    }

    /// The orders a listing is asked for are orders the schema can read in, and
    /// the question every listing asks of every record is one it can answer
    /// without opening the record.
    ///
    /// Nothing in the program names these indexes, which is what makes them easy
    /// to tidy away as unused, and losing one is silent: every answer stays
    /// right and a page of twenty goes from twenty rows read to the whole
    /// catalogue read, sorted into a temporary table and thrown away. On a slow
    /// machine that is the difference between an eyeblink and two seconds, and
    /// it was measured at four hundred and thirty times the work.
    ///
    /// So what is checked is the plan rather than a duration, which would only
    /// say how fast this machine is today. "USE TEMP B-TREE" is SQLite saying it
    /// had to gather everything before it could give back the first row; a SCAN
    /// of a table is it saying it read the table through. Neither belongs in a
    /// listing that ends in LIMIT.
    /// How SQLite says it would answer, as one line. The plan comes back a row
    /// per step, and the step's own words are its last column.
    async fn explain(pool: &SqlitePool, statement: &'static str) -> String {
        let steps: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(statement).fetch_all(pool).await.unwrap();

        steps
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[tokio::test]
    async fn a_listing_is_read_in_order_rather_than_gathered_and_sorted() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        // How each kind of album list orders itself, in the words the statements
        // use — a different expression is a different index, and `coalesce` with
        // the collation spelled out is the one the alphabetical lists ask for.
        //
        // Written out whole rather than assembled around the ordering, because
        // sqlx takes SQL it can read at compile time and nothing else — which is
        // the rule that keeps a statement from ever being built out of anything
        // that came from outside.
        for (what, statement) in [
            (
                "newest",
                "EXPLAIN QUERY PLAN
                 SELECT id FROM albums ORDER BY created_at DESC LIMIT 20",
            ),
            (
                "by year",
                "EXPLAIN QUERY PLAN
                 SELECT id FROM albums ORDER BY year LIMIT 20",
            ),
            (
                "alphabetical",
                "EXPLAIN QUERY PLAN
                 SELECT id FROM albums
                  ORDER BY coalesce(sort_name, name) COLLATE NOCASE LIMIT 20",
            ),
        ] {
            let plan = explain(&pool, statement).await;

            assert!(
                !plan.contains("TEMP B-TREE"),
                "the {what} listing sorts the whole catalogue to hand back twenty: {plan}"
            );
        }

        // And whether a record has anything left to play, which is asked of every
        // record in every one of those listings.
        let plan = explain(
            &pool,
            "EXPLAIN QUERY PLAN
             SELECT 1 FROM tracks t
              WHERE t.album_id = 1 AND t.missing_since IS NULL AND t.library_id = 1",
        )
        .await;

        // Both columns inside the search is the whole point: the album narrows it
        // and the library settles it, without the track ever being opened. Found
        // by the album alone — which is what the older index offers — every
        // candidate has to be read to learn which library it is in, and that is
        // the walk this is here to keep out.
        assert!(
            plan.contains("tracks_present_idx (album_id=? AND library_id=?)"),
            "asking whether a record still has anything on it opens the tracks \
             themselves: {plan}"
        );

        // Picking songs at random has to consider every song that could be
        // picked — there is no ordering that lets it stop early — so the whole of
        // what it saves is not reading them. Library and year are what the
        // choosing asks about, and they are both in the index.
        let plan = explain(
            &pool,
            "EXPLAIN QUERY PLAN
             SELECT t.id FROM tracks t
              WHERE t.missing_since IS NULL AND t.library_id = 1 AND t.year >= 1990
              ORDER BY random() LIMIT 10",
        )
        .await;

        assert!(
            plan.contains("tracks_pick_idx (library_id=? AND year>?)"),
            "picking ten songs at random reads every song there is: {plan}"
        );
    }

    /// The two settings a database of ours is opened with, read back from the
    /// database itself.
    ///
    /// Both are deliberate and neither is visible in how the program behaves
    /// until the day it matters: turn the log off and readers start blocking on
    /// the writer, put the flush back on every commit and a write of one row
    /// costs an fsync. A pragma nobody reads back is a pragma that quietly
    /// returns to its default the next time these options are rewritten.
    ///
    /// Opened as a file rather than in memory on purpose. A database in memory
    /// has no log to keep and nothing to flush, so it answers whatever it likes
    /// and would agree with any expectation put to it.
    #[tokio::test]
    async fn a_database_is_opened_with_the_log_on_and_the_flush_off() {
        let directory = std::env::temp_dir().join("tocata-pragmas");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let pool = connect(&directory.join("tocata.db")).await.unwrap();

        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(journal, "wal", "readers must not block the writer");

        // 0 is OFF, 1 NORMAL, 2 FULL, 3 EXTRA.
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            synchronous, 1,
            "flushed at checkpoints, not once for every row written"
        );
    }

    /// The reason this function exists rather than the string being stored as
    /// given: text comparison is what decides whether a moment has passed, so an
    /// offset left in place would be read as though it were UTC and the moment
    /// would land hours away from the one that was meant.
    #[test]
    fn an_offset_becomes_the_same_instant_in_utc() {
        assert_eq!(
            timestamp_from("2026-08-26T11:00:00+02:00").unwrap(),
            "2026-08-26T09:00:00Z"
        );
    }

    #[test]
    fn what_is_already_utc_is_left_where_it_is() {
        assert_eq!(
            timestamp_from("2026-08-26T09:00:00Z").unwrap(),
            "2026-08-26T09:00:00Z"
        );
    }

    /// Sub-second precision is dropped rather than rejected: the schema keeps
    /// seconds, and refusing a moment for being too precise helps nobody.
    #[test]
    fn fractions_of_a_second_are_dropped() {
        assert_eq!(
            timestamp_from("2026-08-26T09:00:00.123456Z").unwrap(),
            "2026-08-26T09:00:00Z"
        );
    }

    #[test]
    fn what_is_not_a_moment_is_refused() {
        for given in ["", "tomorrow", "2026-08-26", "2026-13-01T00:00:00Z"] {
            assert!(timestamp_from(given).is_none(), "{given} is not a moment");
        }
    }

    /// One queue per database, asked for as many times as there are writes.
    ///
    /// The whole mechanism rests on this and would fail silently without it: a key
    /// that changed between calls would hand out a fresh permit every time, so every
    /// write would hold a queue of its own and none would ever wait — serialised in
    /// appearance and racing in fact, which is the state this module exists to leave.
    #[tokio::test]
    async fn a_database_has_one_queue_however_often_it_is_asked_for() {
        let one = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let other = SqlitePool::connect("sqlite::memory:").await.unwrap();

        assert!(
            Arc::ptr_eq(&queue_for(&one), &queue_for(&one)),
            "the same database, so the same queue"
        );
        assert!(
            !Arc::ptr_eq(&queue_for(&one), &queue_for(&other)),
            "and two databases wait for nobody but themselves"
        );

        // A clone of a pool is that pool, which is how it travels through the state.
        assert!(Arc::ptr_eq(&queue_for(&one), &queue_for(&one.clone())));
    }

    /// And the queue is a queue: the second writer waits for the first.
    #[tokio::test]
    async fn a_second_write_waits_for_the_first() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let queue = queue_for(&pool);

        let held = turn(&pool).await;
        assert_eq!(queue.available_permits(), 0, "somebody has the turn");

        // Nothing else can have it while that one lives.
        assert!(queue.clone().try_acquire_owned().is_err());

        drop(held);
        assert_eq!(queue.available_permits(), 1, "and giving it up frees it");
    }
}

/// The one thing a semaphore of one permit cannot do for itself: make sure
/// everything goes through it.
///
/// A write that reaches the pool directly waits on SQLite rather than on the queue,
/// which is exactly the race this module exists to remove — and one such write is
/// enough to bring the whole class of failure back. There is nothing in the type
/// system to prevent it, because `.execute(pool)` is what sqlx offers and it is
/// right for everything that reads. So the program reads itself instead.
#[cfg(test)]
mod every_write_takes_its_turn {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// How far past a statement to look for the call that runs it. Generous: the
    /// longest of them is a scan's track insert, with its conflict clause and its
    /// twenty binds.
    const REACH: usize = 4_000;

    fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir)
            .expect("reading the source tree")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, found);
            // `tests.rs` is a whole module of fixtures, which write to a database
            // of their own and have nobody to take turns with.
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|name| name != "tests.rs")
            {
                found.push(path);
            }
        }
    }

    /// Whether a position sits inside a `#[cfg(test)]` module.
    ///
    /// Decided by the last item that starts at the left margin before it: in this
    /// program every module and every function does, so the nearest one going
    /// backwards is the one a position belongs to. Counting braces would have to
    /// know which of them are inside a SQL string, and the SQL here is full of them.
    fn under_cfg_test(text: &str, at: usize) -> bool {
        let before = &text[..at];
        let Some(start) = before
            .match_indices("\nmod ")
            .chain(before.match_indices("\npub mod "))
            .chain(before.match_indices("\nfn "))
            .chain(before.match_indices("\npub fn "))
            .chain(before.match_indices("\nasync fn "))
            .chain(before.match_indices("\npub async fn "))
            .chain(before.match_indices("\nimpl "))
            .chain(before.match_indices("\npub trait "))
            .map(|(i, _)| i)
            .max()
        else {
            return false;
        };

        // The attribute sits on the line above whatever it applies to.
        text[..start].trim_end().ends_with("#[cfg(test)]")
    }

    #[test]
    fn nothing_writes_to_the_pool_behind_the_queue() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        assert!(!files.is_empty(), "no sources found under {root:?}");

        let mut loose = Vec::new();

        for path in files {
            let text = fs::read_to_string(&path).expect("reading a source file");

            for statement in ["INSERT INTO", "INSERT OR", "UPDATE ", "DELETE FROM"] {
                for (at, _) in text.match_indices(statement) {
                    if under_cfg_test(&text, at) {
                        continue;
                    }

                    let ahead = &text[at..text.len().min(at + REACH)];
                    let Some(call) = ["in_turn(", "execute(", "fetch_one(", "fetch_optional("]
                        .iter()
                        .filter_map(|call| ahead.find(call).map(|i| (i, *call)))
                        .min()
                    else {
                        continue;
                    };

                    let (i, call) = call;
                    if call == "in_turn(" {
                        continue;
                    }

                    // Against a transaction, which carries the turn it was opened
                    // with. Only the pool is a way round the queue.
                    let executor: String = ahead[i + call.len()..]
                        .chars()
                        .take_while(|c| *c != ')')
                        .collect();
                    if !executor.contains("pool") {
                        continue;
                    }

                    let line = text[..at].matches('\n').count() + 1;
                    let name = path.strip_prefix(&root).unwrap_or(&path).display();
                    loose.push(format!(
                        "{name}:{line} runs a write with .{call}{executor})"
                    ));
                }
            }
        }

        assert!(
            loose.is_empty(),
            "these writes go straight to the pool instead of taking their turn — \
             use `.in_turn(pool)`, or open a transaction with `db::writing`:\n  {}",
            loose.join("\n  ")
        );
    }
}
