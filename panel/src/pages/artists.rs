// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Everybody credited on anything, and how much of them there is.
//!
//! Three things about an artist, so a row rather than a card: their name, how many
//! records and how many songs. Nothing to look at, which is why this is the one
//! collection screen that fills a wide window by running two columns of rows
//! instead of by making anything bigger.
//!
//! Those columns fill across rather than down — the second name is beside the first,
//! not under it. Read on its own that is the wrong way round for an alphabetical
//! list, and with a list that grows as it is scrolled it is the only way round that
//! works: columns that filled downwards would have to be rebalanced end to end every
//! time another fifty names arrived, so the name somebody was reading would move.
//!
//! No picture of them here, though the listing says whether there is one. A face is
//! worth a lot at the top of an artist's own page and nothing at all in a list of
//! nine hundred names being scanned for one of them.

use super::endless::{Fetch, Foot, Reel};
use crate::api;
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use rust_i18n::t;
use tocata::types::Artist;

fn window(search: String, offset: usize, limit: i64) -> super::endless::Window<Artist> {
    Box::pin(async move {
        api::artists(&search, offset, limit)
            .await
            .map(|page| (page.total, page.artists))
    })
}

#[component]
pub fn Artists(on_expired: Callback<()>) -> impl IntoView {
    let reel = Reel::new(window as Fetch<Artist>, on_expired);

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.artists")}</h1>
                <p class="quiet lead">{move || held(reel.total.get())}</p>
            </div>

            <div class="finding">
                <label class="search">
                    <Glyph icon=Icon::Search />
                    <input
                        type="search"
                        placeholder=t!("artists.search")
                        prop:value=reel.typing
                        on:input:target=move |e| reel.typed(e.target().value())
                    />
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

        <Show when=move || reel.total.get().is_some_and(|held| held == 0)>
            <p class="nothing">{t!("artists.none_found")}</p>
        </Show>

        <ul class="crowd">
            <For each=move || reel.rows.get() key=|artist| artist.id.clone() let:artist>
                <li>
                    <span class="what">{artist.name}</span>
                    // Both figures carry their word. The frame leaves it off the
                    // second, and with no heading over these columns a bare number
                    // at the end of a row is a number nobody can name.
                    <span class="figure">{records(artist.albums)}</span>
                    <span class="figure">{songs(artist.tracks)}</span>
                </li>
            </For>
        </ul>

        <Foot
            shown=reel.shown()
            total=reel.total
            fetching=reel.fetching
            on_reach=Callback::new(move |()| reel.more())
        />
    }
}

/// How many there are, for the line under the title.
fn held(total: Option<i64>) -> String {
    match total {
        None => t!("common.loading").to_string(),
        Some(1) => t!("artists.one").to_string(),
        Some(count) => t!("artists.many", count = super::thousands(count)).to_string(),
    }
}

/// How many records somebody is on. Both spellings written out, because rust-i18n
/// interpolates and does not pluralise.
fn records(count: i64) -> String {
    if count == 1 {
        t!("collection.one_album").to_string()
    } else {
        t!("collection.many_albums", count = super::thousands(count)).to_string()
    }
}

fn songs(count: i64) -> String {
    if count == 1 {
        t!("collection.one_track").to_string()
    } else {
        t!("collection.many_tracks", count = super::thousands(count)).to_string()
    }
}
