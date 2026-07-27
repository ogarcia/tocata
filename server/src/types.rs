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
pub struct AccountChanges {
    /// A new name for the account. Nothing else has to move with it.
    #[schema(example = "oscar")]
    pub username: Option<String>,
    pub password: Option<String>,
    pub email: Option<String>,
    /// Only an administrator may set this, and none may clear their own.
    pub admin: Option<bool>,
    pub scrobbling: Option<bool>,
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
fn mentioned<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
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
}

/// What may be changed. Anything left out is left alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChanges {
    /// The replacement list. An empty list is a valid answer: it means no word
    /// is skipped, which is what a collection in a language without articles
    /// wants.
    pub ignored_articles: Option<Vec<String>>,
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
