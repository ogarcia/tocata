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
pub fn Albums(admin: bool, on_expired: Callback<()>) -> impl IntoView {
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
                which=super::endless::Listing::Albums
                typing=reel.typing
                held=reel.held
                on_clear=Callback::new(move |()| reel.clear())
                admin
            />
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
            stumbled=Signal::derive(move || {
                reel.failure.with(Option::is_some) && !reel.rows.with(Vec::is_empty)
            })
            on_reach=Callback::new(move |()| reel.more())
            on_retry=Callback::new(move |()| reel.again())
        />
    }
}

/// One record: its cover, its name, who made it, and what it is.
#[component]
fn Sleeve(album: Album) -> impl IntoView {
    let player = crate::player::player();
    let who = album.artist.unwrap_or_else(|| super::MISSING.to_string());

    let id = StoredValue::new(album.id.clone());

    // Whether asking for the cover came back with nothing, which is the only answer
    // that says a record has none: the listing cannot say it, since a record nothing
    // has looked at yet is indistinguishable there from one with no art at all.
    let broken = RwSignal::new(false);

    // Whether what is sounding came off this record. Read from the track playing
    // rather than remembered when play was pressed, so it is still right after the
    // queue has walked on to the next one — and right about a record reached from
    // anywhere else, since it is the same question either way.
    let sounding = move || {
        player
            .now
            .get()
            .and_then(|track| track.album_id)
            .is_some_and(|from| from == id.get_value())
    };

    // A record is a narrowing of its own, so the whole of it goes in the queue
    // however long it is — no sitting's cap here, unlike an unfiltered listing of
    // every track there is. Somebody who pressed play on a record asked for that
    // record.
    let start = move |event: web_sys::MouseEvent| {
        event.stop_propagation();

        if sounding() {
            player.toggle();
            return;
        }

        let mine = id.get_value();

        spawn_local(async move {
            if let Ok(queue) = api::queue("", Some(&mine), None, false, None).await {
                player.play(queue, 0);
            }
        });
    };

    // When it came out, how much of it there is, and how long it lasts — or that it
    // is sounding, which takes the place of the length rather than of the year: the
    // year is a fact about the record and does not stop being true while it plays.
    //
    // Joined rather than laid out, because a record with no year is not a record
    // whose year is blank: it reads as "5 tracks · 46:36" with nothing missing from
    // in front of it. Same for a record nothing on which has a length.
    let counted = if album.tracks == 1 {
        t!("collection.one_track").to_string()
    } else {
        t!(
            "collection.many_tracks",
            count = super::thousands(album.tracks)
        )
        .to_string()
    };

    let year = album.year.map(|year| year.to_string());
    let long = album.duration.map(super::runs);

    let facts = move || {
        let last = if sounding() {
            Some(t!("albums.playing").to_string())
        } else {
            long.clone()
        };

        [year.clone(), Some(counted.clone()), last]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    };

    view! {
        // Everywhere but the button over the cover opens what is known about the
        // record. The button keeps its own press, so playing never also opens.
        <div
            class="sleeve"
            on:click=move |_| crate::drawer::open(crate::drawer::Open::Album(id.get_value()))
        >
            // The cover, and over it the button that plays the record. The button is
            // its own target rather than the whole square, which the mockups do not
            // cover and which matters for what comes next: the rest of the cover is
            // what will open the record's own panel, and a play button filling it
            // would leave nowhere to press for that.
            <span class="cover">
                // Always asked for, and the answer is what settles it.
                //
                // The listing's own `cover` is not "there is one" — it is "one has
                // been found already", which is false for every record nothing has
                // looked at yet, because the looking is what asking does. So gating
                // the request on it meant a shelf of empty frames that filled in one
                // square at a time as something else happened to ask: in practice a
                // cover appeared once you had played the record, which is the one
                // moment you are not looking at the shelf.
                //
                // Still lazily, so a window of fifty covers two rows deep is not
                // fifty files opened on the server for pictures nobody has scrolled
                // to.
                <Show
                    when=move || !broken.get()
                    fallback=|| {
                        view! {
                            <span class="art">
                                <Glyph icon=Icon::Albums />
                            </span>
                        }
                    }
                >
                    <img
                        class="art"
                        src=api::cover(&id.get_value())
                        alt=""
                        loading="lazy"
                        on:error=move |_| broken.set(true)
                    />
                </Show>

                // Kept in view while this record is the one sounding, so a shelf says
                // which of it is playing without being pointed at — and so the way to
                // pause is where the way to play was.
                <button
                    class="over"
                    class:sounding=sounding
                    title=t!("player.play_record")
                    on:click=start
                >
                    <Show
                        when=move || sounding() && player.playing.get()
                        fallback=|| view! { <Glyph icon=Icon::Play /> }
                    >
                        <Glyph icon=Icon::Pause />
                    </Show>
                </button>
            </span>

            // Three lines: what it is called, who made it, and what it is. The last
            // two are inside the first because the three are one label for one
            // record, not three things that happen to sit under a picture.
            <span class="what" class:sounding=sounding>
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
