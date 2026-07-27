// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Says what to do about a panel that has not been built.
//!
//! Without this, a fresh clone fails with `proc macro panicked` from inside
//! `include_dir!`, which says nothing about trunk and nothing about the order
//! these two have to be built in.

use std::path::Path;

fn main() {
    println!("cargo::rerun-if-changed=../panel/dist");

    // The panel builds this very crate to get at the types, with everything
    // else switched off, and it does that before there is any panel to embed.
    // Complaining then would be a circle: the panel cannot be built until the
    // panel has been built. Only the build that embeds it has anything to say.
    if std::env::var_os("CARGO_FEATURE_SERVER").is_none() {
        return;
    }

    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../panel/dist");

    if !dist.join("index.html").exists() {
        println!(
            "cargo::error=the panel has not been built, and the server carries it inside. \
             Run `trunk build --release` in panel/ first."
        );
    }
}
