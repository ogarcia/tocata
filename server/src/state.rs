// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What every request handler can reach.

use crate::config::Config;
use crate::resources::Meter;
use crate::scanner::Progress;
use axum::extract::FromRef;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub scan: Arc<Progress>,
    pub config: Arc<Config>,
    /// Shared rather than made per request, because a share of the processor is a
    /// difference between two readings and something has to remember the first
    /// one.
    pub meter: Arc<Meter>,
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

impl FromRef<AppState> for watch::Receiver<bool> {
    fn from_ref(state: &AppState) -> Self {
        state.shutdown.clone()
    }
}
