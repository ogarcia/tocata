// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use std::env;
use std::path::{Path, PathBuf};

/// Where Tocata keeps everything it owns on disk.
const DEFAULT_DATA_DIR: &str = "data";

/// The database file, always inside the data directory.
const DATABASE_FILE: &str = "tocata.db";

/// Runtime configuration, read from the environment.
pub struct Config {
    data_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        let data_dir = env::var_os("TOCATA_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));

        Self { data_dir }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join(DATABASE_FILE)
    }
}
