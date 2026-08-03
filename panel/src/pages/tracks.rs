// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Every track there is, in the order they sit on their records.
//!
//! A table, because five things about a song are five columns and reading down one
//! of them is how somebody finds anything here. How it is read a window at a time,
//! and how it narrows as it is typed into, is in [`super::endless`] — this screen
//! brings the rows and the words for them.

use super::endless::{Fetch, Foot, Reel};
use crate::api;
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use rust_i18n::t;
use tocata::types::Track;

/// One window of the tracks. Wrapping the call is what pins the row type down for
/// [`Reel`], which is otherwise the same machinery on all four screens.
fn window(search: String, offset: usize, limit: i64) -> super::endless::Window<Track> {
    Box::pin(async move {
        api::tracks(&search, offset, limit)
            .await
            .map(|page| (page.total, page.tracks))
    })
}

#[component]
pub fn Tracks(on_expired: Callback<()>) -> impl IntoView {
    let reel = Reel::new(window as Fetch<Track>, on_expired);

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.tracks")}</h1>
                <p class="quiet lead">{move || held(reel.total.get())}</p>
            </div>

            <div class="finding">
                <label class="search">
                    <Glyph icon=Icon::Search />
                    <input
                        type="search"
                        placeholder=t!("tracks.search")
                        prop:value=reel.typing
                        on:input:target=move |e| reel.typed(e.target().value())
                    />
                    // What the search left, beside the search itself. Only while
                    // there is one: with the field empty this is the figure the
                    // lead already carries, said twice.
                    <Show when=move || !reel.typing.with(String::is_empty)>
                        <span class="found">
                            {move || super::thousands(reel.total.get().unwrap_or_default())}
                        </span>
                    </Show>
                </label>
            </div>
        </header>

        {move || {
            reel.failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        // Nothing at all until the first answer, and then either the list or the
        // news that a search matched none of it.
        <Show when=move || reel.total.get().is_some_and(|held| held == 0)>
            <p class="nothing">{t!("tracks.none_found")}</p>
        </Show>

        <Show when=move || !reel.rows.with(Vec::is_empty)>
            // No box of its own scrolling sideways here, unlike the roster of
            // accounts: the heading has to stay put while the rows go past it, and
            // a sticky element resolves against the nearest scrolling ancestor —
            // which, inside a box that scrolls horizontally, is a box that does not
            // scroll vertically, so it would do nothing at all. The block is pulled
            // out by the twelve pixels the rows bleed instead, and the columns fold
            // into one line before they run out of room, so nothing has to be
            // dragged sideways.
            <div class="bled">
                <div class="listing-head">
                    <span>{t!("tracks.title")}</span>
                    <span>{t!("tracks.artist")}</span>
                    <span>{t!("tracks.album")}</span>
                    <span>{t!("tracks.genre")}</span>
                    <span class="figure">{t!("tracks.length")}</span>
                </div>

                <ul class="listing">
                    <For each=move || reel.rows.get() key=|track| track.id.clone() let:track>
                        <Row track />
                    </For>
                </ul>
            </div>
        </Show>

        <Foot
            shown=reel.shown()
            total=reel.total
            fetching=reel.fetching
            on_reach=Callback::new(move |()| reel.more())
        />
    }
}

/// One track, as a row.
#[component]
fn Row(track: Track) -> impl IntoView {
    // A file that is not where it was. The row stays — a scan marks rather than
    // deletes, and a track nobody can find is still a track somebody owns — and
    // it says so where a row says who made it, because that is the line that has
    // room for a clause.
    let credited = match (track.artists, track.missing) {
        (Some(who), true) => format!("{who} · {}", t!("tracks.missing")),
        (Some(who), false) => who,
        (None, true) => t!("tracks.missing").to_string(),
        (None, false) => super::MISSING.to_string(),
    };

    view! {
        <li class:gone=track.missing>
            <span class="what">{track.title}</span>
            <span class="by">{credited}</span>
            <span class="from">{track.album.unwrap_or_else(|| super::MISSING.to_string())}</span>
            <span class="kind">{track.genre.unwrap_or_else(|| super::MISSING.to_string())}</span>
            <span class="figure">
                {track.duration.map(super::length).unwrap_or_else(|| super::MISSING.to_string())}
            </span>
        </li>
    }
}

/// How many there are, for the line under the title.
///
/// Both spellings are written out because rust-i18n interpolates and does not
/// pluralise, and a collection of one track is what somebody sees the first time
/// they point Tocata at a directory to try it.
fn held(total: Option<i64>) -> String {
    match total {
        None => t!("common.loading").to_string(),
        Some(1) => t!("tracks.one").to_string(),
        Some(count) => t!("tracks.many", count = super::thousands(count)).to_string(),
    }
}
