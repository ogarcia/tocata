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
    /// Where it sits on its record. What a row shows in the column its play button
    /// lives in, so the column says something at rest instead of standing empty.
    pub track_number: Option<i64>,
    /// Seconds, like every other length in this API.
    pub duration: Option<i64>,
    /// The file's extension, as scanned: flac, mp3, ogg. What the player names at
    /// the foot of its sheet — this is an administration panel before it is a
    /// listening client, and that is where somebody notices a file is not the
    /// quality they thought it was.
    pub suffix: String,
    /// Thousands of bits a second, where the file said. Absent for a format that
    /// does not report one.
    pub bit_rate: Option<i64>,
    /// Its file is not where it was. The row says so and stays in the listing,
    /// because a scan marks rather than deletes.
    pub missing: bool,
}

/// Everything a track's own panel says about it, as the database holds it.
///
/// The wide answer to [`Track`]'s narrow one, and a second call rather than a wider
/// first: a listing draws fifty rows and this fills one panel, so the columns only a
/// panel reads are not read fifty times over for nothing.
///
/// What is *not* here is what the file says. The scanner keeps the fields it has
/// columns for and lets the rest of a tag go by, so the credits there is no room for
/// — composer, producer, whoever engineered it — are read from the file itself when
/// somebody asks. That is [`Tags`].
///
/// Every optional field here means the file did not say. The panel leaves a row out
/// rather than printing a blank one, so what is on screen is what is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetail {
    pub id: String,
    pub title: String,
    /// Everybody credited on the recording, joined the way a line prints them.
    pub artists: Option<String>,
    pub album: Option<String>,
    /// So a panel can lead on to the record's own.
    pub album_id: Option<String>,
    /// Whoever the record is filed under, which is not always who played on this
    /// song: a compilation is filed under itself and its tracks credit their acts.
    pub album_artist: Option<String>,
    /// All of them, not the first one a row has room for.
    pub genres: Option<String>,
    pub track_number: Option<i64>,
    /// How many the record holds, so a number can be read as "2 of 10". Counted
    /// over the tracks that are still there, like every other figure about a
    /// record.
    pub album_tracks: Option<i64>,
    pub disc_number: Option<i64>,
    /// How many discs the record came on, where its tracks say. Absent — rather
    /// than one — when nothing on it numbers a disc at all, because a record that
    /// said nothing is not a record that said one.
    pub album_discs: Option<i64>,
    /// The track's own year, or its record's where the file did not say.
    pub year: Option<i64>,
    /// Seconds, like every other length in this API.
    pub duration: Option<i64>,
    pub suffix: String,
    pub bit_rate: Option<i64>,
    /// Hertz, as the file reports it. Said in kilohertz by whoever draws it.
    pub sampling_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    /// Where the file is *within its library*, which is how the scanner stores it:
    /// moving a library is then one update of one row rather than a rescan.
    ///
    /// Deliberately not the whole path. It is enough to find the file among the
    /// others and to tell two copies apart, and every account that may see the
    /// library can read it — where that library is mounted on the machine is
    /// nobody's business but the person who mounted it.
    pub path: String,
    /// Which library it came out of, by name.
    pub library: String,
    /// Bytes.
    pub size: i64,
    /// When the server last read this file's tags — which is not when it last saw
    /// the file. An incremental scan that finds a file unchanged marks it as seen
    /// and reads nothing, so this answers the question worth asking: how old is
    /// what you are looking at.
    pub read_at: String,
    pub isrc: Option<String>,
    pub mbid_recording: Option<String>,
    pub comment: Option<String>,
    /// Its file is not where it was, so nothing can be read out of it and nothing
    /// can be played.
    pub missing: bool,
}

/// What a file says about itself, read from the file and not from the database.
///
/// Every tag the reader made sense of, under the name the file's own format writes
/// rather than the name Tocata uses for it. This is where the credits are that the
/// schema has no columns for, which is the whole reason for asking.
///
/// It is not every byte in the tag. What arrives here has been through a reader that
/// maps the frames it knows onto one set of names, and a vendor's own invention it
/// has never heard of does not come through at all — so this cannot say how many were
/// left behind, and does not pretend to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tags {
    /// The kind of tag it turned out to be: ID3v2, Vorbis comments, MP4 and so on.
    /// Absent where the file carries no tag at all, which is a file with a title
    /// taken from its own name and nothing else.
    pub kind: Option<String>,
    /// In the order the file holds them.
    pub tags: Vec<Tagged>,
    /// The embedded artwork, described rather than sent: what it is a picture of,
    /// what kind of file, and how big. Somebody reading a tag list wants to know
    /// there is a cover in there and how much of the file it accounts for; drawing
    /// it is what the cover endpoint is for.
    ///
    /// A tag like the rest, and kept apart from them because it is the one whose
    /// value is a description rather than what the file says.
    pub picture: Option<Tagged>,
}

/// One tag: its name as the file spells it, and what it says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tagged {
    pub name: String,
    pub value: String,
}

/// The words of a song, and where they were found.
///
/// Read when asked and never stored: lyrics are the one long text a music file
/// carries, and a copy of them in the database would be hundreds of megabytes
/// saying what is already on disk. It also means words edited on disk show up
/// without a rescan, which is why nothing here reports when a scan last looked —
/// no scan ever looks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    /// Where they came from, so a panel can say it: the name of the file beside the
    /// music, or the tag frame inside it. Absent when there are none, which is the
    /// case worth explaining rather than an error.
    pub source: Option<LyricSource>,
    /// Whether the lines carry timings. Untimed words are one block of text and
    /// there is nothing for a player to follow.
    pub synced: bool,
    pub lines: Vec<LyricLine>,
    /// The name a file beside the music would have to have, without its extension.
    /// What the panel spells out when there are no words at all, since "put them
    /// here" is the only useful thing to say then.
    pub beside: String,
}

/// Which of the two places the words turned out to be in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum LyricSource {
    /// A file of this name sitting beside the music. It wins over an embedded tag:
    /// it is what somebody put there deliberately, it can be edited, and it is
    /// where anything fetched later would be written.
    Beside(String),
    /// A frame inside the file, named as its format names it: `USLT` in ID3v2,
    /// `LYRICS` or `UNSYNCEDLYRICS` in Vorbis comments.
    Frame(String),
}

/// One line of a song, with its place in it when the words are timed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    /// Milliseconds from the start. Absent on words with no timings.
    pub at: Option<i64>,
    /// Empty on a line that is a gap between verses, which is worth keeping: it is
    /// how a passage with no words reads as a passage rather than as the end.
    pub value: String,
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

/// Everything a record's own panel shows.
///
/// One call and not four, because a panel about a record is one thing to read: its
/// figures, what it is, what is on it, and who played. Fetching the track list apart
/// from the rest would only mean the panel drawing itself twice.
///
/// Like a track's, every optional field means nothing said so, and the panel leaves
/// the row out rather than printing a blank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetail {
    pub id: String,
    pub name: String,
    /// Who it is filed under, which on a compilation is not who played on it.
    pub artist: Option<String>,
    pub year: Option<i64>,
    /// Every genre its tracks carry, not the first one a row has room for.
    pub genres: Option<String>,
    /// Who put it out, as the tag said.
    pub label: Option<String>,
    /// How many of its tracks are still there.
    pub tracks: i64,
    /// And how many are not. Kept apart rather than added in, because a record
    /// missing four of its files is a thing to say out loud.
    pub missing: i64,
    /// Seconds, over the tracks that are still there.
    pub duration: Option<i64>,
    /// Bytes, over the same tracks.
    pub size: i64,
    /// The directory its files sit in, relative to the library — the same choice a
    /// track's own panel makes, and for the same reason.
    pub path: Option<String>,
    pub library: String,
    /// When the server last read any of its files.
    pub read_at: Option<String>,
    /// How many discs its tracks say it came on. Absent where none of them says.
    pub discs: Option<i64>,
    pub listing: Vec<AlbumTrack>,
    /// Everybody credited on its tracks, which is a different question from who the
    /// record is filed under: it is where the guests are.
    pub players: Vec<String>,
}

/// One track as a record's panel lists it: enough to read down the running order and
/// to open any of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlbumTrack {
    pub id: String,
    pub title: String,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub duration: Option<i64>,
    pub missing: bool,
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

/// Everything an artist's own panel shows.
///
/// What is here rather than on [`Artist`] is what a list of nine hundred names must
/// not pay for: their records, their most played songs, and how long all of it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDetail {
    pub id: String,
    pub name: String,
    /// What they are filed under, over everything they are on.
    pub genres: Option<String>,
    pub albums: i64,
    pub tracks: i64,
    /// Seconds, over everything of theirs that is still there.
    pub duration: Option<i64>,
    /// How many times anything of theirs has been played, by everybody.
    ///
    /// Summed from the per-track counts, because there is nowhere else it could come
    /// from: the artist stats table holds a rating and a star and no count. Which is
    /// right — a play is a play of a song, and an artist's total is a question asked of
    /// those rather than a number worth keeping in step with them.
    pub plays: i64,
    /// Whether a picture of them has been found. Same meaning as the listing's, which
    /// is "found already" rather than "exists".
    pub image: bool,
    pub records: Vec<ArtistAlbum>,
    /// Their most played songs, across everybody who listens here.
    pub played_most: Vec<PlayedTrack>,
}

/// One of an artist's records, as their panel lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtistAlbum {
    pub id: String,
    pub name: String,
    pub year: Option<i64>,
    pub tracks: i64,
    /// How many of its files have gone, so a record with a hole in it says so where it
    /// is listed rather than only once it is opened.
    pub missing: i64,
    pub duration: Option<i64>,
    pub cover: bool,
}

/// One song and how often it has been played.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlayedTrack {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    pub plays: i64,
    pub duration: Option<i64>,
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
