// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

mod api;
mod artwork;
mod auth;
mod config;
mod db;
mod lyrics;
mod scanner;
mod session;
mod settings;
mod state;
mod subsonic;
mod user;

use anyhow::{Context, Result};
use axum::Router;
use config::Config;
use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{oneshot, watch};
use tokio::time::sleep;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// How long whatever is being served gets to finish once the server has been
/// asked to stop.
///
/// Comfortably under the ten seconds a container runtime waits before it stops
/// asking and starts killing, because being killed is precisely what leaves the
/// database's side files behind.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

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
    settings::seed(&pool, config.ignored_articles()).await?;

    // Held here and handed out through the state, so a handler that keeps a
    // connection open can tell when to let go of it.
    let (stopping, is_stopping) = watch::channel(false);

    let state = AppState {
        pool: pool.clone(),
        scan: Arc::new(scanner::Progress::default()),
        config: config.clone(),
        shutdown: is_stopping,
    };

    let app = Router::new()
        .nest("/rest", subsonic::router(state.clone()))
        .merge(api::router(state.clone()));

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

    let mut interrupt =
        signal(SignalKind::interrupt()).context("installing the interrupt handler")?;
    let mut terminate =
        signal(SignalKind::terminate()).context("installing the termination handler")?;

    // One signal, two things that need to know about it: the server, so it stops
    // accepting, and the deadline below, so it starts counting from then instead
    // of from now.
    let (asked, was_asked) = oneshot::channel();
    let scan = state.scan.clone();

    let shutdown = async move {
        tokio::select! {
            _ = interrupt.recv() => info!("interrupted"),
            _ = terminate.recv() => info!("asked to stop"),
        }
        // First of all, before anything drains: a scan is the only thing here
        // that holds a transaction open for minutes, and nothing can close the
        // database under it.
        scan.cancel();
        // Streams that would otherwise stay open until their client goes away.
        let _ = stopping.send(true);
        let _ = asked.send(());
    };

    let deadline = async {
        let _ = was_asked.await;
        sleep(DRAIN_GRACE).await;
    };

    info!("listening on http://{addr}");

    // Streaming a file holds no database connection — the row is read and the
    // connection returned long before the bytes start moving — so a slow listener
    // delays only their own request. Waiting on them for ever, though, is what
    // makes a server look hung to somebody who pressed Ctrl-C.
    tokio::select! {
        result = axum::serve(listener, app).with_graceful_shutdown(shutdown) => {
            result.context("serving")?;
        }
        () = deadline => warn!("still serving something after {DRAIN_GRACE:?}; stopping anyway"),
    }

    // This is what takes the -wal and -shm files with it. SQLite writes them
    // beside the database while anybody has it open and tidies them away when the
    // last connection goes, so leaving them behind is not untidiness: it is the
    // sign of a process that was killed rather than asked.
    pool.close().await;
    info!("stopped");

    Ok(())
}
