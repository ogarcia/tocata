// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Which colour the panel picks things out in.
//!
//! Every accent in the stylesheet comes from one variable, so choosing one is
//! setting one attribute on the root element and letting CSS do the rest. The
//! colours themselves are not here: each is a rule keyed on that attribute, which
//! is what makes adding one a line of CSS.
//!
//! Two shades per accent, because one is not enough. A blue that reads on white is
//! too dark on black, so every rule names both and `light-dark()` picks — the same
//! mechanism the rest of the theme runs on.
//!
//! What is here is the list of names, and only so that the panel can offer them
//! and mark the one in force. A name it does not know falls back to the accent the
//! panel ships with, which is what makes a value stored by a newer panel harmless.

use leptos::prelude::*;

/// The chosen accent, for the one screen that offers them.
///
/// Wrapped rather than passed as a bare signal of a string, because context is
/// keyed by type and a bare one would be the same key as anything else that ever
/// wants to share a string.
#[derive(Clone, Copy)]
pub struct Accent(pub RwSignal<String>);

/// The ones there are, in the order they are offered.
///
/// The first is the one the stylesheet already uses, so choosing it is choosing
/// nothing: it is stored as no choice at all rather than as its own name, which
/// keeps "I never picked a colour" and "I picked the default" from being two
/// states that look the same and are not.
pub const AVAILABLE: [&str; 6] = ["blue", "teal", "green", "amber", "crimson", "plum"];

/// What no choice means.
pub const DEFAULT: &str = "blue";

/// The attribute the rules hang off, on the root element and on every swatch that
/// shows a colour. One selector, two uses: a swatch redefines the variable for
/// itself, so the button showing plum is plum whatever the panel is set to.
pub const ATTRIBUTE: &str = "data-accent";

/// Settles on one, without asking anybody: what the browser remembers from last
/// time, applied before the first paint.
pub fn settle() -> RwSignal<String> {
    let accent = RwSignal::new(remembered());
    apply(&accent.get_untracked());

    accent
}

/// Puts one into effect and remembers it here. Telling the server is the caller's
/// business, since this module has no session.
pub fn choose(accent: &RwSignal<String>, chosen: &str) {
    let chosen = known(chosen);

    accent.set(chosen.to_string());
    apply(chosen);
    remember(chosen);
}

/// Adopts what the server says this account chose, which may be nothing.
///
/// Applied and cached, so the next load of this browser starts in the right colour
/// instead of flashing the default on the way to it.
pub fn adopt(accent: &RwSignal<String>, chosen: Option<&str>) {
    let chosen = chosen.map_or(DEFAULT, known);

    accent.set(chosen.to_string());
    apply(chosen);
    remember(chosen);
}

/// On the root element, as an attribute rather than a style: the colours belong in
/// the stylesheet, and what goes here is only which of them.
///
/// The default is the absence of the attribute rather than a value of its own, so
/// what the stylesheet says with nothing set is what somebody who chose it gets.
fn apply(accent: &str) {
    let Some(root) = root() else { return };

    if accent == DEFAULT {
        let _ = root.remove_attribute(ATTRIBUTE);
    } else {
        let _ = root.set_attribute(ATTRIBUTE, accent);
    }
}

/// Anything unrecognised is the default, which is how a colour this panel has
/// never heard of stays harmless.
fn known(accent: &str) -> &str {
    AVAILABLE
        .iter()
        .find(|available| **available == accent)
        .copied()
        .unwrap_or(DEFAULT)
}

fn root() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.document_element()
}

const REMEMBERED: &str = "tocata.accent";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn remember(accent: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(REMEMBERED, accent);
    }
}

fn remembered() -> String {
    storage()
        .and_then(|storage| storage.get_item(REMEMBERED).ok().flatten())
        .map(|name| known(&name).to_string())
        .unwrap_or_else(|| DEFAULT.to_string())
}
