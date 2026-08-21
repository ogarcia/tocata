// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What every request handler can reach.

use crate::attempts::Attempts;
use crate::config::Config;
use crate::net::Net;
use crate::portraits::Fetching;
use crate::resources::Meter;
use crate::scanner::Progress;
use crate::settings;
use axum::extract::FromRef;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub scan: Arc<Progress>,
    /// The walk out to MusicBrainz and Commons for pictures of the artists.
    /// Shared for the same reason a scan's progress is: it is what stops two of
    /// them from going at once, and what a panel watching it reads.
    pub portraits: Arc<Fetching>,
    pub config: Arc<Config>,
    /// Shared rather than made per request, because a share of the processor is a
    /// difference between two readings and something has to remember the first
    /// one.
    pub meter: Arc<Meter>,
    /// Who has been getting their password wrong lately. Shared because it is
    /// the whole point: a count kept per request would count to one for ever.
    pub attempts: Arc<Attempts>,
    /// The settings, kept in memory so the one loop that watches the clock does
    /// not have to ask the database every minute what hour it is watching for.
    /// Shared because it is also where a change to them is published.
    pub settings: Arc<settings::Current>,
    /// For reaching somebody else's server, which today means passing listens on.
    /// Shared for the connection pool inside it: one kept per request would open
    /// a fresh connection, and a fresh TLS handshake, for every song.
    pub net: Net,
    /// Turns true once, when the server has been asked to stop. Handlers that
    /// hold a connection open for as long as the client wants it — the event
    /// stream — have to watch this, or every shutdown would wait out the whole
    /// drain deadline on a stream that was never going to end by itself.
    pub shutdown: watch::Receiver<bool>,
}

/// Lets an extractor ask for just the pool, without knowing what else the
/// state carries.
impl FromRef<AppState> for SqlitePool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<AppState> for Arc<Progress> {
    fn from_ref(state: &AppState) -> Self {
        state.scan.clone()
    }
}

impl FromRef<AppState> for Arc<settings::Current> {
    fn from_ref(state: &AppState) -> Self {
        state.settings.clone()
    }
}

impl FromRef<AppState> for Arc<Fetching> {
    fn from_ref(state: &AppState) -> Self {
        state.portraits.clone()
    }
}

impl FromRef<AppState> for Arc<Config> {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}

impl FromRef<AppState> for Arc<Meter> {
    fn from_ref(state: &AppState) -> Self {
        state.meter.clone()
    }
}

impl FromRef<AppState> for Arc<Attempts> {
    fn from_ref(state: &AppState) -> Self {
        state.attempts.clone()
    }
}

impl FromRef<AppState> for Net {
    fn from_ref(state: &AppState) -> Self {
        state.net.clone()
    }
}

impl FromRef<AppState> for watch::Receiver<bool> {
    fn from_ref(state: &AppState) -> Self {
        state.shutdown.clone()
    }
}
