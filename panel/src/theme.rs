// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Light, dark, or whatever the machine says.
//!
//! One line of CSS does all of it. Every colour in the stylesheet comes from
//! `light-dark()`, which reads `color-scheme` to decide which of its two values
//! to use, so setting that property on the root element is the whole mechanism.
//! No second set of rules, no class on the body, nothing to keep in step.
//!
//! Unlike the language, this needs no reload: the browser recalculates the
//! moment the property changes.

use leptos::prelude::*;

/// What somebody can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Follow the machine, which is what it does with nothing set.
    Auto,
    Light,
    Dark,
}

impl Theme {
    /// As it is remembered, as it is read back, and as it is stored on the
    /// account: one name for all three, so nothing has to translate between them.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::Auto,
        }
    }

    /// What `color-scheme` has to say for this to happen. Naming both is what
    /// tells the browser to follow the machine.
    fn color_scheme(self) -> &'static str {
        match self {
            Self::Auto => "light dark",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// In the order a menu should offer them.
pub const AVAILABLE: [Theme; 3] = [Theme::Auto, Theme::Light, Theme::Dark];

const REMEMBERED: &str = "tocata.theme";

/// Applies what this browser saw last time, before the first paint.
///
/// A cache of what the account chose, not the choice itself: the choice lives on
/// the server and arrives with the session. Starting from the cache is what keeps
/// the panel from painting light and turning dark a moment later.
///
/// Two accounts sharing a browser will each see the other's theme for as long as
/// that answer takes to arrive. Nothing can be done about that without knowing who
/// is asking before asking, and a wrong theme for one paint is a smaller price
/// than a blank screen until the server answers.
pub fn settle() -> RwSignal<Theme> {
    let theme = RwSignal::new(remembered());
    apply(theme.get_untracked());

    theme
}

/// Remembers a choice and puts it into effect. Telling the server is the caller's
/// business: this module has no session and no reason to grow one.
pub fn choose(theme: RwSignal<Theme>, chosen: Theme) {
    theme.set(chosen);
    apply(chosen);
    remember(chosen);
}

/// Adopts what the server says this account chose, which may be nothing at all —
/// and nothing means following the machine, not the light theme.
pub fn adopt(theme: RwSignal<Theme>, chosen: Option<&str>) {
    let chosen = chosen.map_or(Theme::Auto, Theme::from_name);

    theme.set(chosen);
    apply(chosen);
    remember(chosen);
}

fn remember(theme: Theme) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(REMEMBERED, theme.name());
    }
}

/// On the root element rather than the body, because that is where the property
/// has to be for the whole document to inherit it — including the scrollbars and
/// the space behind everything, which is what betrays a half applied theme.
fn apply(theme: Theme) {
    let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    else {
        return;
    };

    let _ = root.set_attribute("style", &format!("color-scheme: {}", theme.color_scheme()));
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn remembered() -> Theme {
    storage()
        .and_then(|storage| storage.get_item(REMEMBERED).ok().flatten())
        .map(|name| Theme::from_name(&name))
        .unwrap_or(Theme::Auto)
}
