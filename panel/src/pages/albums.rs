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
use leptos::task::spawn_local;
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

/// One record: its cover, its name, who made it, and what it is.
#[component]
fn Sleeve(album: Album) -> impl IntoView {
    let player = crate::player::player();
    let who = album.artist.unwrap_or_else(|| super::MISSING.to_string());

    // A record is a narrowing of its own, so the whole of it goes in the queue
    // however long it is — no sitting's cap here, unlike an unfiltered listing of
    // every track there is. Somebody who pressed play on a record asked for that
    // record.
    let id = StoredValue::new(album.id.clone());
    let start = move |_| {
        let mine = id.get_value();

        spawn_local(async move {
            if let Ok(queue) = api::queue("", Some(&mine), None, false, None).await {
                player.play(queue, 0);
            }
        });
    };

    // When it came out, how much of it there is and how long it lasts, joined with
    // dots — and joined rather than laid out, because a record with no year is not
    // a record whose year is blank: it reads as "5 tracks · 46:36" with nothing
    // missing from in front of it. Same for a record nothing on which has a length.
    let facts = [
        album.year.map(|year| year.to_string()),
        Some(if album.tracks == 1 {
            t!("collection.one_track").to_string()
        } else {
            t!(
                "collection.many_tracks",
                count = super::thousands(album.tracks)
            )
            .to_string()
        }),
        album.duration.map(super::runs),
    ];

    let facts = facts.into_iter().flatten().collect::<Vec<_>>().join(" · ");

    view! {
        <div class="sleeve">
            // The cover, and over it the button that plays the record. The button is
            // its own target rather than the whole square, which the mockups do not
            // cover and which matters for what comes next: the rest of the cover is
            // what will open the record's own panel, and a play button filling it
            // would leave nowhere to press for that.
            <span class="cover">
                // Asked for only where the listing says there is one, so a shelf of
                // records with no art is not a shelf of requests for pictures that
                // are not there. And lazily, because a window of fifty covers on a
                // grid two rows tall is forty-odd images nobody has scrolled to yet.
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

                <button class="over" title=t!("player.play_record") on:click=start>
                    <Glyph icon=Icon::Play />
                </button>
            </span>

            // Three lines: what it is called, who made it, and what it is. The last
            // two are inside the first because the three are one label for one
            // record, not three things that happen to sit under a picture.
            <span class="what">
                {album.name} <span class="by">{who}</span> <span>{facts}</span>
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
