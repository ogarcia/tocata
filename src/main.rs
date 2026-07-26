// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

mod auth;
mod config;
mod db;
mod scanner;
mod state;
mod subsonic;
mod user;

use anyhow::{Context, Result};
use axum::Router;
use config::Config;
use state::AppState;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    info!(
        "{} {} — free software under the GPL, version 3 or later",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    let config = Arc::new(Config::from_env()?);

    std::fs::create_dir_all(config.data_dir())
        .with_context(|| format!("creating data directory {}", config.data_dir().display()))?;

    let database_path = config.database_path();
    let pool = db::connect(&database_path).await?;
    info!("database ready at {}", database_path.display());

    user::ensure_initial_user(&pool).await?;
    scanner::sync_libraries(&pool, config.library_paths()).await?;

    let state = AppState {
        pool: pool.clone(),
        scan: Arc::new(scanner::Progress::default()),
        config: config.clone(),
    };

    let app = Router::new().nest("/rest", subsonic::router(state.clone()));

    let addr = config.listen_addr();
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    // Only once the port is ours: starting a scan before knowing whether the
    // server can even listen would leave a half finished run behind every
    // failed start.
    let initial = state.clone();
    tokio::spawn(async move {
        match scanner::scan_all(&initial.pool, scanner::Mode::Incremental, &initial.scan).await {
            Ok(Some(outcome)) => info!(
                "initial scan finished: {} folders, {} tracks ({} unchanged), {} failed, {} gone",
                outcome.folders, outcome.tracks, outcome.unchanged, outcome.failed, outcome.gone
            ),
            Ok(None) => {}
            Err(e) => tracing::error!("initial scan failed: {e:#}"),
        }
    });

    info!("listening on http://{addr}");
    axum::serve(listener, app).await.context("serving")?;

    Ok(())
}
