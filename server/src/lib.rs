// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Tocata, as a library, which it is for exactly one reason.
//!
//! The panel is written in Rust too, and the whole point of that is for both
//! sides to agree on the shape of what travels between them by sharing the
//! definition rather than by both remembering it. So [`types`] is compiled into
//! the panel as well.
//!
//! Everything else here is the server, and the panel has no use for it — nor
//! could it, since sqlx, tokio and lofty do not build for the browser. That is
//! what the `server` feature is: on by default, and switched off by the one
//! dependant that wants only the types.

// Written here, above the modules, because a `macro_rules!` is in scope from
// where it stands to the end of the module holding it, nested modules included —
// so this is what puts them within reach of both APIs at once.
//
// They belong to both. Which libraries somebody may look at is a rule about the
// server and not about OpenSubsonic, and a second copy of it living under `/api`
// would be a second place for it to be wrong.

/// The libraries this request may look at, hoisted to the front of the statement.
///
/// A library counts when it is switched on and the person asking is allowed it:
/// either they have no restriction at all, which is the ordinary case and costs
/// nothing, or they have one naming this library.
///
/// It is a common table expression rather than a predicate repeated inline, and
/// that is what makes the parameter count predictable. The identifier is named
/// once here however many times the filter is needed further down, so every
/// statement that uses this takes it as its **first** bind and nothing else moves.
///
/// The subquery does not correlate with the statement around it, so SQLite works
/// it out once rather than per row.
///
/// Forgetting the expression is loud — `no such table: visible_libraries` —
/// and forgetting the bind is not, but it fails closed: a null identifier matches
/// no user, the set comes out empty, and the answer is nothing rather than
/// somebody else's music.
macro_rules! visible_libraries_head {
    () => {
        "
    WITH visible_libraries (id) AS (
        SELECT l.id FROM libraries l
         WHERE l.enabled = 1
           AND EXISTS (
                   SELECT 1 FROM users u
                    WHERE u.id = "
    };
}

macro_rules! visible_libraries_tail {
    () => {
        "
                      AND (NOT EXISTS (SELECT 1 FROM user_libraries ul
                                        WHERE ul.user_id = u.id)
                           OR EXISTS (SELECT 1 FROM user_libraries ul
                                       WHERE ul.user_id = u.id
                                         AND ul.library_id = l.id))
               )
    )
"
    };
}

/// The whole expression, for the callers that bind the user themselves.
///
/// Split in two above for the same reason the column lists are: a `QueryBuilder`
/// cannot take an argument without also writing its own `?`, so a statement it
/// assembles has to be given the pieces either side of the parameter.
macro_rules! visible_libraries {
    () => {
        concat!(visible_libraries_head!(), "?", visible_libraries_tail!())
    };
}

/// Whether a thing has at least one track worth showing.
///
/// A track is worth showing when its file is still there and its library is
/// switched on. Everything above a track — an album, an artist — is visible
/// exactly when one of its tracks is, so this is the one place that decides it.
///
/// `$join` reaches the tracks in question and must end in its own `WHERE`, since
/// the conditions below are appended to it.
macro_rules! has_a_visible_track {
    ($join:expr) => {
        concat!(
            "EXISTS (SELECT 1 FROM tracks t ",
            $join,
            " AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries))"
        )
    };
}

/// An album with something in it.
macro_rules! album_is_visible {
    ($album:literal) => {
        has_a_visible_track!(concat!("WHERE t.album_id = ", $album))
    };
}

/// An artist credited on a track, or on an album that still has one.
macro_rules! artist_is_visible {
    ($artist:literal) => {
        concat!(
            "(",
            has_a_visible_track!(concat!(
                "JOIN track_artists ta ON ta.track_id = t.id WHERE ta.artist_id = ",
                $artist
            )),
            " OR ",
            has_a_visible_track!(concat!(
                "JOIN album_artists aa ON aa.album_id = t.album_id WHERE aa.artist_id = ",
                $artist
            )),
            ")"
        )
    };
}

/// Whether a song announced as playing could still be playing.
///
/// A client says "now playing" when a song starts, and says it again for the next
/// one. Nothing obliges it to say anything when it stops: a phone that runs out of
/// battery, an app killed by the system or a browser tab closed all leave their
/// last announcement behind with nobody to replace it. Without a window that
/// entry stays for good, and the list of what is playing becomes part answer and
/// part graveyard.
///
/// How long a song could be playing is how long the song is. A minute is added for
/// a client that announced a little early, for one that reports the whole queue at
/// once, and for two clocks that do not quite agree. A length we do not know falls
/// back to five minutes: longer than most songs, and short enough to forget.
///
/// Nothing here can be read as a promise that the song stopped. It says only that
/// we no longer have grounds to claim it did not, which is the honest answer for
/// a client that stopped speaking.
///
/// `$started` and `$duration` name the columns to read, the second in
/// milliseconds.
macro_rules! still_playing {
    ($started:literal, $duration:literal) => {
        concat!(
            "(julianday('now') - julianday(",
            $started,
            ")) * 86400 < coalesce(",
            $duration,
            " / 1000, 300) + 60"
        )
    };
}

pub mod types;

/// Files on disk for the tests that need them, shared by the two modules that do.
#[cfg(all(test, feature = "server"))]
mod fixtures;

#[cfg(feature = "server")]
pub mod api;
#[cfg(feature = "server")]
pub mod artwork;
#[cfg(feature = "server")]
pub mod attempts;
#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod config;
#[cfg(feature = "server")]
pub mod db;
#[cfg(feature = "server")]
pub mod jobs;
#[cfg(feature = "server")]
pub mod lyrics;
#[cfg(feature = "server")]
pub mod media;
#[cfg(feature = "server")]
pub mod net;
#[cfg(feature = "server")]
pub mod panel;
#[cfg(feature = "server")]
pub mod plays;
#[cfg(feature = "server")]
pub mod purge;
#[cfg(feature = "server")]
pub mod resources;
#[cfg(feature = "server")]
pub mod scanner;
#[cfg(feature = "server")]
pub mod scrobble;
#[cfg(feature = "server")]
pub mod search;
#[cfg(feature = "server")]
pub mod session;
#[cfg(feature = "server")]
pub mod settings;
#[cfg(feature = "server")]
pub mod state;
#[cfg(feature = "server")]
pub mod subsonic;
#[cfg(feature = "server")]
pub mod upkeep;
#[cfg(feature = "server")]
pub mod user;
