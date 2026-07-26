// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What every request handler can reach.

use crate::scanner::Progress;
use axum::extract::FromRef;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub scan: Arc<Progress>,
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
