// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use anyhow::{Context, Result};
use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tocata::config::Config;
use tocata::state::AppState;
use tocata::{
    api, attempts, db, net, panel, portraits, resources, scanner, scrobble, settings, subsonic,
    upkeep, user,
};
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

    // Read once, here, and from now on read from memory. Everything that wants a
    // setting inside a request still asks the row; this is for the scheduler,
    // which wants one every minute and nothing else.
    let settings = Arc::new(settings::Current::read(&pool).await?);

    // Held here and handed out through the state, so a handler that keeps a
    // connection open can tell when to let go of it.
    let (stopping, is_stopping) = watch::channel(false);

    // Its first reading is taken here, so the first share it reports covers the
    // time since the server started rather than nothing at all.
    let meter = resources::Meter::new().context("reading what this process is using")?;

    let state = AppState {
        pool: pool.clone(),
        scan: Arc::new(scanner::Progress::default()),
        portraits: Arc::new(portraits::Fetching::default()),
        attempts: Arc::new(attempts::Attempts::new()),
        config: config.clone(),
        meter: Arc::new(meter),
        settings: settings.clone(),
        net: net::Net::new(),
        shutdown: is_stopping,
    };

    let app = Router::new()
        .nest("/rest", subsonic::router(state.clone()))
        .merge(api::router(state.clone()))
        .fallback(panel::serve);

    let addr = config.listen_addr();
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    // Only once the port is ours: starting a scan before knowing whether the
    // server can even listen would leave a half finished run behind every
    // failed start.
    if settings.borrow().scan_at_startup {
        let initial = state.clone();
        tokio::spawn(async move {
            upkeep::scan(&initial, scanner::Mode::Incremental).await;
        });
    }

    tokio::spawn(upkeep::on_schedule(state.clone()));

    // Nothing to cancel and nothing to wait for on the way out: it holds a
    // connection only while a request of its own is in flight, and a listen it did
    // not manage to hand over is still in the queue for the next start.
    tokio::spawn(scrobble::sending::as_they_come(
        state.net.clone(),
        pool.clone(),
    ));

    let mut interrupt =
        signal(SignalKind::interrupt()).context("installing the interrupt handler")?;
    let mut terminate =
        signal(SignalKind::terminate()).context("installing the termination handler")?;

    // One signal, two things that need to know about it: the server, so it stops
    // accepting, and the deadline below, so it starts counting from then instead
    // of from now.
    let (asked, was_asked) = oneshot::channel();
    let scan = state.scan.clone();
    let fetching = state.portraits.clone();

    let shutdown = async move {
        tokio::select! {
            _ = interrupt.recv() => info!("interrupted"),
            _ = terminate.recv() => info!("asked to stop"),
        }
        // First of all, before anything drains: a scan is the only thing here
        // that holds a transaction open for minutes, and nothing can close the
        // database under it.
        scan.cancel();
        // And the walk out for portraits, which holds no transaction but does
        // hold a task that would otherwise sit out a one second pace, and then
        // another, for as long as the drain allows.
        fetching.cancel();
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
        // With the connection's address, because logging in counts its failures
        // by where they came from and there is nowhere else to learn that. Behind
        // a reverse proxy every request arrives from the proxy, which makes the
        // count server-wide rather than per visitor — the wait still works, it is
        // just shared. Trusting a forwarded-for header instead would mean
        // trusting whoever sends it.
        result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        ).with_graceful_shutdown(shutdown) => {
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
