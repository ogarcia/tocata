// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Pictures of the people, fetched rather than read.
//!
//! A cover is in the music. Nearly every album has one inside its files or
//! beside them, which is why finding one costs an open and nothing else. A
//! photograph of the band is in neither place: tags carry one so rarely that a
//! collection of nine hundred artists turns up a handful, and the rest of the
//! shelf stands under a glyph. That is the hole this fills, and it is the only
//! one — nothing else here goes looking on the network for anything.
//!
//! **Two hops and a download, at one request a second.** MusicBrainz is asked
//! what an artist's relations say, and among them may be a picture on Wikimedia
//! Commons; Commons is asked where a copy of that file is and what using it asks
//! for; the copy is fetched. Nine hundred artists is three quarters of an hour,
//! which is why this is a thing that runs in the background with somewhere to
//! watch it and never something that happens while a panel waits for an image.
//!
//! **Only artists with an identifier.** The relation is on the MusicBrainz
//! artist, so an artist the scanner never saw an MBID for has no way in. Looking
//! one up by name is available and is not done: two bands share a name often
//! enough that it would put a stranger's face on somebody's shelf, and a missing
//! picture is a better answer than a wrong one.
//!
//! **What it finds is somebody else's work.** Commons is nearly all licensed
//! rather than free of conditions, and the conditions are almost always
//! attribution. So the author, the licence and the page it came off are stored
//! with the picture and drawn under it, and a fetch that cannot say those things
//! keeps nothing.

use crate::artwork;
use crate::db;
use crate::net::Net;
use anyhow::{Context, Result};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tracing::{debug, info, warn};

/// What `artwork_lookups.source` says for a walk out to Commons, which is what
/// keeps it apart from the row a look through the user's own files leaves.
pub const SOURCE: &str = "commons";

/// How long between requests.
///
/// MusicBrainz asks for one a second in writing and blocks whoever does not
/// listen. Commons is far more relaxed, and is asked at the same rate anyway:
/// the pace of this is set by the slowest thing in the walk, and a second is
/// nothing next to a job measured in three quarters of an hour.
pub const PACE: Duration = Duration::from_secs(1);

/// How long a "there is nothing here" is believed before it is worth asking
/// again.
///
/// Commons grows: a band with no photograph today may have one next year. But
/// the cost of asking is a walk to somebody else's server, so asking again every
/// week would be spending an hour a week to be told the same thing. Three months
/// is slow enough to be polite and quick enough that somebody who uploads a
/// picture of their own band sees it here within a season.
pub const PATIENCE_DAYS: i64 = 90;

/// How wide a copy to ask for.
///
/// Not the original, which on Commons is regularly a twenty megapixel photograph
/// and would be tens of megabytes for a picture drawn at 56 pixels in a drawer
/// and a few hundred on a page. Commons scales on request, so asking for the
/// size actually wanted costs them one resize and us one download.
const WIDTH: u32 = 640;

/// The relation that means "this is a picture of them".
const IMAGE: &str = "image";

/// Where a Commons file's own page lives, which is how a relation names one.
const FILE_PAGE: &str = "https://commons.wikimedia.org/wiki/";

/// What a service says when it wants the caller to slow down rather than to go
/// away.
///
/// MusicBrainz answers 503 for it and Wikimedia answers 429, and neither means
/// the service is broken: both mean "not yet". Treating either as a failure
/// throws away the rest of a walk that is three quarters of an hour long,
/// because of one answer that asked for a pause.
const NOT_YET: [u16; 2] = [429, 503];

/// How long to wait when one of them says that and does not say how long.
///
/// Both publish a limit of about one request a second, so a refusal means
/// something further upstream than our own pacing — a shared address, a busy
/// hour. Long enough to be a real pause rather than the same request again.
const BACK_OFF: Duration = Duration::from_secs(5);

/// Asks, and asks once more if the answer was "not yet".
///
/// Once. A second refusal is a service that is not going to answer this minute,
/// and the walk gives up rather than standing at the door: whatever is left
/// unasked is still wanting, and starting again picks it up where this stopped.
async fn patiently(net: &Net, url: &str, who: &'static str) -> Result<crate::net::Answer> {
    let asked = net
        .ask(url)
        .await
        .with_context(|| format!("asking {who}"))?;

    if !NOT_YET.contains(&asked.status) {
        return Ok(asked);
    }

    let wait = asked
        .seconds("retry-after")
        .map(Duration::from_secs)
        .unwrap_or(BACK_OFF);

    debug!("{who} answered {}; waiting {:?}", asked.status, wait);
    tokio::time::sleep(wait).await;

    net.ask(url).await.with_context(|| format!("asking {who}"))
}

/// Everything a fetched portrait carries: the bytes, and what using them asks
/// for.
pub struct Portrait {
    pub bytes: Vec<u8>,
    /// The file's name on Commons, as `File:Something.jpg`.
    pub file: String,
    pub author: Option<String>,
    pub license: Option<String>,
    pub license_url: Option<String>,
    /// The file's page, which is where an attribution is supposed to point.
    pub page: Option<String>,
}

/// Looks one artist up, all the way to the bytes.
///
/// `None` means the walk finished and there was nothing there, which is an
/// answer worth remembering. An error means the walk did not finish, which is
/// not: a service that was down for an hour has not said anything about this
/// artist.
pub async fn look_up(net: &Net, mbid: &str) -> Result<Option<Portrait>> {
    let asked = patiently(net, &relations_url(mbid), "MusicBrainz").await?;

    if !asked.ok() {
        anyhow::bail!("MusicBrainz answered {}: {}", asked.status, said(&asked));
    }

    let Some(file) = pictured_at(&asked.body()) else {
        return Ok(None);
    };

    tokio::time::sleep(PACE).await;

    let asked = patiently(net, &commons_url(&file), "Commons").await?;

    if !asked.ok() {
        anyhow::bail!("Commons answered {}: {}", asked.status, said(&asked));
    }

    let Some(told) = told_about(&asked.body()) else {
        return Ok(None);
    };

    // Nothing is kept that cannot be credited. Commons is other people's work
    // under terms that nearly always ask for a name, and a picture on screen
    // with nothing under it is the one outcome this must not produce.
    let (Some(license), Some(page)) = (told.license.clone(), told.page.clone()) else {
        return Ok(None);
    };

    tokio::time::sleep(PACE).await;

    let fetched = net
        .fetch(&told.url)
        .await
        .context("fetching a picture from Commons")?;

    // The same patience as the two questions, and it is needed in the same
    // place: the file itself comes off a different host with its own limit.
    let fetched = if NOT_YET.contains(&fetched.status) {
        tokio::time::sleep(
            fetched
                .seconds("retry-after")
                .map(Duration::from_secs)
                .unwrap_or(BACK_OFF),
        )
        .await;

        net.fetch(&told.url)
            .await
            .context("fetching a picture from Commons")?
    } else {
        fetched
    };

    if !fetched.ok() {
        anyhow::bail!("fetching a picture answered {}", fetched.status);
    }

    let bytes = fetched.bytes;

    // Trusted from the bytes rather than from the name or the header, like every
    // other image here.
    if artwork::mime_of(&bytes).is_none() {
        return Ok(None);
    }

    Ok(Some(Portrait {
        bytes,
        file,
        author: told.author,
        license: Some(license),
        license_url: told.license_url,
        page: Some(page),
    }))
}

/// Writes one down: the bytes where a sweep cannot reach them, the row with what
/// the licence asks for, and the artist pointed at it.
///
/// Leaves an artist that already has a picture alone. A photograph somebody put
/// beside their own music beats one this went looking for, and by the time this
/// writes, the scan or a request may have found one.
pub async fn keep(
    pool: &SqlitePool,
    data_dir: &Path,
    artist_id: i64,
    portrait: &Portrait,
) -> Result<()> {
    let hash = artwork::acquire(data_dir, &portrait.bytes)?;
    let mime_type = artwork::mime_of(&portrait.bytes).context("it was checked to be an image")?;
    let at = db::now();

    let mut tx = db::writing(pool).await?;

    // The hash is the identity, as everywhere else: two artists Commons gives
    // the same photograph share the row.
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM artworks WHERE content_hash = ? LIMIT 1")
            .bind(&hash)
            .fetch_optional(&mut **tx)
            .await?;

    let artwork_id = match existing {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "INSERT INTO artworks (
                     public_id, kind, source, source_ref, mime_type, content_hash, fetched_at,
                     author, license, license_url, source_url
                 ) VALUES (?, 'artist', ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 RETURNING id",
            )
            .bind(db::public_id()?)
            .bind(artwork::FROM_COMMONS)
            .bind(&portrait.file)
            .bind(mime_type)
            .bind(&hash)
            .bind(&at)
            .bind(&portrait.author)
            .bind(&portrait.license)
            .bind(&portrait.license_url)
            .bind(&portrait.page)
            .fetch_one(&mut **tx)
            .await?
        }
    };

    sqlx::query("UPDATE artists SET artwork_id = ? WHERE id = ? AND artwork_id IS NULL")
        .bind(artwork_id)
        .bind(artist_id)
        .execute(&mut **tx)
        .await?;

    remember(&mut tx, artist_id, true, &at).await?;

    tx.commit().await?;

    Ok(())
}

/// Writes down that this one was asked about and there was nothing, so the next
/// pass walks past it rather than out to two servers again.
pub async fn remember_nothing(pool: &SqlitePool, artist_id: i64) -> Result<()> {
    let at = db::now();
    let mut tx = db::writing(pool).await?;
    remember(&mut tx, artist_id, false, &at).await?;
    tx.commit().await?;

    Ok(())
}

async fn remember(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    artist_id: i64,
    found: bool,
    at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO artwork_lookups (entity_type, entity_id, source, attempted_at, found)
              VALUES ('artist', ?, ?, ?, ?)
         ON CONFLICT (entity_type, entity_id, source) DO UPDATE SET
             attempted_at = excluded.attempted_at,
             found = excluded.found",
    )
    .bind(artist_id)
    .bind(SOURCE)
    .bind(at)
    .bind(found)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// One artist worth walking out for.
pub struct Wanted {
    pub id: i64,
    pub name: String,
    pub mbid: String,
}

/// Who is still without a picture, has an identifier to look one up by, and has
/// not been asked about lately.
///
/// Ordered by name so two runs walk the same shelf in the same order, which is
/// what makes stopping one half way and starting it again pick up where it left
/// off rather than somewhere arbitrary.
pub async fn wanting(pool: &SqlitePool) -> Result<Vec<Wanted>> {
    let stale = db::from_now(-chrono::Duration::days(PATIENCE_DAYS));

    sqlx::query_as(
        "SELECT a.id, a.name, a.mbid
           FROM artists a
          WHERE a.artwork_id IS NULL
            AND a.mbid IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM artwork_lookups l
                 WHERE l.entity_type = 'artist' AND l.entity_id = a.id
                   AND l.source = ? AND l.attempted_at > ?)
          ORDER BY coalesce(a.sort_name, a.name) COLLATE NOCASE",
    )
    .bind(SOURCE)
    .bind(&stale)
    .fetch_all(pool)
    .await
    .context("reading who is still without a picture")
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Wanted {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;

        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            mbid: row.try_get("mbid")?,
        })
    }
}

/// The first line of what a refusal said, for a log that has to be enough to act
/// on. These answer with prose or with JSON; either way the first line names the
/// reason, and the rest is a page of HTML nobody wants in a log.
fn said(answer: &crate::net::Answer) -> String {
    answer
        .body()
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

fn relations_url(mbid: &str) -> String {
    format!("https://musicbrainz.org/ws/2/artist/{mbid}?inc=url-rels&fmt=json")
}

fn commons_url(file: &str) -> String {
    let titles = utf8_percent_encode(file, NON_ALPHANUMERIC);

    format!(
        "https://commons.wikimedia.org/w/api.php\
         ?action=query&prop=imageinfo&iiprop=url%7Cextmetadata\
         &iiurlwidth={WIDTH}&format=json&titles={titles}"
    )
}

/// The Commons file an artist's relations point at.
///
/// One relation of the several kinds MusicBrainz keeps, and the only one that is
/// a photograph of them: everything else there is a homepage, a shop, a social
/// account. A relation pointing at Commons but not at a file — a category, a
/// gallery — is not one picture and is passed over rather than guessed at.
fn pictured_at(json: &str) -> Option<String> {
    let read: Value = serde_json::from_str(json).ok()?;

    read.get("relations")?
        .as_array()?
        .iter()
        .filter(|relation| relation.get("type").and_then(Value::as_str) == Some(IMAGE))
        .find_map(|relation| {
            let resource = relation.get("url")?.get("resource")?.as_str()?;
            let page = resource.strip_prefix(FILE_PAGE)?;

            page.starts_with("File:")
                .then(|| percent_decode_str(page).decode_utf8_lossy().into_owned())
        })
}

/// What Commons says about one file.
struct Told {
    url: String,
    author: Option<String>,
    license: Option<String>,
    license_url: Option<String>,
    page: Option<String>,
}

/// Reads that answer: where a copy the size we asked for is, and what using it
/// asks of us.
///
/// The pages come back under their own identifiers, and a title Commons does not
/// have comes back as page `-1` with no `imageinfo` at all — which falls out of
/// this as nothing found, without a case of its own.
fn told_about(json: &str) -> Option<Told> {
    let read: Value = serde_json::from_str(json).ok()?;
    let pages = read.get("query")?.get("pages")?.as_object()?;
    let info = pages
        .values()
        .find_map(|page| page.get("imageinfo")?.as_array()?.first())?;

    // The scaled copy where there is one. A file smaller than what was asked for
    // is not scaled up and has no thumbnail, and then the original is the right
    // answer and is already small.
    let url = info
        .get("thumburl")
        .or_else(|| info.get("url"))?
        .as_str()?
        .to_string();

    let meta = info.get("extmetadata");
    let said = |name: &str| -> Option<String> {
        let value = meta?.get(name)?.get("value")?.as_str()?;
        let plain = plain(value);

        (!plain.is_empty()).then_some(plain)
    };

    Some(Told {
        url,
        author: said("Artist"),
        license: said("LicenseShortName"),
        license_url: said("LicenseUrl"),
        page: info
            .get("descriptionurl")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The words out of a scrap of HTML.
///
/// Commons answers some of these fields with markup — an author is usually a
/// link to their user page — and what goes under a picture is a name rather than
/// an anchor. Not a parser and not trying to be: everything between angle
/// brackets goes, the handful of entities that survive that are spelled back
/// out, and anything stranger than that ends up as text, which is the safe way
/// to be wrong.
fn plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut inside = false;

    for character in html.chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(character),
            _ => {}
        }
    }

    out = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        // Last, so a doubly escaped ampersand does not become the start of
        // another entity halfway through this.
        .replace("&amp;", "&");

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Everything known about the walk in flight, or about the last one to run.
///
/// One value rather than a stream of changes, for the reason the scanner's is:
/// whichever one a watcher reads is complete on its own.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub fetching: bool,
    /// Who it was asking about when this was taken, while it is asking.
    pub artist: Option<String>,
    /// How many it has been through, and how many there were to go through.
    pub done: u64,
    pub total: u64,
    /// How many of those came back with a picture. The rest are artists nobody
    /// has photographed, which is most of them.
    pub found: u64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Set when the last walk was told to stop rather than finishing.
    pub cancelled: bool,
    /// Why it stopped early, where something other than a person stopped it.
    pub failure: Option<String>,
}

/// The parts that need a lock, which change once an artist rather than once a
/// counter.
#[derive(Debug, Default)]
struct Current {
    artist: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    cancelled: bool,
    failure: Option<String>,
}

/// Live progress of a walk, and what keeps two of them from running at once.
#[derive(Debug, Default)]
pub struct Fetching {
    running: AtomicBool,
    cancel: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    found: AtomicU64,
    current: RwLock<Current>,
}

impl Fetching {
    pub fn is_fetching(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Snapshot {
        let current = self.current.read().unwrap_or_else(|e| e.into_inner());

        Snapshot {
            fetching: self.is_fetching(),
            artist: current.artist.clone(),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            found: self.found.load(Ordering::Relaxed),
            started_at: current.started_at.clone(),
            finished_at: current.finished_at.clone(),
            cancelled: current.cancelled,
            failure: current.failure.clone(),
        }
    }

    /// Asks the walk in flight to give up, whether because somebody pressed a
    /// button or because the process is going away.
    ///
    /// It stops between artists rather than mid-request, which is at most one
    /// pace away: there is nothing here worth interrupting a fetch for, and an
    /// artist half looked up would leave nothing written either way.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    fn should_stop(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// Claims the right to walk. `None` means one is already going.
    fn begin(&self, total: u64) -> Option<Walking<'_>> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;

        self.done.store(0, Ordering::Relaxed);
        self.found.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.cancel.store(false, Ordering::Release);

        *self.current.write().unwrap_or_else(|e| e.into_inner()) = Current {
            started_at: Some(db::now()),
            ..Current::default()
        };

        Some(Walking { fetching: self })
    }

    fn looking_at(&self, name: &str) {
        self.current
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .artist = Some(name.to_string());
    }

    fn went(&self, found: bool) {
        self.done.fetch_add(1, Ordering::Relaxed);
        if found {
            self.found.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn gave_up(&self, why: String) {
        self.current
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .failure = Some(why);
    }
}

/// Says a walk is running for as long as this is held, and puts the flag back
/// however the walk ends — including by panicking, which is the whole reason
/// this is a guard rather than two calls.
struct Walking<'a> {
    fetching: &'a Fetching,
}

impl Drop for Walking<'_> {
    fn drop(&mut self) {
        let mut current = self
            .fetching
            .current
            .write()
            .unwrap_or_else(|e| e.into_inner());

        current.finished_at = Some(db::now());
        current.artist = None;
        current.cancelled = self.fetching.should_stop();

        self.fetching.cancel.store(false, Ordering::Release);
        self.fetching.running.store(false, Ordering::Release);
    }
}

/// Walks everybody who is wanting a picture, at the pace the far end asks for.
///
/// Runs until it runs out, is told to stop, or the far end stops answering.
/// Every artist is written down as it goes rather than at the end, so stopping
/// this half way keeps everything it had already found — which matters when the
/// whole walk is three quarters of an hour.
pub async fn walk(pool: &SqlitePool, data_dir: &Path, net: &Net, fetching: &Fetching) {
    let wanted = match wanting(pool).await {
        Ok(wanted) => wanted,
        Err(e) => return warn!("could not read who wants a picture: {e:#}"),
    };

    let Some(_walking) = fetching.begin(wanted.len() as u64) else {
        return debug!("a walk for portraits is already going");
    };

    info!("looking for {} portraits", wanted.len());

    for artist in wanted {
        if fetching.should_stop() {
            info!("stopped looking for portraits");
            break;
        }

        fetching.looking_at(&artist.name);

        match look_up(net, &artist.mbid).await {
            Ok(Some(portrait)) => {
                if let Err(e) = keep(pool, data_dir, artist.id, &portrait).await {
                    warn!("could not keep a portrait of {}: {e:#}", artist.name);
                }
                fetching.went(true);
            }
            Ok(None) => {
                if let Err(e) = remember_nothing(pool, artist.id).await {
                    warn!("could not write down a look at {}: {e:#}", artist.name);
                }
                fetching.went(false);
            }
            // The walk did not finish, which says nothing about this artist and
            // is not written down as if it did. One of these is a hiccup; a
            // service that is down is going to answer the same way nine hundred
            // times, so this gives up rather than spending three quarters of an
            // hour finding that out.
            Err(e) => {
                warn!("gave up looking for portraits: {e:#}");
                fetching.gave_up(format!("{e:#}"));
                break;
            }
        }

        tokio::time::sleep(PACE).await;
    }

    let snapshot = fetching.snapshot();
    info!(
        "portraits: {} of {} looked up, {} found",
        snapshot.done, snapshot.total, snapshot.found
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What MusicBrainz answers, cut down to the shape rather than the size: an
    /// artist with a homepage, a shop and a photograph.
    const RELATIONS: &str = r#"{
        "name": "Above & Beyond",
        "relations": [
            {"type": "official homepage",
             "url": {"resource": "https://aboveandbeyond.nu/"}},
            {"type": "image",
             "url": {"resource": "https://commons.wikimedia.org/wiki/File:Above_%26_Beyond_2013.jpg"}},
            {"type": "purchase for download",
             "url": {"resource": "https://example.com/shop"}}
        ]
    }"#;

    const TOLD: &str = r#"{
        "query": {"pages": {"123456": {
            "pageid": 123456,
            "title": "File:Above & Beyond 2013.jpg",
            "imageinfo": [{
                "url": "https://upload.wikimedia.org/wikipedia/commons/a/ab/Above.jpg",
                "thumburl": "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Above.jpg/640px-Above.jpg",
                "descriptionurl": "https://commons.wikimedia.org/wiki/File:Above_%26_Beyond_2013.jpg",
                "extmetadata": {
                    "Artist": {"value": "<a href=\"//commons.wikimedia.org/wiki/User:Someone\" title=\"User:Someone\">Someone</a>"},
                    "LicenseShortName": {"value": "CC BY-SA 4.0"},
                    "LicenseUrl": {"value": "https://creativecommons.org/licenses/by-sa/4.0"}
                }
            }]
        }}}
    }"#;

    #[test]
    fn the_one_relation_that_is_a_photograph_is_the_one_taken() {
        assert_eq!(
            pictured_at(RELATIONS).as_deref(),
            Some("File:Above_&_Beyond_2013.jpg"),
            "and its name comes back spelled rather than escaped"
        );
    }

    /// An artist with relations and none of them a picture is the common case,
    /// and it is not a failure.
    #[test]
    fn an_artist_with_no_picture_is_no_picture() {
        let json = r#"{"relations": [
            {"type": "official homepage", "url": {"resource": "https://example.com/"}}
        ]}"#;

        assert_eq!(pictured_at(json), None);
        assert_eq!(pictured_at(r#"{"relations": []}"#), None);
        assert_eq!(pictured_at("not json at all"), None);
    }

    /// A relation into Commons that is not one file — a category of them, a
    /// gallery — names no picture, and picking one out of it would be this
    /// choosing on somebody's behalf.
    #[test]
    fn a_relation_that_is_not_one_file_names_no_picture() {
        let json = r#"{"relations": [
            {"type": "image",
             "url": {"resource": "https://commons.wikimedia.org/wiki/Category:Above_and_Beyond"}}
        ]}"#;

        assert_eq!(pictured_at(json), None);
    }

    #[test]
    fn commons_is_read_for_the_copy_and_for_what_it_asks() {
        let told = told_about(TOLD).unwrap();

        assert!(
            told.url.contains("640px"),
            "the scaled copy, not the original: {}",
            told.url
        );
        assert_eq!(
            told.author.as_deref(),
            Some("Someone"),
            "the name, not the anchor"
        );
        assert_eq!(told.license.as_deref(), Some("CC BY-SA 4.0"));
        assert_eq!(
            told.license_url.as_deref(),
            Some("https://creativecommons.org/licenses/by-sa/4.0")
        );
        assert!(told.page.is_some(), "and where to point the credit");
    }

    /// A file Commons does not have comes back as a page with nothing in it,
    /// which has to read as nothing found rather than as an answer.
    #[test]
    fn a_file_commons_does_not_have_is_nothing() {
        let json = r#"{"query": {"pages": {"-1": {"title": "File:Nope.jpg", "missing": ""}}}}"#;

        assert!(told_about(json).is_none());
    }

    /// A picture too small to scale has no thumbnail, and then the original is
    /// both the right answer and already the size we wanted.
    #[test]
    fn a_file_with_no_scaled_copy_falls_back_to_itself() {
        let json = r#"{"query": {"pages": {"1": {"imageinfo": [{
            "url": "https://upload.wikimedia.org/small.jpg",
            "descriptionurl": "https://commons.wikimedia.org/wiki/File:Small.jpg",
            "extmetadata": {"LicenseShortName": {"value": "CC0"}}
        }]}}}}"#;

        let told = told_about(json).unwrap();
        assert_eq!(told.url, "https://upload.wikimedia.org/small.jpg");
        assert_eq!(told.author, None, "nobody said, so nobody is named");
    }

    #[test]
    fn markup_around_a_name_is_not_part_of_the_name() {
        assert_eq!(plain("<a href=\"/x\">Someone</a>"), "Someone");
        assert_eq!(plain("Ann &amp; Bob"), "Ann & Bob");
        assert_eq!(plain("<span>  spread   out  </span>"), "spread out");
        assert_eq!(plain("&quot;quoted&quot;"), "\"quoted\"");
        assert_eq!(plain(""), "");
    }

    /// The title goes into a query string, so the characters that are ordinary
    /// in a file name and special in a URL have to stop being special.
    #[test]
    fn a_title_with_an_ampersand_in_it_stays_one_parameter() {
        let url = commons_url("File:Above & Beyond.jpg");

        assert!(
            url.contains("titles=File%3AAbove%20%26%20Beyond%2Ejpg"),
            "{url}"
        );
        assert!(!url.contains("File:Above & Beyond"));
    }
}
