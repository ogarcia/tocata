// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The icons, compiled in one by one.
//!
//! From Tabler, under the MIT licence kept beside them in `icons/`. Only the ones
//! that get used are here: a crate holding all four thousand of them would have
//! to be trusted to leave the rest out of the bundle, and copying nine files is
//! cheaper than finding out whether it does.
//!
//! Every one draws with `stroke="currentColor"`, so an icon is the colour of the
//! text beside it and a section that lights up takes its icon with it. Nothing
//! here sets a colour and nothing has to.

use leptos::prelude::*;

/// Which one. Named for the thing it stands for rather than what it depicts, so
/// swapping the drawing is one line and touches nothing that uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Logo,
    Overview,
    Scan,
    Libraries,
    Accounts,
    Settings,
    Maintenance,
    Account,
    Menu,
    LogOut,
    Language,
}

impl Icon {
    /// The file itself, read at compile time.
    fn svg(self) -> &'static str {
        match self {
            Self::Logo => include_str!("../icons/logo.svg"),
            Self::Overview => include_str!("../icons/overview.svg"),
            Self::Scan => include_str!("../icons/scan.svg"),
            Self::Libraries => include_str!("../icons/libraries.svg"),
            Self::Accounts => include_str!("../icons/accounts.svg"),
            Self::Settings => include_str!("../icons/settings.svg"),
            Self::Maintenance => include_str!("../icons/maintenance.svg"),
            Self::Account => include_str!("../icons/account.svg"),
            Self::Menu => include_str!("../icons/menu.svg"),
            Self::LogOut => include_str!("../icons/logout.svg"),
            Self::Language => include_str!("../icons/language.svg"),
        }
    }
}

/// Draws one.
///
/// `inner_html` with markup of our own, fixed at compile time: there is no input
/// here for anybody to put anything into.
///
/// Hidden from anything that reads the page aloud, because every icon sits beside
/// its own label. Announcing both would say everything twice.
#[component]
pub fn Glyph(icon: Icon) -> impl IntoView {
    view! { <span class="icon" aria-hidden="true" inner_html=icon.svg()></span> }
}
