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
    Scan,
    Chevron,
    /// Points at the screen a figure opens, on the two figures that open one.
    Arrow,
    Songs,
    Albums,
    Artists,
    Genres,
    Add,
    Remove,
    More,
    Rotate,
    Key,
    Search,
    Loading,
    /// The filled triangle a player is started with, as against the outlined one
    /// that stands for a count of plays.
    ///
    /// Its drawing is nudged left of Tabler's, which centres the triangle's canvas
    /// rather than the triangle: see the note in the file.
    Play,
    Pause,
    Previous,
    Next,
    /// On the row that is sounding, where another row has its number.
    Sounding,
    /// Takes something out of a list without destroying it, which is why it is a
    /// cross and not the bin that removes a thing for good.
    Close,
    /// What a row is dragged by. Two columns of dots, which is the one shape that
    /// says "hold me" without saying anything else.
    Handle,
    /// Two paths crossing: what is coming, in no order.
    Shuffle,
    Plays,
    Favourites,
    /// The same heart with its inside painted: what something marked wears. Two marks
    /// for one idea on purpose, the way play has an outline and a filled triangle —
    /// the state is the whole of what the button says, and a colour alone is not
    /// enough to carry it.
    Marked,
    Ratings,
    Playlists,
    Menu,
    LogOut,
    Alert,
}

impl Icon {
    /// The file itself, read at compile time.
    fn svg(self) -> &'static str {
        match self {
            Self::Alert => include_str!("../icons/alert.svg"),
            Self::Logo => include_str!("../icons/logo.svg"),
            Self::Scan => include_str!("../icons/scan.svg"),
            Self::Chevron => include_str!("../icons/chevron.svg"),
            Self::Arrow => include_str!("../icons/arrow.svg"),
            Self::Songs => include_str!("../icons/songs.svg"),
            Self::Albums => include_str!("../icons/albums.svg"),
            Self::Artists => include_str!("../icons/artists.svg"),
            Self::Genres => include_str!("../icons/genres.svg"),
            Self::Add => include_str!("../icons/add.svg"),
            Self::Remove => include_str!("../icons/remove.svg"),
            Self::More => include_str!("../icons/more.svg"),
            // The same two arrows as a scan: both mean "go round again".
            Self::Rotate => include_str!("../icons/scan.svg"),
            Self::Key => include_str!("../icons/key.svg"),
            Self::Search => include_str!("../icons/search.svg"),
            // An open arc rather than the scan's two arrows: what turns here is
            // one thing arriving, not a cycle going round again.
            Self::Loading => include_str!("../icons/loading.svg"),
            Self::Play => include_str!("../icons/play-solid.svg"),
            Self::Pause => include_str!("../icons/pause.svg"),
            Self::Previous => include_str!("../icons/previous.svg"),
            Self::Next => include_str!("../icons/next.svg"),
            Self::Sounding => include_str!("../icons/sounding.svg"),
            Self::Close => include_str!("../icons/close.svg"),
            Self::Handle => include_str!("../icons/handle.svg"),
            Self::Shuffle => include_str!("../icons/shuffle.svg"),
            Self::Plays => include_str!("../icons/play.svg"),
            Self::Favourites => include_str!("../icons/heart.svg"),
            Self::Marked => include_str!("../icons/heart-solid.svg"),
            Self::Ratings => include_str!("../icons/star.svg"),
            Self::Playlists => include_str!("../icons/playlist.svg"),
            Self::Menu => include_str!("../icons/menu.svg"),
            Self::LogOut => include_str!("../icons/logout.svg"),
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
