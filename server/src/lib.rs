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
//
// And behind the `server` feature, one by one, like the modules below: they are SQL
// for a database the panel does not have. Compiled into the panel they were seven
// warnings on every wasm build — the sort of noise that ends up hiding a warning
// worth reading.

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
#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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
#[cfg(feature = "server")]
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
#[cfg(feature = "server")]
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
#[cfg(feature = "server")]
macro_rules! album_is_visible {
    ($album:literal) => {
        has_a_visible_track!(concat!("WHERE t.album_id = ", $album))
    };
}

/// Whether a track is one of an artist's.
///
/// Two ways of being theirs, and a name only has to meet one: credited on the track,
/// or signing the record the track is on. The second is not a technicality — it is
/// how a file writes "Prince and The Revolution", who sign Purple Rain while every
/// track on it credits Prince and The Revolution apart, and how it writes the name a
/// compilation is filed under. Counting only the first leaves those names in the
/// listing with nothing behind them.
///
/// Membership of a set gathered from the artist rather than a pair of questions
/// asked of a track, and the two say exactly the same thing at very different
/// prices. Asked of a track, the artist is named in another table, so no index
/// reaches the condition and answering it means reading every track once per
/// artist — a page of fifty names over eleven thousand tracks measured at seven
/// seconds on a slow machine. Gathered from the artist, each branch is an index
/// lookup on them, and a track that is theirs both ways lands in both branches
/// without counting twice, because this asks whether a track is in the set and
/// not how often.
///
/// `$track` is the alias the tracks table goes by, and `$artist` an expression for
/// the artist's row id — which the set names twice, so a caller binding it has to
/// bind it twice.
#[cfg(feature = "server")]
macro_rules! track_is_theirs {
    ($track:literal, $artist:literal) => {
        concat!(
            $track,
            ".id IN (SELECT ta.track_id FROM track_artists ta
                      WHERE ta.artist_id = ",
            $artist,
            "
                     UNION
                     SELECT tt.id FROM album_artists aa
                       JOIN tracks tt ON tt.album_id = aa.album_id
                      WHERE aa.artist_id = ",
            $artist,
            ")"
        )
    };
}

/// An artist with a track of theirs still worth showing.
#[cfg(feature = "server")]
macro_rules! artist_is_visible {
    ($artist:literal) => {
        has_a_visible_track!(concat!("WHERE ", track_is_theirs!("t", $artist)))
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
#[cfg(feature = "server")]
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
pub mod portraits;
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
