// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The shapes that travel between the server and the panel.
//!
//! One definition each, compiled into both sides. The point is not tidiness: it
//! is that renaming a field breaks the panel's build instead of breaking the
//! panel, which is the difference between a compiler error and a blank table
//! somebody notices next month.
//!
//! Everything here derives both directions. The server only ever writes an
//! `Account` and only ever reads a `NewAccount`, but the panel does the
//! opposite, and a shared type has to serve whoever holds it.
//!
//! `utoipa` is not behind a feature. It looked like it should be — the panel has
//! no use for a schema generator — but a schema nobody calls is dead code, and
//! measuring said it costs 105 bytes of the compressed bundle. That is not worth
//! a `cfg_attr` on every field.
//!
//! Every one of them compares, because a panel that can tell whether a value
//! changed is a panel that can decide not to repaint.
//!
//! What is not here: query parameters, and anything with a lifetime. Query
//! parameters belong to how a call is addressed rather than to what it carries,
//! and a borrowed string cannot be deserialised into, so text that used to be
//! `&'static str` on the way out is `String`.

use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

/// What a failed call returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    /// Stable identifier for the kind of failure. What a client should branch on.
    #[schema(example = "wrongCredentials")]
    pub code: String,
    /// English, for people reading responses. Never shown to a user as is.
    #[schema(example = "Wrong username or password")]
    pub message: String,
}

/// Deliberately says nothing beyond whether the answer is yes.
///
/// No version, no counts, no scan state. This is the only call that answers
/// without a session, so what it discloses to a stranger is the whole of what it
/// discloses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Health {
    #[schema(example = "ok")]
    pub status: String,
}

/// What a login asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Credentials {
    #[schema(example = "admin")]
    pub username: String,
    pub password: String,
    /// Whether the browser should keep the way in after it closes. Left out it
    /// does, which is what a client that has never heard of this expects and what
    /// the panel ticks by default.
    #[serde(default = "yes")]
    pub remember: bool,
}

/// The default for a field whose absence means true.
fn yes() -> bool {
    true
}

/// Who is logged in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    #[schema(example = "admin")]
    pub username: String,
    /// Whether this account administers the server, which is what decides how
    /// much of the panel is worth drawing.
    pub admin: bool,
    /// When the session stops working, so the panel can say so before it does
    /// rather than after a call has already failed.
    #[schema(example = "2026-08-25T18:00:00Z")]
    pub expires_at: String,
    /// Carried here rather than fetched separately, because the panel needs them
    /// before it draws anything: a second round trip would be a panel painted in
    /// the wrong theme first and corrected afterwards.
    pub preferences: Preferences,
}

/// How the panel looks and speaks, for the account that chose it.
///
/// Each of these is a choice or the absence of one, and the absence means
/// something: no theme is following the machine, no locale is following the
/// browser, no accent is the one the panel ships with. There are no defaults
/// here because a default would be the server deciding what it cannot know.
///
/// The values are identifiers the server stores and never reads. What a theme or
/// an accent can be belongs to the panel, so adding a colour is a line of CSS
/// rather than a change here, and the panel falls back to its own default for
/// anything it does not recognise.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// Light, dark, or nothing to follow the machine.
    #[schema(example = "dark")]
    pub theme: Option<String>,
    /// Which language, or nothing to follow the browser.
    #[schema(example = "es")]
    pub locale: Option<String>,
    /// Which of the panel's accent colours, or nothing for its own.
    #[schema(example = "plum")]
    pub accent: Option<String>,
}

/// What may be changed. Anything left out is left alone, and an explicit `null`
/// unchooses — which has to be told apart from absent, because "follow the
/// machine" is a thing somebody can go back to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceChanges {
    #[serde(
        default,
        deserialize_with = "mentioned",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(example = "dark")]
    pub theme: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "mentioned",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(example = "es")]
    pub locale: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "mentioned",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(example = "plum")]
    pub accent: Option<Option<String>>,
}

/// A session as the panel can talk about it, which is to say without the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Login {
    pub id: i64,
    pub created_at: String,
    /// Roughly when a request last arrived on it, to the nearest few minutes.
    pub last_seen_at: String,
    pub expires_at: String,
    /// Whether this is the session asking. What keeps somebody from closing the
    /// window they are looking through by mistake.
    pub current: bool,
}

/// How many keys were revoked, so the panel can say something true afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Revoked {
    pub revoked: u64,
}

/// How many were closed, so the panel can say something true afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Closed {
    pub closed: u64,
}

/// How a scan is going, or how the last one went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether one is running now. Everything else describes that run while it
    /// is true, and the run before it once it is not.
    pub scanning: bool,
    /// Name of the library being walked.
    #[schema(example = "music")]
    pub library: Option<String>,
    /// Where the scan had got to when this was sampled. Not every file goes past
    /// here: it is here to show that something is happening.
    pub path: Option<String>,
    pub folders: u64,
    /// Files recorded, including the ones whose tags could not be read.
    pub tracks: u64,
    /// Of those, the ones already known and unchanged, so never reopened.
    pub unchanged: u64,
    /// Of those, the ones whose tags could not be understood.
    pub failed: u64,
    /// Marked absent because they are no longer on disk. Marked, never deleted.
    pub gone: u64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// True when the last run gave up rather than finishing, in which case
    /// nothing it had written was kept.
    pub cancelled: bool,
}

/// A library and how much of it there is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub id: i64,
    #[schema(example = "music")]
    pub name: String,
    #[schema(example = "/srv/music")]
    pub path: String,
    /// A disabled library is skipped by scans and left out of the folder list
    /// clients ask for. Everything already recorded from it stays browsable, so
    /// this is not a way to hide music; enabling it again costs nothing.
    pub enabled: bool,
    /// Tracks that were there the last time this library was scanned.
    pub tracks: i64,
    /// Tracks recorded here that are no longer on disk. Marked, never deleted, so
    /// a disk that failed to mount does not take somebody's playlists with it.
    pub missing: i64,
    /// When a scan of this library last ran to the end.
    pub last_scanned_at: Option<String>,
}

/// What it takes to add one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewLibrary {
    /// Absolute path to a directory that already exists.
    #[schema(example = "/srv/music")]
    pub path: String,
    /// What to call it. Defaults to the name of the directory, which is what
    /// `TOCATA_LIBRARY_PATHS` does too.
    #[schema(example = "music")]
    pub name: Option<String>,
}

/// What may be changed about one. Anything left out is left alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LibraryChanges {
    #[schema(example = "vinyl rips")]
    pub name: Option<String>,
    /// Where the music is now. Moving a library does not lose anything: the next
    /// scan finds each file at its new path and matches it against the row that
    /// went missing, so ratings, play counts and playlists stay where they were.
    #[schema(example = "/srv/music")]
    pub path: Option<String>,
    pub enabled: Option<bool>,
}

/// An account, as somebody entitled to see it may.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    #[schema(example = "admin")]
    pub username: String,
    pub email: Option<String>,
    /// Whether this account administers the server.
    pub admin: bool,
    /// Whether plays from this account are passed on to a scrobbling service.
    pub scrobbling: bool,
    /// When the password was last set. What tells somebody looking at their own
    /// account how long ago that was, which `updated_at` cannot: that moves for a
    /// change of address as readily as for a change of password.
    #[schema(example = "2026-05-14T09:12:00Z")]
    pub password_set_at: String,
    /// Roughly when a request last arrived on this account, by any door. Null means
    /// it has never been used. To the nearest few minutes, which is as precise as a
    /// figure nobody reads twice a day needs to be.
    #[schema(example = "2026-07-29T08:15:00Z")]
    pub last_seen_at: Option<String>,
    /// Sessions logged in and not yet expired. What tells an administrator that
    /// an account is in use before they remove it.
    pub sessions: i64,
    /// API keys issued to this account and not revoked.
    pub keys: i64,
    /// Libraries this account is restricted to. Empty means no restriction, so
    /// every library that is switched on.
    pub libraries: Vec<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// What only this account has, counted.
///
/// For the one question that needs it: what is lost by deleting the account. The
/// figures are not on the account itself because a listing of ten accounts would
/// count seven things apiece to show none of them — this is asked at the moment
/// somebody is about to be told what they are about to destroy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Holdings {
    /// Panel logins still open.
    pub sessions: i64,
    /// API keys that have not been revoked.
    pub keys: i64,
    /// Tracks, albums and artists starred, counted together: what somebody would
    /// call their favourites is one figure to them, whatever it is three tables
    /// here.
    pub favourites: i64,
    /// Ratings given, over the same three.
    pub ratings: i64,
    /// Times a track was played, which is the figure a listener would recognise.
    /// Album and artist counts are the same plays counted again.
    pub plays: i64,
    /// Playlists they own. Shared ones go too: a playlist belongs to whoever made
    /// it, and there is nobody to hand it to.
    pub playlists: i64,
    pub bookmarks: i64,
}

/// What it takes to create an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewAccount {
    #[schema(example = "oscar")]
    pub username: String,
    pub password: String,
    pub email: Option<String>,
    /// Defaults to false. An account that administers nothing is the safe thing
    /// to create by accident.
    #[serde(default)]
    pub admin: bool,
}

/// What may be changed. Anything left out is left alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountChanges {
    /// A new name for the account. Nothing else has to move with it.
    #[schema(example = "oscar")]
    pub username: Option<String>,
    pub password: Option<String>,
    pub email: Option<String>,
    /// Only an administrator may set this, and none may clear their own.
    pub admin: Option<bool>,
    pub scrobbling: Option<bool>,
    /// The password as it is now, which changing your own name, address or
    /// password requires and nothing else does.
    ///
    /// What it is for is the browser somebody left open, not the session: the
    /// session already proved itself, and that is exactly the problem — it proved
    /// itself an hour ago and whoever is sitting there now inherited it. Anything
    /// that would lock the owner out of their own account asks again.
    ///
    /// Not asked of an administrator changing somebody else's account: they do not
    /// have that password, and the account they would be locking out is not theirs.
    pub current_password: Option<String>,
}

/// Which libraries an account is restricted to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LibraryAccess {
    /// Identifiers of the libraries this account may see. An empty list removes
    /// the restriction, which is not the same as seeing nothing: an account with
    /// no restriction sees every library that is switched on.
    pub libraries: Vec<i64>,
}

/// A key as it can be talked about afterwards, which is to say without the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    pub id: i64,
    /// What it is for, so one can be revoked without guessing.
    #[schema(example = "phone")]
    pub label: String,
    pub created_at: String,
    /// When it stops working. Null means never, which is the default.
    #[schema(example = "2026-12-31T23:59:59Z")]
    pub expires_at: Option<String>,
    /// Whether that moment has passed. Worked out by the server so that a panel
    /// showing a list does not have to compare timestamps of its own to know what
    /// to grey out, and so it cannot disagree about which keys work.
    pub expired: bool,
    /// When a request last arrived with it. Null means it has never been used,
    /// which is the interesting case when something is not working.
    pub last_used_at: Option<String>,
    /// When it was withdrawn. Null means it was not. A revoked key authenticates
    /// nothing whatever its expiry says, and cannot be brought back: what is left
    /// to do with it is remove it.
    #[schema(example = "2026-07-29T08:15:00Z")]
    pub revoked_at: Option<String>,
}

/// A key at the one moment it can be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedKey {
    pub id: i64,
    pub label: String,
    pub created_at: String,
    /// When it stops working, if it was given a date.
    pub expires_at: Option<String>,
    /// The key itself. Not stored and not shown again: what the database keeps is
    /// a hash of it.
    #[schema(example = "3b1f...")]
    pub key: String,
}

/// What it takes to issue one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewKey {
    /// A name for it. Defaults to something unhelpful on purpose, so that whoever
    /// makes several is nudged into naming them.
    #[schema(example = "phone")]
    pub label: Option<String>,
    /// When it should stop working. Left out, it never does.
    #[schema(example = "2026-12-31T23:59:59Z")]
    pub expires_at: Option<String>,
}

/// What may be changed about a key once it exists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyChanges {
    #[schema(example = "phone")]
    pub label: Option<String>,
    /// A new expiry, or null for none at all. Leaving it out keeps whatever the
    /// key has, which is why this is an option inside an option: "never again"
    /// and "not mentioned" are different answers and both have to be sayable.
    #[serde(
        default,
        deserialize_with = "mentioned",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(example = "2027-06-30T00:00:00Z")]
    pub expires_at: Option<Option<String>>,
}

/// Tells `null` apart from absent for a nullable field.
///
/// Serde folds both into `None` by default, since the outer option is filled in
/// by `default` and the inner one swallows the null. Deserializing the inner
/// value and wrapping it here means this only runs when the key was written, so
/// what comes back is `Some(None)` for null and `None` for absent.
fn mentioned<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

/// The settings, whole. Small enough that there is no reason to fetch pieces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Words skipped when deciding which letter a name files under, so that
    /// "The Beatles" appears among the Bs. A list rather than one string,
    /// because a separator is a thing to get wrong and a list is not.
    #[schema(example = json!(["The", "El", "La", "Los", "Las"]))]
    pub ignored_articles: Vec<String>,
    /// Whether a quick scan runs every time the server starts.
    pub scan_at_startup: bool,
    /// The minute of the local day a quick scan runs at, `HH:MM`, or null for no
    /// schedule.
    #[schema(example = "04:00")]
    pub scan_at: Option<String>,
    /// How many days something stays marked absent before a scan clears it out,
    /// or null to never clear it automatically. Zero removes it as soon as a
    /// scan finds it gone.
    #[schema(example = 7)]
    pub absent_grace_days: Option<i64>,
    /// How long a panel login lasts, in days.
    #[schema(example = 30)]
    pub session_days: i64,
}

/// What may be changed. Anything left out is left alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChanges {
    /// The replacement list. An empty list is a valid answer: it means no word
    /// is skipped, which is what a collection in a language without articles
    /// wants.
    pub ignored_articles: Option<Vec<String>>,
    pub scan_at_startup: Option<bool>,
    /// An hour of the local day, or null to stop scanning on a schedule. An
    /// option inside an option, like every nullable field here: "never" and "not
    /// mentioned" are different answers.
    #[serde(
        default,
        deserialize_with = "mentioned",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(example = "04:00")]
    pub scan_at: Option<Option<String>>,
    /// A new quarantine, or null to stop clearing absent things automatically.
    #[serde(
        default,
        deserialize_with = "mentioned",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(example = 7)]
    pub absent_grace_days: Option<Option<i64>>,
    /// Only sessions opened afterwards last this long. An open one keeps the
    /// expiry it was given.
    #[schema(example = 30)]
    pub session_days: Option<i64>,
}

/// What the server is costing the machine, right now.
///
/// Separate from [`Stats`] because these are the only figures here that are true
/// of a moment rather than of the collection: everything else in this API is worth
/// asking for once, and these two are worth watching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Resources {
    /// Share of the machine's processing capacity this process is using, from 0 to
    /// 100, averaged over the time since these figures were last asked for.
    ///
    /// Of the machine and not of one core, so it has a ceiling: a process spread
    /// over four threads of eight reads as 50 rather than as 400.
    #[schema(example = 3.75)]
    pub cpu: f64,
    /// How many cores that is a share of, so a panel can say what the share means.
    pub cores: i64,
    /// Bytes of memory the process is holding.
    pub memory: i64,
    /// Bytes the machine has, when it can be read. What the figure above is a
    /// share of, and without it there is no share to draw.
    pub memory_total: Option<i64>,
}

/// The dashboard, more or less.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Version of the server answering.
    #[schema(example = "0.1.0")]
    pub version: String,
    pub artists: i64,
    pub albums: i64,
    /// Tracks that are on disk.
    pub tracks: i64,
    /// Tracks recorded but no longer on disk. What a purge would remove.
    pub missing: i64,
    pub genres: i64,
    pub playlists: i64,
    pub users: i64,
    /// API keys across every account that have not been revoked. Counted here
    /// because the panel shows it beside the accounts, and an administrator
    /// looking at one wants the other.
    pub keys: i64,
    pub libraries: i64,
    /// Bytes of music, counting only what is still there.
    pub total_size: i64,
    /// Seconds of music, likewise.
    pub total_duration: i64,
    /// Bytes the database takes, its write-ahead log included, since during
    /// normal running that is where a good part of it lives.
    pub database_size: i64,
}

/// What a purge would take, in the terms that matter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Loss {
    /// Tracks marked absent, which is what would be deleted.
    pub tracks: i64,
    /// Playlist entries pointing at them. The playlists themselves survive,
    /// shorter.
    pub playlist_entries: i64,
    /// Of those tracks, how many somebody had starred.
    pub favourites: i64,
    /// How many carried a rating.
    pub ratings: i64,
    /// How many had been played at least once, and would lose the count.
    pub played: i64,
    /// Bookmarks inside them.
    pub bookmarks: i64,
}

/// What a purge actually took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub tracks: i64,
    pub folders: i64,
    /// Albums left with no tracks.
    pub albums: i64,
    /// Artists left with neither tracks nor albums.
    pub artists: i64,
    pub genres: i64,
    pub moods: i64,
    /// Cover art no album or artist refers to any more. Cached files go with the
    /// rows.
    pub artworks: i64,
}

/// A job somebody runs when something is off.
///
/// Named rather than numbered, because the name is what the API takes and what
/// a run in the history is filed under, and both outlive any list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Job {
    /// Removes for good what a scan marked absent.
    Purge,
    /// Gives back the space deletions left behind in the database.
    Compact,
    /// Puts the cover cache in order: deletes the cached files no row names any
    /// more, and forgets that a cover was looked for and not found, so that the
    /// next client to ask makes the server look again.
    Covers,
    /// Reads the database through, and says whether anything is wrong with it.
    Check,
}

impl Job {
    /// What the name looks like in a URL, and in the history.
    pub fn name(self) -> &'static str {
        match self {
            Self::Purge => "purge",
            Self::Compact => "compact",
            Self::Covers => "covers",
            Self::Check => "check",
        }
    }
}

/// One run of one job.
///
/// What it found is a single number, and the job says what the number is of:
/// tracks removed, bytes reclaimed, files deleted, lookups forgotten, problems
/// found. Zero is an answer rather than the absence of one — a check that found
/// nothing wrong is what somebody was hoping for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub job: Job,
    /// When it started.
    #[schema(example = "2026-08-01T04:00:00Z")]
    pub at: String,
    /// False for a run the server stopped in the middle of, which is the only
    /// way one ends without either a count or an error.
    pub finished: bool,
    pub affected: i64,
    /// Why it could not be done — or, for the check, what it found wrong, since
    /// that is the one job whose bad news is prose.
    pub error: Option<String>,
}

/// A job as a screen needs it: what it is, what it would do if it ran now, and
/// how it went last time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobState {
    pub job: Job,
    /// How much this job would affect right now, in whatever it counts. None for
    /// a job that changes nothing and so has nothing to warn about.
    pub pending: Option<i64>,
    pub last_run: Option<Run>,
}

/// The whole maintenance screen in one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Maintenance {
    /// Every job, in the order they are worth offering.
    pub jobs: Vec<JobState>,
    /// The last few runs of anything, newest first.
    pub lately: Vec<Run>,
}

/// A page of a listing, and how many there are in all.
///
/// The total is what an endless list needs to know when to stop asking, and what
/// the heading counts while somebody narrows a search. One type per kind rather
/// than one generic one: the schema a client reads is the point of these, and a
/// generic named `Page_of_Track` says less than `Tracks` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tracks {
    pub total: i64,
    pub tracks: Vec<Track>,
}

/// A track as a list shows it.
///
/// Not what `/rest` calls a Child: that carries everything a player might want,
/// and this carries the five things a row prints. Anything a screen does not draw
/// is a column read for nothing, once per row, twenty-four thousand times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub title: String,
    /// Everybody credited, joined the way the row prints them.
    pub artists: Option<String>,
    pub album: Option<String>,
    /// So a title can lead to the album it is from.
    pub album_id: Option<String>,
    /// The first one, since a row has space for one.
    pub genre: Option<String>,
    /// Seconds, like every other length in this API.
    pub duration: Option<i64>,
    /// Its file is not where it was. The row says so and stays in the listing,
    /// because a scan marks rather than deletes.
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Albums {
    pub total: i64,
    pub albums: Vec<Album>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub year: Option<i64>,
    pub tracks: i64,
    /// How long it lasts, in seconds, counting only the tracks that are still
    /// there — the same tracks `tracks` counts, so the two never describe
    /// different records. Absent where nothing on it has a length.
    pub duration: Option<i64>,
    /// Whether asking for its cover would come back with one. A grid of two
    /// hundred albums draws the empty ones without asking for two hundred images
    /// that are not there.
    pub cover: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Artists {
    pub total: i64,
    pub artists: Vec<Artist>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub albums: i64,
    pub tracks: i64,
    /// Whether a picture of them has already been found. False also covers "not
    /// looked for yet", which is why a listing draws the empty frame and the
    /// picture appears on the next visit rather than never.
    pub image: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Genres {
    pub total: i64,
    pub genres: Vec<Genre>,
}

/// A genre. Its name is its identifier: the column is unique, and a genre has
/// nothing else to be known by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    pub name: String,
    pub albums: i64,
    pub tracks: i64,
}

/// What to play, as identifiers and nothing else.
///
/// Playing what you are looking at means playing everything the filter matches
/// rather than the fifty rows that happened to be fetched, so this is every one
/// of them — which is affordable precisely because it is only the identifiers.
///
/// How many is enough is the caller's to decide and comes back as asked for. An
/// object rather than a bare array so that something can be said alongside them
/// one day without every client having to be changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    pub tracks: Vec<String>,
}
