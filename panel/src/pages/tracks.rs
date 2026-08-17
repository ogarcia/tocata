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
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::Track;

/// How much music is enough, when nothing has been narrowed down.
///
/// Five hundred tracks is about a day of it. Somebody who presses play on the whole
/// collection meant "play me some music" rather than "queue up every hour of it",
/// and somebody who searched first has said what they want and gets all of it. The
/// cap is here rather than in the API on purpose: it is a reading of what pressing
/// this particular button meant, which belongs to the screen that drew it.
const A_SITTING: i64 = 500;

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
pub fn Tracks(admin: bool, on_expired: Callback<()>) -> impl IntoView {
    let reel = Reel::new(window as Fetch<Track>, on_expired);

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.tracks")}</h1>
                <p class="quiet lead">{move || held(reel.total.get())}</p>
            </div>

            <div class="finding">
                <super::Search
                    placeholder=t!("tracks.search")
                    value=reel.typing
                    on_type=Callback::new(move |typed| reel.typed(typed))
                    on_clear=Callback::new(move |()| reel.clear())
                />
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

        // Nothing at all until the first answer, and then either the list or the
        // news that a search matched none of it.
        // Never one answer, so never one sentence. Which of them it is lives in
        // `endless`, because all four screens have the same four.
        <Show when=move || reel.total.get().is_some_and(|held| held == 0)>
            <super::endless::Nothing
                which=super::endless::Listing::Tracks
                typing=reel.typing
                held=reel.held
                on_clear=Callback::new(move |()| reel.clear())
                admin
            />
        </Show>

        <Table reel starred=false />

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

/// The tracks as a table, wherever the tracks are being listed.
///
/// Here and on the screen about your own favourites, which is this same table over a
/// listing narrowed to what you have marked — so it belongs to whoever owns the shape
/// of a track row rather than being written out twice.
///
/// `starred` is what that screen is: it decides the fifth column, since when you
/// marked something is the only new fact there and a genre is not what anybody came
/// for, and it decides what pressing play queues up.
#[component]
pub(super) fn Table(reel: Reel<Track>, starred: bool) -> impl IntoView {
    view! {
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
                    // Over the column of numbers and play buttons, which is a
                    // column of controls and has no name.
                    <span></span>
                    <span>{t!("tracks.title")}</span>
                    <span>{t!("tracks.artist")}</span>
                    <span>{t!("tracks.album")}</span>
                    <span>
                        {if starred {
                            t!("favourites.marked").to_string()
                        } else {
                            t!("tracks.genre").to_string()
                        }}
                    </span>
                    <span class="figure">{t!("tracks.length")}</span>
                </div>

                <ul class="listing">
                    <For each=move || reel.rows.get() key=|track| track.id.clone() let:track>
                        <Row track reel starred />
                    </For>
                </ul>
            </div>
        </Show>
    }
}

/// One track, as a row.
#[component]
fn Row(track: Track, reel: Reel<Track>, starred: bool) -> impl IntoView {
    let player = crate::player::player();

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

    // Where it sits on its record, or nothing at all. `unwrap_or_default` on the
    // number itself would print a nought, which is a track number no record has —
    // and a file with no tags at all, which is how the tones this was tested
    // against arrived, would have every row claiming to be track zero.
    let number = track
        .track_number
        .map(|number| number.to_string())
        .unwrap_or_default();

    let id = StoredValue::new(track.id.clone());
    let sounding = move || player.current().as_deref() == Some(&id.get_value());

    // Playing a row plays the whole listing from that row down, which is what "play
    // what you are looking at" means: the queue is everything the search matched,
    // not the fifty rows that happen to be fetched, and it starts where you pressed.
    //
    // Where nothing has been narrowed, it stops at a sitting's worth. Somebody who
    // pressed play on an unfiltered collection meant "play me some music", not
    // "queue up nine hundred hours of it" — and somebody who searched first has said
    // what they want, so they get all of it.
    let start = move |event: web_sys::MouseEvent| {
        // The row itself opens the panel about this track, so the button has to keep
        // its press to itself: pressing play must never also open something.
        event.stop_propagation();

        if sounding() {
            player.toggle();
            return;
        }

        let needle = reel.typing.get_untracked();
        let mine = id.get_value();

        spawn_local(async move {
            // Never a cap on what somebody marked: a hundred favourites are a
            // hundred choices already made, where a hundred hours of unfiltered
            // collection are nobody's choice at all.
            let cap = (!starred && needle.is_empty()).then_some(A_SITTING);
            let narrowing = api::Narrowing {
                search: needle,
                starred,
                ..Default::default()
            };

            if let Ok(queue) = api::queue(narrowing, false, cap).await {
                // Where the row sits in that queue, which is not where it sits on
                // screen: a missing track has no button but still holds a row, so the
                // two can differ by the time you are far enough down.
                let from = queue.iter().position(|track| *track == mine).unwrap_or(0);
                player.play(queue, from);
            }
        });
    };

    view! {
        // Two gestures on one row and two targets for them: the first column plays it,
        // and everywhere else on the row opens what is known about it. A row that both
        // played and opened would be a row where neither could be had on its own.
        <li
            class:gone=track.missing
            class:sounding=sounding
            on:click=move |_| {
                crate::drawer::open(crate::drawer::Open::Track(id.get_value()))
            }
        >
            // A missing file gets its number and no button: it cannot be played, and
            // it is exactly the row somebody wants to look at.
            {if track.missing {
                view! {
                    <span class="starter">{number}</span>
                }
                    .into_any()
            } else {
                view! {
                    <button
                        class="starter"
                        title=t!("player.play_this")
                        on:click=start
                    >
                        // At rest, its number — or the speaker, on the row that is
                        // sounding, which is how that row says so from across the
                        // table without being pointed at.
                        //
                        // Over it, the glyph that takes its place when the pointer
                        // is on the row: stacked rather than swapped so the column
                        // never changes width, and carrying the paper so what is
                        // behind does not show through.
                        <span class="resting">
                            <Show
                                when=sounding
                                fallback=move || view! { {number.clone()} }
                            >
                                <Glyph icon=Icon::Sounding />
                            </Show>
                        </span>
                        <span class="acting">
                            <Show
                                when=move || sounding() && player.playing.get()
                                fallback=|| view! { <Glyph icon=Icon::Play /> }
                            >
                                <Glyph icon=Icon::Pause />
                            </Show>
                        </span>
                    </button>
                }
                    .into_any()
            }}

            <span class="what">{track.title}</span>
            <span class="by">{credited}</span>
            <span class="from">{track.album.unwrap_or_else(|| super::MISSING.to_string())}</span>
            // The genre, or when you marked it on the screen that is about that. The
            // same cell either way, so the table keeps its columns: a favourite is the
            // collection filtered, not a wider thing to draw.
            <span class="kind">
                {match (starred, track.starred_at) {
                    (true, Some(at)) => super::since(&at),
                    (true, None) => super::MISSING.to_string(),
                    (false, _) => track.genre.unwrap_or_else(|| super::MISSING.to_string()),
                }}
            </span>
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
