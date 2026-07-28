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
/// What this browser saw last wins over what the browser prefers, because it is a
/// cache of what the account chose and somebody who picked a language meant it.
/// The account's own answer arrives with the session, and `adopt` applies it.
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

/// Adopts what the server says this account chose.
///
/// No reload, and none needed: this runs before the panel proper is built, so every
/// string in it is read after the language is settled. What was on screen until now
/// is a spinner.
///
/// Nothing chosen leaves the browser's own preference in force, which is what the
/// absence of a choice means.
pub fn adopt(locale: Option<&str>) {
    match locale.filter(|locale| is_available(locale)) {
        Some(locale) => {
            set_locale(locale);
            remember(locale);
        }
        // Nothing chosen has to undo a cache from whoever used this browser before,
        // or the browser would keep speaking their language to somebody who never
        // asked for it.
        None => {
            forget();
            set_locale(
                &from_browser()
                    .filter(|locale| is_available(locale))
                    .unwrap_or_else(|| {
                        let (fallback, _) = AVAILABLE[0];
                        fallback.to_string()
                    }),
            );
        }
    }
}

/// Whether a language was chosen, as against one being in force because the
/// browser asked for it. The screen that offers them needs to tell those apart.
pub fn chosen() -> bool {
    remembered().is_some()
}

/// Remembers a language, or unremembers one, and reloads — which is what applies
/// either.
pub fn choose(locale: Option<&str>) {
    match locale {
        Some(locale) => remember(locale),
        None => forget(),
    }

    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

fn remember(locale: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(REMEMBERED, locale);
    }
}

fn forget() {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(REMEMBERED);
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
