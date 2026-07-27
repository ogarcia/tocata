// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Which language the panel speaks.
//!
//! rust-i18n keeps the choice in a global, which is not something Leptos can
//! watch, so changing it mid-session does not repaint anything. Rather than
//! wrapping every string in machinery to make it reactive, choosing a language
//! stores it and reloads the page. It happens once, and a reload of a panel
//! costs nothing.

use rust_i18n::set_locale;

/// The ones there are. English first: it is the base language and the fallback.
pub const AVAILABLE: [(&str, &str); 2] = [("en", "English"), ("es", "Español")];

/// Where the choice is remembered between visits.
const REMEMBERED: &str = "tocata.locale";

/// Settles on a language and tells rust-i18n about it.
///
/// What was chosen wins over what the browser prefers, because somebody who
/// picked a language meant it.
pub fn settle() {
    let chosen = remembered().or_else(from_browser);

    if let Some(locale) = chosen.filter(|locale| is_available(locale)) {
        set_locale(&locale);
    }
}

/// The language in use, for marking the right entry in the menu.
pub fn current() -> String {
    rust_i18n::locale().to_string()
}

/// Remembers a language and reloads, which is what applies it.
pub fn choose(locale: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(REMEMBERED, locale);
    }

    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

fn is_available(locale: &str) -> bool {
    AVAILABLE.iter().any(|(code, _)| *code == locale)
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn remembered() -> Option<String> {
    storage()?.get_item(REMEMBERED).ok()?
}

/// The browser's preference, cut down to the language: `es-ES` and `es-419` are
/// both Spanish as far as anything here is concerned.
fn from_browser() -> Option<String> {
    let language = web_sys::window()?.navigator().language()?;
    let language = language.split('-').next()?.to_lowercase();

    Some(language)
}
