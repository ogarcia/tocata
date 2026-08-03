// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Every track there is, in the order they sit on their records.
//!
//! A collection is the one thing in this panel that does not fit on a screen, and
//! the two decisions here both follow from that. It arrives fifty rows at a time
//! as somebody scrolls rather than as pages with a Next under them: a page number
//! is a thing to keep track of, and nobody looking for a song wants to keep track
//! of anything. And it narrows as it is typed into rather than when a button is
//! pressed, because the server answers a prefix — so the answer is already there
//! by the time somebody would have reached for the button.
//!
//! Typing is not asking, though. Every keystroke would be a request, five of them
//! for a word, four of whose answers are thrown away by the time they arrive. So a
//! search is asked a quarter of a second after the last key, and each answer says
//! which search it belongs to: the network is free to answer out of order, and the
//! rows on screen have to be the ones that were asked for last rather than the
//! ones that happened to come back last.

use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use send_wrapper::SendWrapper;
use std::time::Duration;
use tocata::types::Track;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

/// How many rows one request brings back. Enough to fill a tall window once, so
/// arriving on the screen takes one request rather than three.
const PAGE: i64 = 50;

/// How long after the last keystroke the search is actually asked.
///
/// Long enough that a word typed at speed is one request and not six, short
/// enough that it still reads as the list answering while you type rather than
/// after you stop.
const SETTLE: Duration = Duration::from_millis(250);

/// How far past the last row the next request is asked for.
///
/// In pixels, and it belongs in pixels: it is not a measure of anything on screen
/// but the distance at which fetching should start, and what makes it the right
/// distance is how fast a scroll travels rather than how large the reader's text
/// is. Roughly a screenful, so the rows are there before the bottom of the list
/// is.
const AHEAD: &str = "600px";

#[component]
pub fn Tracks(on_expired: Callback<()>) -> impl IntoView {
    // What is in the field, and what has actually been asked of the server. They
    // are apart by a quarter of a second, and that gap is the whole reason there
    // are two.
    let (typing, set_typing) = signal(String::new());
    let asked = StoredValue::new(String::new());

    let rows = RwSignal::new(Vec::<Track>::new());
    let (total, set_total) = signal(None::<i64>);
    let (failure, set_failure) = signal(None::<String>);
    let (fetching, set_fetching) = signal(false);

    // Which search the answers now in flight belong to. A search that has been
    // superseded has its answer dropped rather than merged: the rows have to be
    // the ones somebody last asked for.
    let run = StoredValue::new(0_u64);

    // Everything, and there is nothing more to ask for. Not the same as an empty
    // list: one is the end of the collection and the other is a search that found
    // nothing, and they say different things.
    let all_of_it = move || {
        total
            .get()
            .is_some_and(|held| rows.with(Vec::len) as i64 >= held)
    };

    // One window, from wherever the list currently stops.
    //
    // `restart` is a search having changed: the rows go when the answer comes back
    // rather than when the key was pressed, so the list somebody is reading does
    // not empty itself under them for a quarter of a second.
    let fetch = move |restart: bool| {
        let needle = asked.get_value();
        let from = if restart {
            0
        } else {
            rows.with_untracked(Vec::len)
        };
        let mine = run.get_value();

        set_fetching.set(true);

        spawn_local(async move {
            match api::tracks(&needle, from, PAGE).await {
                Ok(page) => {
                    // The answer to a question nobody is asking any more.
                    if run.get_value() != mine {
                        return;
                    }

                    set_total.set(Some(page.total));
                    set_failure.set(None);
                    rows.update(|have| {
                        if restart {
                            have.clear();
                        }
                        have.extend(page.tracks);
                    });
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(super::said(&why))),
            }

            if run.get_value() == mine {
                set_fetching.set(false);
            }
        });
    };

    // The next window, if there is one and nothing is already on its way. Called
    // from the sentinel at the foot of the list, which fires again every time the
    // rows it sits under grow — so a window too short to be filled by one request
    // keeps asking until it is.
    let more = move || {
        if !fetching.get_untracked() && !all_of_it() {
            fetch(false);
        }
    };

    // A search, once the typing has stopped. Bumping the run is what makes every
    // answer already in flight irrelevant: whichever of them lands, it lands on a
    // run that is no longer the one on screen and is dropped. Which is also why
    // nothing here has to reset `fetching` — the request this starts owns it from
    // now on, and the abandoned ones leave it alone on their way out.
    let search = move |needle: String| {
        asked.set_value(needle);
        run.update_value(|count| *count += 1);
        fetch(true);
    };

    // The pending keystroke, held so that the next one can cancel it. Without the
    // cancelling this is not a debounce: it is the same six requests, each a
    // quarter of a second later than it would have been.
    let waiting = StoredValue::new(None::<SendWrapper<TimeoutHandle>>);

    let typed = move |needle: String| {
        set_typing.set(needle.clone());

        if let Some(pending) = waiting.get_value() {
            pending.take().clear();
        }

        let handle = set_timeout_with_handle(move || search(needle), SETTLE).ok();
        waiting.set_value(handle.map(SendWrapper::new));
    };

    // A screen left behind while a keystroke is still pending would run the search
    // into signals nobody reads.
    on_cleanup(move || {
        if let Some(pending) = waiting.get_value() {
            pending.take().clear();
        }
    });

    // The first window. Everything after it is asked for by the sentinel, but the
    // first one cannot be: it is what puts the list on screen for the sentinel to
    // sit under.
    fetch(true);

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.tracks")}</h1>
                <p class="quiet lead">{move || held(total.get())}</p>
            </div>

            <div class="finding">
                <label class="search">
                    <Glyph icon=Icon::Search />
                    <input
                        type="search"
                        placeholder=t!("tracks.search")
                        prop:value=typing
                        on:input:target=move |e| typed(e.target().value())
                    />
                    // What the search left, beside the search itself. Only while
                    // there is one: with the field empty this is the figure the
                    // lead already carries, said twice.
                    <Show when=move || !typing.get().is_empty()>
                        <span class="found">
                            {move || super::thousands(total.get().unwrap_or_default())}
                        </span>
                    </Show>
                </label>
            </div>
        </header>

        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}

        // Nothing at all until the first answer, and then either the list or the
        // news that a search matched none of it.
        <Show when=move || total.get().is_some_and(|held| held == 0)>
            <p class="nothing">{t!("tracks.none_found")}</p>
        </Show>

        <Show when=move || !rows.with(Vec::is_empty)>
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
                    <For each=move || rows.get() key=|track| track.id.clone() let:track>
                        <Row track />
                    </For>
                </ul>
            </div>
        </Show>

        // What is on screen out of what there is, and the thing that asks for the
        // rest. One element for both: what says how far down the list you are is
        // exactly what has to be reached for it to grow.
        <Foot rows total fetching on_reach=Callback::new(move |()| more()) />
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
            <span class="figure">{track.duration.map(length).unwrap_or_else(|| super::MISSING.to_string())}</span>
        </li>
    }
}

/// The foot of the list: that more is coming, how much of it there is, and the
/// thing whose coming into view fetches it.
///
/// It is one element because it is one thing. Reaching the line that says how far
/// down the list you are is the same event as needing the next fifty, so there is
/// nothing to keep in step between them.
///
/// It says both at once — turning, and "6 of 1 200" — because either alone is
/// worse. A spinner with no figures is a list with no bottom, and figures with
/// nothing turning look like the list has stopped there. Most of the time neither
/// is read: the rows are usually already on their way before this comes into view,
/// which is what the six hundred pixels above are for.
#[component]
fn Foot(
    rows: RwSignal<Vec<Track>>,
    total: ReadSignal<Option<i64>>,
    fetching: ReadSignal<bool>,
    on_reach: Callback<()>,
) -> impl IntoView {
    let edge = NodeRef::<leptos::html::Div>::new();

    // Watched once the element exists. An observer rather than the scroll event:
    // scrolling fires per pixel and would have to work out which box is doing the
    // scrolling — here the page's own, elsewhere the window's — where this only
    // has to be told that the foot of the list came into view.
    Effect::new(move |_| {
        let Some(node) = edge.get() else { return };

        let reached = Closure::<dyn Fn(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let arrived = entries.iter().any(|entry| {
                entry
                    .dyn_into::<web_sys::IntersectionObserverEntry>()
                    .is_ok_and(|entry| entry.is_intersecting())
            });

            if arrived {
                on_reach.run(());
            }
        });

        let options = web_sys::IntersectionObserverInit::new();
        options.set_root_margin(AHEAD);

        let Ok(observer) = web_sys::IntersectionObserver::new_with_options(
            reached.as_ref().unchecked_ref(),
            &options,
        ) else {
            // Nothing to do about it from here, and nothing lost but the endless
            // part: the first window is on screen either way.
            return;
        };

        observer.observe(&node);

        // Both are handles into the browser, so neither is `Send` and `on_cleanup`
        // asks for one. There is a single thread here, so the wrapper never has
        // anything to refuse. Dropping the closure while the browser still holds a
        // pointer to it is what makes a page panic on the next scroll.
        let held = SendWrapper::new((observer, reached));

        on_cleanup(move || {
            let (observer, reached) = held.take();
            observer.disconnect();
            drop(reached);
        });
    });

    view! {
        // Empty when there is nothing on its way, and it keeps its height either
        // way: this is what has to be reached for the list to grow, and a box of no
        // height is never reached.
        //
        // Nothing is said once the whole list is here. The count is in the lead, and
        // a line under the last row repeating it would only be there to be read
        // after there is nothing left to read.
        <div class="brink" node_ref=edge>
            <Show when=move || fetching.get()>
                <Glyph icon=Icon::Loading />
                <span>
                    {move || {
                        t!(
                            "tracks.reading",
                            shown = super::thousands(rows.with(Vec::len) as i64),
                            total = super::thousands(total.get().unwrap_or_default()),
                        )
                    }}
                </span>
            </Show>
        </div>
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

/// Minutes and seconds, and hours only when there are any.
///
/// Zero-padded from the minutes down but never at the front: "3:44" rather than
/// "03:44", which is how a length is written everywhere it is read as one.
fn length(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::length;

    #[test]
    fn a_length_is_written_the_way_a_length_is_read() {
        assert_eq!(length(224), "3:44");
        assert_eq!(length(0), "0:00");
        assert_eq!(length(9), "0:09");
        assert_eq!(length(60), "1:00");
        // The minutes are padded once there are hours in front of them, and not
        // before: "1:2:03" is not a time.
        assert_eq!(length(3723), "1:02:03");
        assert_eq!(length(36_000), "10:00:00");
        // Nothing sends a negative length. If something did, the row would say
        // zero rather than an hour with a minus in the middle of it.
        assert_eq!(length(-5), "0:00");
    }
}
