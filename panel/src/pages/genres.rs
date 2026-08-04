// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the collection is made of, counted.
//!
//! The plainest of the four: a word and how many songs wear it. Two columns of rows
//! like the artists, for the same reason and with the same caveat about which way
//! they fill.
//!
//! Two things it does not have, both because a genre is the one thing in the
//! collection that is not an object. It has no identifier of its own — the name is
//! the name — and there is no page behind it, so its rows do not tint under the
//! pointer and do not bleed past their text. A row that lights up when you reach it
//! has promised something, and here there would be nothing to give.
//!
//! And one figure rather than the artists' two. A genre lives on a song: how many
//! records are "partly folk" is a number about tagging rather than about music.

use super::endless::{Fetch, Foot, Reel};
use crate::api;
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use rust_i18n::t;
use tocata::types::Genre;

fn window(search: String, offset: usize, limit: i64) -> super::endless::Window<Genre> {
    Box::pin(async move {
        api::genres(&search, offset, limit)
            .await
            .map(|page| (page.total, page.genres))
    })
}

#[component]
pub fn Genres(admin: bool, on_expired: Callback<()>) -> impl IntoView {
    let reel = Reel::new(window as Fetch<Genre>, on_expired);

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.genres")}</h1>
                <p class="quiet lead">{move || held(reel.total.get())}</p>
            </div>

            <div class="finding">
                <label class="search">
                    <Glyph icon=Icon::Search />
                    <input
                        type="search"
                        placeholder=t!("genres.search")
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

        // Only where nothing arrived at all. A list that stopped partway says so in
        // its own foot, beside the way to carry on.
        {move || {
            reel.failure
                .get()
                .filter(|_| reel.rows.with(Vec::is_empty))
                .map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        // Never one answer, so never one sentence. Which of them it is lives in
        // `endless`, because all four screens have the same four.
        <Show when=move || reel.total.get().is_some_and(|held| held == 0)>
            <super::endless::Nothing
                which=super::endless::Listing::Genres
                typing=reel.typing
                held=reel.held
                on_clear=Callback::new(move |()| reel.clear())
                admin
            />
        </Show>

        <ul class="kinds">
            // Keyed by the name, which is all a genre has: the column is unique, so
            // it is the identifier as well as the label.
            <For each=move || reel.rows.get() key=|genre| genre.name.clone() let:genre>
                <li>
                    <span class="what">{genre.name}</span>
                    <span class="figure">{songs(genre.tracks)}</span>
                </li>
            </For>
        </ul>

        <Foot
            shown=reel.shown()
            total=reel.total
            fetching=reel.fetching
            stumbled=Signal::derive(move || {
                reel.failure.with(Option::is_some) && !reel.rows.with(Vec::is_empty)
            })
            on_reach=Callback::new(move |()| reel.more())
            on_retry=Callback::new(move |()| reel.again())
        />
    }
}

/// How many there are, for the line under the title.
fn held(total: Option<i64>) -> String {
    match total {
        None => t!("common.loading").to_string(),
        Some(1) => t!("genres.one").to_string(),
        Some(count) => t!("genres.many", count = super::thousands(count)).to_string(),
    }
}

fn songs(count: i64) -> String {
    if count == 1 {
        t!("collection.one_track").to_string()
    } else {
        t!("collection.many_tracks", count = super::thousands(count)).to_string()
    }
}
