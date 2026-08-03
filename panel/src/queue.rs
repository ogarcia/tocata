// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What is coming after this.
//!
//! One thing in two shapes: a drawer down the right of a window, and the whole of a
//! phone's screen. Not two components — the same element, laid out twice, because it
//! holds exactly the same list either way and a second copy would be a second place
//! for the list to be wrong.
//!
//! **The queue is identifiers and this draws rows**, so the titles have to be fetched
//! before anything can be shown. Not one request per track: the listing narrows by a
//! named handful, so the whole visible run of the queue arrives at once. And only the
//! visible run — nobody reorders the three hundredth track, so what is drawn is the
//! next fifty and the heading says how many there are altogether.
//!
//! Nothing here is a request to the server. A queue is this browser's own idea of what
//! to play next: taking a track out of it changes what sounds in a minute and nothing
//! else, which is why it needs no saving and no undoing.

use crate::api;
use crate::icon::{Glyph, Icon};
use crate::pages;
use crate::player::Player;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::Track;

/// How many of the tracks to come are drawn.
///
/// Enough that scrolling the drawer shows what a sitting will be, few enough that it
/// is one request. Somebody wanting to see the four hundredth track in a shuffle is
/// not doing anything the queue is for.
const SHOWN: usize = 50;

/// How tall a row in the queue is, which is what turns a gesture's distance into a
/// number of rows to move by.
///
/// Written here as well as in the stylesheet, which is the one thing in this module
/// that has to be kept in step with it: 12px of padding either side of two lines at
/// 1.3, which is the shape every other row in the panel is. Being a little out only
/// means a long drag lands a row early or late, so this is worth knowing rather than
/// worth measuring on every move.
const ROW: f64 = 61.0;

/// The queue, over whatever was on screen.
#[component]
pub fn Queue(open: RwSignal<bool>) -> impl IntoView {
    let player = crate::player::player();

    // The rows for what is coming, each with where it sits in the queue — which is
    // what the buttons on it act by, since a title is not an identity: the same track
    // can be queued twice.
    let coming = RwSignal::new(Vec::<(usize, Track)>::new());

    // Fetched when the drawer opens and again whenever the queue changes under it,
    // which is what fills the gap left by a track taken out.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }

        let at = player.at.get();
        let ids: Vec<String> = player
            .queue
            .with(|queue| queue.iter().skip(at + 1).take(SHOWN).cloned().collect());

        spawn_local(async move {
            // A failure leaves the drawer holding what it had. There is nothing to
            // say about it that is worth a message where the music carries on.
            if let Ok(rows) = api::some_tracks(&ids).await {
                coming.set(
                    rows.into_iter()
                        .enumerate()
                        .map(|(nth, track)| (at + 1 + nth, track))
                        .collect(),
                );
            }
        });
    });

    // How many are still to come, and which record they came out of. The count is of
    // the whole queue rather than of the rows below it: fifty rows drawn out of four
    // hundred to come is worth saying.
    let ahead = move || {
        let left = player.ahead();

        let counted = if left == 1 {
            t!("queue.one_ahead").to_string()
        } else {
            t!("queue.many_ahead", count = pages::thousands(left as i64)).to_string()
        };

        match player.now.get().and_then(|track| track.album) {
            Some(album) => format!("{counted} · {}", t!("player.from", album = album)),
            None => counted,
        }
    };

    view! {
        <Show when=move || open.get()>
            // Catches anything aimed at the screen behind, which on a wide window is
            // most of the screen. Nothing on a phone, where the queue covers the lot.
            <div class="veil" on:click=move |_| open.set(false)></div>

            <div class="queue">
                <header>
                    <div>
                        <h2>{t!("queue.heading")}</h2>
                        <p class="quiet">{ahead}</p>
                    </div>

                    <button class="tap" title=t!("common.close") on:click=move |_| open.set(false)>
                        <Glyph icon=Icon::Close />
                    </button>
                </header>

                <div class="rolling">
                    // What is sounding, apart from the rest: it is the one row that is
                    // not waiting, and the only one carrying a time.
                    <Show when=move || player.now.get().is_some()>
                        <p class="lettering">{t!("queue.playing")}</p>

                        <div class="row now">
                            <span class="what">
                                <span>
                                    {move || {
                                        player
                                            .now
                                            .get()
                                            .map(|track| track.title)
                                            .unwrap_or_default()
                                    }}
                                </span>
                                <span class="by">
                                    {move || {
                                        player
                                            .now
                                            .get()
                                            .and_then(|track| track.artists)
                                            .unwrap_or_default()
                                    }}
                                </span>
                            </span>

                            <span class="figure">
                                {move || {
                                    format!(
                                        "{} / {}",
                                        pages::length(player.elapsed.get() as i64),
                                        pages::length(player.duration.get() as i64),
                                    )
                                }}
                            </span>
                        </div>
                    </Show>

                    <Show when=move || !coming.with(Vec::is_empty)>
                        <p class="lettering next">{t!("queue.next")}</p>

                        <ul class="waiting">
                            <For
                                each=move || coming.get()
                                key=|(index, track)| format!("{index}-{}", track.id)
                                let:queued
                            >
                                <Row at=queued.0 track=queued.1 player />
                            </For>
                        </ul>
                    </Show>

                    // The end of a queue rather than an empty one: something is
                    // playing and there is nothing behind it.
                    <Show when=move || coming.with(Vec::is_empty) && player.now.get().is_some()>
                        <p class="nothing">{t!("queue.nothing_ahead")}</p>
                    </Show>
                </div>
            </div>
        </Show>
    }
}

/// How far a row has to be pushed sideways before letting go drops it.
///
/// Far enough that a thumb travelling down the list does not throw a track out by
/// brushing past it, near enough that the gesture does not need the whole width.
const SWIPED: f64 = 96.0;

/// One track waiting: what it is, the way to drop it, and the two gestures.
#[component]
fn Row(at: usize, track: Track, player: Player) -> impl IntoView {
    // Two gestures on one row, each with its own axis, so neither has to work out
    // whether it is the one being asked for.
    let swipe = crate::drag::Drag::new();
    let hold = crate::drag::Drag::new();

    // Pushed sideways to be dropped. It follows the finger only outwards: dragging a
    // row back past where it started would be dragging it the other way, and rows do
    // not go anywhere in that direction.
    let pushed = move || {
        let across = swipe.across().min(0.0);
        format!("transform: translateX({across}px); opacity: {}", {
            let gone = (across.abs() / (SWIPED * 2.0)).min(0.6);
            1.0 - gone
        })
    };

    // Held to be moved. Rows are a fixed height in this drawer, so where it would land
    // is how many of them the finger has travelled — no measuring, and no asking the
    // browser what is under the pointer.
    //
    // Given the distance rather than reading it, because by the time the gesture has
    // ended the offset is back to nought: that is what springs a short drag back.
    let landing = move |down: f64| {
        let rows = (down / ROW).round() as i64;
        (at as i64 + rows).max(player.at.get() as i64 + 1) as usize
    };

    let lifted = move || format!("transform: translateY({}px)", hold.down());

    view! {
        <li
            class:sliding=move || swipe.going.get()
            class:lifted=move || hold.going.get()
            style=move || if hold.going.get() { lifted() } else { pushed() }
            on:pointerdown=move |e: web_sys::PointerEvent| {
                swipe.begin(&e);
            }
            on:pointermove=move |e: web_sys::PointerEvent| swipe.moved(&e)
            on:pointerup=move |_| {
                // Far enough out and it goes; anything short of that springs back,
                // which is what `end` clearing the offset does on its own.
                if let Some((across, _)) = swipe.end()
                    && across <= -SWIPED
                {
                    player.drop_at(at);
                }
            }
            on:pointercancel=move |_| {
                swipe.end();
            }
        >
            // What the row is dragged by, and the only part of it that takes a
            // vertical gesture: everywhere else on the row that gesture is the list
            // being scrolled.
            <button
                class="handle"
                title=t!("queue.reorder")
                on:pointerdown=move |e: web_sys::PointerEvent| {
                    e.stop_propagation();
                    hold.begin(&e);
                }
                on:pointermove=move |e: web_sys::PointerEvent| hold.moved(&e)
                on:pointerup=move |_| {
                    if let Some((_, down)) = hold.end() {
                        player.move_in_queue(at, landing(down));
                    }
                }
                on:pointercancel=move |_| {
                    hold.end();
                }
            >
                <Glyph icon=Icon::Handle />
            </button>

            <span class="what">
                <span>{track.title}</span>
                <span class="by">{track.artists.unwrap_or_else(|| pages::MISSING.to_string())}</span>
            </span>

            <span class="figure">
                {track.duration.map(pages::length).unwrap_or_else(|| pages::MISSING.to_string())}
            </span>

            // Takes it out of what is coming. Not a deletion of anything: the track
            // stays in the collection and this only changes what sounds next. The
            // same thing the swipe does, for a pointer that has no swipe.
            <button class="drop" title=t!("queue.drop") on:click=move |_| player.drop_at(at)>
                <Glyph icon=Icon::Close />
            </button>
        </li>
    }
}
