// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use anyhow::{Context, Result};
use std::env;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

/// Where Tocata keeps everything it owns on disk.
const DEFAULT_DATA_DIR: &str = "data";

/// The database file, always inside the data directory.
const DATABASE_FILE: &str = "tocata.db";

/// The port Subsonic servers have historically listened on.
const DEFAULT_PORT: u16 = 4040;

/// Articles dropped when deciding which letter an artist files under. Spanish
/// and English by default, since those are the two the author has in his own
/// library; anything else is a matter of setting the variable.
const DEFAULT_IGNORED_ARTICLES: &str = "The El La Los Las Le Les";

/// Separator for the library list. A colon, as in PATH, because a comma is a
/// perfectly ordinary character in a directory name and a colon is not.
const LIBRARY_SEPARATOR: char = ':';

/// Runtime configuration, read from the environment.
pub struct Config {
    data_dir: PathBuf,
    port: u16,
    library_paths: Vec<PathBuf>,
    ignored_articles: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_dir = env::var_os("TOCATA_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));

        // A malformed port is a mistake worth stopping for, not something to
        // paper over with the default.
        let port = match env::var("TOCATA_PORT") {
            Ok(value) => value
                .parse()
                .with_context(|| format!("TOCATA_PORT is not a port number: {value}"))?,
            Err(_) => DEFAULT_PORT,
        };

        // Declarative on purpose: a container gets its libraries from the
        // environment, the same place it gets its volumes.
        let library_paths = env::var("TOCATA_LIBRARY_PATHS")
            .unwrap_or_default()
            .split(LIBRARY_SEPARATOR)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect();

        let ignored_articles = env::var("TOCATA_IGNORED_ARTICLES")
            .unwrap_or_else(|_| DEFAULT_IGNORED_ARTICLES.to_string())
            .split_whitespace()
            .map(str::to_string)
            .collect();

        Ok(Self {
            data_dir,
            port,
            library_paths,
            ignored_articles,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join(DATABASE_FILE)
    }

    pub fn library_paths(&self) -> &[PathBuf] {
        &self.library_paths
    }

    pub fn ignored_articles(&self) -> &[String] {
        &self.ignored_articles
    }

    pub fn listen_addr(&self) -> SocketAddr {
        (Ipv4Addr::UNSPECIFIED, self.port).into()
    }
}
