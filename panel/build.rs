// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Makes a changed translation rebuild the panel.
//!
//! `rust_i18n::i18n!` reads the locale files while compiling and turns them into
//! code, but it does not tell cargo that it read them. So editing a translation
//! changes nothing until something else forces a rebuild, and what you see in the
//! browser is the text from whenever the last `.rs` file happened to change.
//!
//! Found the hard way: a fix to a translation appeared to do nothing, twice, and
//! the second time it was a test that had also stopped seeing it.

fn main() {
    println!("cargo::rerun-if-changed=locales");
}
