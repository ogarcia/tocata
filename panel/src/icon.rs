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
    Home,
    Scan,
    Chevron,
    Theme,
    Songs,
    Albums,
    Artists,
    Genres,
    Add,
    Remove,
    Rename,
    Folder,
    On,
    Off,
    Close,
    More,
    Rotate,
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
            Self::Home => include_str!("../icons/home.svg"),
            Self::Scan => include_str!("../icons/scan.svg"),
            Self::Chevron => include_str!("../icons/chevron.svg"),
            Self::Theme => include_str!("../icons/theme.svg"),
            Self::Songs => include_str!("../icons/songs.svg"),
            Self::Albums => include_str!("../icons/albums.svg"),
            Self::Artists => include_str!("../icons/artists.svg"),
            Self::Genres => include_str!("../icons/genres.svg"),
            Self::Add => include_str!("../icons/add.svg"),
            Self::Remove => include_str!("../icons/remove.svg"),
            Self::Rename => include_str!("../icons/rename.svg"),
            Self::Folder => include_str!("../icons/folder.svg"),
            // Books and books crossed out: a library is a shelf, and what the
            // button does to it reads without a word.
            Self::On => include_str!("../icons/on.svg"),
            Self::Off => include_str!("../icons/off.svg"),
            Self::Close => include_str!("../icons/close.svg"),
            Self::More => include_str!("../icons/more.svg"),
            // The same two arrows as a scan: both mean "go round again".
            Self::Rotate => include_str!("../icons/scan.svg"),
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
