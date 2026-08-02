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

pub mod types;

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
pub mod panel;
#[cfg(feature = "server")]
pub mod purge;
#[cfg(feature = "server")]
pub mod resources;
#[cfg(feature = "server")]
pub mod scanner;
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
