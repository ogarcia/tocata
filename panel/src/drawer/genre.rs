// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Everything under one word.
//!
//! The odd one of the four, because a genre is the one thing in the collection that is
//! not an object. It has no row of its own, no identifier and no picture: the name is
//! the whole of it, and every figure here is a question asked of the songs wearing it.
//!
//! Which is also why what it is made of arrives differently. A record's running order
//! is a dozen rows and comes with the record; a genre is as long as the collection is,
//! and "rock" on a real shelf is thousands of songs. So the list under the figures is
//! a listing like the ones the four screens draw — fifty at a time, more as it is
//! scrolled — and not a field of a single answer.
//!
//! It shows the files that have gone as well as the ones that are there, the same as a
//! record's running order and for the same reason: the figures above count what can be
//! played and say on their own line how much cannot.

use super::{Failed, Figure, Frame, Head, Open};
use crate::api;
use crate::icon::Icon;
use crate::pages;
use crate::pages::endless::{Fetch, Foot, Reel, Window};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{GenreDetail, Track};

/// One window of the songs wearing it.
///
/// The listing narrowed by a name where the four screens narrow it by what was typed,
/// which is the only difference between this and the songs screen.
fn window(genre: String, offset: usize, limit: i64) -> Window<Track> {
    Box::pin(async move {
        api::tracks_in(&genre, offset, limit)
            .await
            .map(|page| (page.total, page.tracks))
    })
}

#[component]
pub fn Genre(name: String) -> impl IntoView {
    let player = crate::player::player();
    let name = StoredValue::new(name);

    let detail = RwSignal::new(None::<GenreDetail>);
    let failure = RwSignal::new(None::<api::Failure>);

    spawn_local(async move {
        match api::genre(&name.get_value()).await {
            Ok(read) => detail.set(Some(read)),
            Err(why) => failure.set(Some(why)),
        }
    });

    // A session that went while the list was being scrolled. Said here rather than
    // bounced to the way in, which is what a screen does with it: a panel is a thing
    // over the screen you are on, and taking somebody off that screen because a
    // window of a list did not arrive is a larger answer than the question.
    let reel = Reel::about(
        name.get_value(),
        window as Fetch<Track>,
        Callback::new(move |()| failure.set(Some(api::Failure::Unauthenticated))),
    );

    // Shuffled, for the same reason a discography is: a genre played in the order it
    // is listed is one artist's whole record before anything else of it is reached,
    // and nobody opening a word meant that.
    let play = move |_| {
        let mine = name.get_value();

        spawn_local(async move {
            if let Ok(queue) = api::queue(
                api::Narrowing {
                    genre: Some(mine),
                    ..Default::default()
                },
                true,
                None,
            )
            .await
            {
                player.play(queue, 0);
            }
        });
    };

    view! {
        <Frame>
            <Head
                icon=Icon::Genres
                heading=Signal::derive(move || name.get_value())
                lead=move || detail.with(placing)
            />

            <div class="leafing">
                {move || failure.get().map(|why| view! { <Failed why /> })}
                {move || detail.get().map(|read| view! { <Said read /> })}

                // Only where nothing arrived at all, the same as on a screen: a list
                // that stopped partway says so in its own foot, beside the way on.
                {move || {
                    reel.failure
                        .get()
                        .filter(|_| reel.rows.with(Vec::is_empty))
                        .map(|why| view! { <p class="failure" role="alert">{why}</p> })
                }}

                <Show when=move || !reel.rows.with(Vec::is_empty)>
                    <p class="lettering">{t!("genre.what_wears_it")}</p>

                    <div class="wearing">
                        <For each=move || reel.rows.get() key=|track| track.id.clone() let:track>
                            <Row track />
                        </For>
                    </div>

                    <p class="quiet remark">{t!("genre.a_title_opens_it")}</p>
                </Show>

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
            </div>

            <footer>
                <span class="quiet">{t!("genre.from_the_tags")}</span>

                <span class="deeds">
                    <Show when=move || {
                        detail.with(|read| read.as_ref().is_some_and(|read| read.tracks > 0))
                    }>
                        <button class="leading" on:click=play>
                            {t!("genre.play_everything")}
                        </button>
                    </Show>
                </span>
            </footer>
        </Frame>
    }
}

/// What the database holds about it, which is only ever figures: a genre has no facts
/// of its own to spell out, since everything true of it is true of the songs below.
#[component]
fn Said(read: GenreDetail) -> impl IntoView {
    let gone = read.missing;

    view! {
        // Ruled underneath, which no other drawer's figures are: with no facts to
        // spell out under them, nothing here brings the line that closes the block.
        <div class="figures ruled">
            <Figure
                value=Some(pages::thousands(read.albums))
                name=t!("genre.albums").to_string()
            />
            <Figure
                value=Some(pages::thousands(read.tracks))
                name=t!("genre.tracks").to_string()
            />
            <Figure value=read.duration.map(pages::runs) name=t!("genre.length").to_string() />
            // Nothing rather than a nought where none of it has been played: a zero
            // here reads as a fact about the music and it is a fact about this server.
            <Figure
                value=(read.plays > 0).then(|| pages::thousands(read.plays))
                name=t!("genre.plays").to_string()
            />
        </div>

        // Said apart from the figures above, which count what can be played, the same
        // as a record's. A genre missing four of its files is not a genre with four
        // fewer songs.
        <Show when=move || { gone > 0 }>
            <p class="absent">
                {move || {
                    if gone == 1 {
                        t!("genre.one_gone").to_string()
                    } else {
                        t!("genre.many_gone", count = pages::thousands(gone)).to_string()
                    }
                }}
            </p>
        </Show>
    }
}

/// One song wearing it: what it is, who made it, how long it runs.
///
/// Who made it rather than where it sits on a record, which is the difference between
/// this list and a running order: these songs have no order and share nothing but the
/// word, so the name beside each is what tells them apart.
#[component]
fn Row(track: Track) -> impl IntoView {
    let player = crate::player::player();
    let id = StoredValue::new(track.id.clone());
    let sounding = move || player.current().as_deref() == Some(&id.get_value());

    view! {
        <div
            class:sounding=sounding
            class:gone=track.missing
            on:click=move |_| super::open(Open::Track(id.get_value()))
        >
            <span class="what">
                {track.title}
                <span class="by">
                    {track.artists.unwrap_or_else(|| pages::MISSING.to_string())}
                </span>
            </span>

            <span class="figure">
                {track.duration.map(pages::length).unwrap_or_else(|| pages::MISSING.to_string())}
            </span>
        </div>
    }
}

/// The line under the word: how much of it there is, and how many names it covers.
///
/// The same shape as an artist's, and read for the same reason: it answers "is there
/// much of this here" before the figures are looked at. How many names rather than how
/// many records, because that is the question a genre raises — a word covering one
/// artist is a word about that artist, and one covering forty is a shelf.
fn placing(read: &Option<GenreDetail>) -> String {
    let Some(read) = read else {
        return String::new();
    };

    if read.tracks == 1 {
        t!(
            "genre.one_track_by",
            artists = pages::thousands(read.artists)
        )
        .to_string()
    } else {
        t!(
            "genre.many_tracks_by",
            count = pages::thousands(read.tracks),
            artists = pages::thousands(read.artists)
        )
        .to_string()
    }
}
