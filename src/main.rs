// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

mod config;
mod db;
mod subsonic;

use anyhow::{Context, Result};
use axum::Router;
use config::Config;
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

    let config = Config::from_env()?;

    std::fs::create_dir_all(config.data_dir())
        .with_context(|| format!("creating data directory {}", config.data_dir().display()))?;

    let database_path = config.database_path();
    let _pool = db::connect(&database_path).await?;
    info!("database ready at {}", database_path.display());

    let app = Router::new().nest("/rest", subsonic::router());

    let addr = config.listen_addr();
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    info!("listening on http://{addr}");
    axum::serve(listener, app).await.context("serving")?;

    Ok(())
}
