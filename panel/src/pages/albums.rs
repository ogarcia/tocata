// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The records, as their covers.
//!
//! A grid rather than the table the tracks are, because a record is a thing people
//! recognise by sight before they read its name — and because the four columns a
//! table would give it are three columns of nothing much: an album is its cover, its
//! name and who made it.
//!
//! The grid fills whatever width there is with as many columns as fit rather than
//! a set number of them, so the same markup is two covers wide on a phone and eight
//! on a monitor with nothing said about either.
//!
//! How the list is read a window at a time, and how it narrows as it is typed into,
//! is in [`super::endless`].

use super::endless::{Fetch, Foot, Reel};
use crate::api;
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use rust_i18n::t;
use tocata::types::Album;

fn window(search: String, offset: usize, limit: i64) -> super::endless::Window<Album> {
    Box::pin(async move {
        api::albums(&search, offset, limit)
            .await
            .map(|page| (page.total, page.albums))
    })
}

#[component]
pub fn Albums(on_expired: Callback<()>) -> impl IntoView {
    let reel = Reel::new(window as Fetch<Album>, on_expired);

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.albums")}</h1>
                <p class="quiet lead">{move || held(reel.total.get())}</p>
            </div>

            <div class="finding">
                <label class="search">
                    <Glyph icon=Icon::Search />
                    <input
                        type="search"
                        placeholder=t!("albums.search")
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
            <p class="nothing">{t!("albums.none_found")}</p>
        </Show>

        <div class="shelves">
            <For each=move || reel.rows.get() key=|album| album.id.clone() let:album>
                <Sleeve album />
            </For>
        </div>

        <Foot
            shown=reel.shown()
            total=reel.total
            fetching=reel.fetching
            on_reach=Callback::new(move |()| reel.more())
        />
    }
}

/// One record: its cover, its name, and the line under it.
#[component]
fn Sleeve(album: Album) -> impl IntoView {
    // Who made it and when, joined only where there is something on both sides of
    // the dot. A record with no year should read as the artist's, not as the
    // artist's followed by a dangling separator.
    let under = match (album.artist, album.year) {
        (Some(who), Some(year)) => format!("{who} · {year}"),
        (Some(who), None) => who,
        (None, Some(year)) => year.to_string(),
        (None, None) => super::MISSING.to_string(),
    };

    view! {
        <div class="sleeve">
            // Asked for only where the listing says there is one, so a shelf of
            // records with no art is not a shelf of requests for pictures that are
            // not there. And lazily, because a window of fifty covers on a grid two
            // rows tall is forty-odd images nobody has scrolled to yet.
            {if album.cover {
                view! {
                    <img class="art" src=api::cover(&album.id) alt="" loading="lazy" />
                }
                    .into_any()
            } else {
                view! {
                    <span class="art">
                        <Glyph icon=Icon::Albums />
                    </span>
                }
                    .into_any()
            }}

            <span class="what">
                {album.name} <span>{under}</span>
            </span>
        </div>
    }
}

/// How many there are, for the line under the title.
fn held(total: Option<i64>) -> String {
    match total {
        None => t!("common.loading").to_string(),
        Some(1) => t!("albums.one").to_string(),
        Some(count) => t!("albums.many", count = super::thousands(count)).to_string(),
    }
}
