// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A list read a window at a time, as somebody scrolls it.
//!
//! The four screens of the collection are the same list of decisions wearing four
//! different rows: fifty at a time rather than pages with a Next under them, a
//! search that answers as it is typed, and a foot that says more is coming. So the
//! decisions live here once and each screen brings its own rows and its own call.
//!
//! What a screen has to do with this is small: hand over a way to fetch one window,
//! draw whatever [`Reel::rows`] holds, and put [`Foot`] under it.
//!
//! Two things in here are less obvious than they look, and both are about time.
//!
//! Typing is not asking. Every keystroke would be a request, five of them for a
//! word, four of whose answers are thrown away by the time they land — so a search
//! is sent a quarter of a second after the last key, and the pending one is
//! cancelled rather than left to fire late.
//!
//! And answers can arrive in any order. Each carries which search it belongs to, so
//! the one that lands is only merged if it is still the search on screen. Without
//! that, a slow answer to "dra" arriving after a fast one to "drake" would leave
//! the rows disagreeing with the field they were typed into.

use crate::api::Failure;
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use send_wrapper::SendWrapper;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

/// How many rows one request brings back. Enough to fill a tall window once, so
/// arriving on a screen takes one request rather than three.
const PAGE: i64 = 50;

/// How long after the last keystroke the search is actually asked.
///
/// Long enough that a word typed at speed is one request and not six, short enough
/// that it still reads as the list answering while you type rather than after you
/// stop.
const SETTLE: Duration = Duration::from_millis(250);

/// How far past the last row the next window is asked for.
///
/// In pixels, and it belongs in pixels: it is not a measure of anything on screen
/// but the distance at which fetching should start, and what makes it the right
/// distance is how fast a scroll travels rather than how large the reader's text
/// is. Roughly a screenful, so the rows are there before the foot of the list is.
const AHEAD: &str = "600px";

/// One window of a listing: how many there are in total, and the rows themselves.
pub type Window<T> = Pin<Box<dyn Future<Output = Result<(i64, Vec<T>), Failure>>>>;

/// How a screen fetches a window: what was searched for, from where, and how many.
///
/// A plain function pointer rather than a generic, so that [`Reel`] stays one type
/// however many screens use it. Each screen writes the three lines that wrap its
/// own call, which is what pins the row type down.
pub type Fetch<T> = fn(String, usize, i64) -> Window<T>;

/// A listing being read a window at a time.
pub struct Reel<T: Send + Sync + 'static> {
    /// Everything fetched so far, in the order the server listed it.
    pub rows: RwSignal<Vec<T>>,
    /// How many there are altogether, once the first answer has said so. What tells
    /// an endless list when to stop asking.
    pub total: RwSignal<Option<i64>>,
    /// Whether a window is on its way. The foot says so while it is.
    pub fetching: RwSignal<bool>,
    pub failure: RwSignal<Option<String>>,
    /// What is in the search field. A quarter of a second ahead of what has been
    /// asked of the server, and that gap is the whole reason there are two.
    pub typing: RwSignal<String>,

    /// What the server was last asked for.
    asked: StoredValue<String>,
    /// Which search the answers now in flight belong to.
    run: StoredValue<u64>,
    /// The keystroke waiting to become a search, held so the next one can cancel it.
    waiting: StoredValue<Option<SendWrapper<TimeoutHandle>>>,
    window: StoredValue<Fetch<T>>,
    expired: Callback<()>,
}

// Written out rather than derived. Every field is a handle — a signal, a stored
// value, a function pointer — and copying one copies the handle and not what it
// points at, whatever the rows turn out to be. `derive` cannot know that: it would
// ask the rows themselves to be `Copy`, which a row holding a `String` never is, and
// then a screen could not read its own listing twice.
impl<T: Send + Sync + 'static> Clone for Reel<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for Reel<T> {}

impl<T: Send + Sync + 'static> Reel<T> {
    /// Starts a listing and asks for its first window.
    ///
    /// The first one is asked for here rather than left to the foot coming into
    /// view, which is what asks for every window after it: the foot cannot ask for
    /// the first, because until there is something on screen there is nothing for it
    /// to sit under.
    pub fn new(window: Fetch<T>, expired: Callback<()>) -> Self {
        let reel = Self {
            rows: RwSignal::new(Vec::new()),
            total: RwSignal::new(None),
            fetching: RwSignal::new(false),
            failure: RwSignal::new(None),
            typing: RwSignal::new(String::new()),
            asked: StoredValue::new(String::new()),
            run: StoredValue::new(0),
            waiting: StoredValue::new(None),
            window: StoredValue::new(window),
            expired,
        };

        // A screen left behind while a keystroke is still pending would run its
        // search into signals nobody reads.
        let waiting = reel.waiting;
        on_cleanup(move || {
            if let Some(pending) = waiting.get_value() {
                pending.take().clear();
            }
        });

        reel.fetch(true);
        reel
    }

    /// Whether everything there is has been read. Not the same as an empty list:
    /// one is the end of a listing and the other is a search that matched none of
    /// it, and they say different things on screen.
    pub fn all_of_it(&self) -> bool {
        let reel = *self;
        reel.total
            .get()
            .is_some_and(|held| reel.rows.with(Vec::len) as i64 >= held)
    }

    /// How many are on screen, for the foot to count against the total.
    pub fn shown(&self) -> Signal<usize> {
        let rows = self.rows;
        Signal::derive(move || rows.with(Vec::len))
    }

    /// The next window, if there is one and nothing is already on its way.
    ///
    /// Called by the foot coming into view, which happens again every time the rows
    /// above it grow — so a window too short to be filled by one request keeps
    /// asking until it is.
    pub fn more(&self) {
        if !self.fetching.get_untracked() && !self.all_of_it() {
            self.fetch(false);
        }
    }

    /// A key was pressed in the search field. The search itself happens a quarter of
    /// a second after the last one.
    pub fn typed(&self, needle: String) {
        let reel = *self;
        reel.typing.set(needle.clone());

        if let Some(pending) = reel.waiting.get_value() {
            pending.take().clear();
        }

        let handle = set_timeout_with_handle(move || reel.search(needle), SETTLE).ok();
        reel.waiting.set_value(handle.map(SendWrapper::new));
    }

    /// Asks for what is in the field now.
    ///
    /// Bumping the run is what makes every answer already in flight irrelevant:
    /// whichever of them lands, it lands on a run that is no longer the one on
    /// screen and is dropped. Which is also why nothing here resets `fetching` —
    /// the request this starts owns it from now on, and the abandoned ones leave it
    /// alone on their way out.
    fn search(&self, needle: String) {
        self.asked.set_value(needle);
        self.run.update_value(|count| *count += 1);
        self.fetch(true);
    }

    /// One window, from wherever the list currently stops.
    ///
    /// `restart` is a search having changed. The rows are replaced when the answer
    /// arrives rather than when the key was pressed, so a list somebody is reading
    /// does not empty itself under them for a quarter of a second.
    fn fetch(&self, restart: bool) {
        let reel = *self;
        let needle = reel.asked.get_value();
        let from = if restart {
            0
        } else {
            reel.rows.with_untracked(Vec::len)
        };
        let mine = reel.run.get_value();

        reel.fetching.set(true);

        spawn_local(async move {
            match (reel.window.get_value())(needle, from, PAGE).await {
                Ok((total, rows)) => {
                    // The answer to a question nobody is asking any more.
                    if reel.run.get_value() != mine {
                        return;
                    }

                    reel.total.set(Some(total));
                    reel.failure.set(None);
                    reel.rows.update(|have| {
                        if restart {
                            have.clear();
                        }
                        have.extend(rows);
                    });
                }
                Err(Failure::Unauthenticated) => reel.expired.run(()),
                Err(why) => reel.failure.set(Some(super::said(&why))),
            }

            if reel.run.get_value() == mine {
                reel.fetching.set(false);
            }
        });
    }
}

/// The foot of a listing: that more is coming, how much of it there is, and the
/// thing whose coming into view fetches it.
///
/// It is one element because it is one thing. Reaching the line that says how far
/// down the list you are is the same event as needing the next fifty, so there is
/// nothing to keep in step between them.
///
/// It says both at once — turning, and "10 of 1 842" — because either alone is
/// worse. A spinner with no figures is a list with no bottom, and figures with
/// nothing turning look like the list has stopped there. Most of the time neither is
/// read: the rows are usually already on their way before this comes into view,
/// which is what the six hundred pixels above are for.
#[component]
pub fn Foot(
    shown: Signal<usize>,
    total: RwSignal<Option<i64>>,
    fetching: RwSignal<bool>,
    on_reach: Callback<()>,
) -> impl IntoView {
    let edge = NodeRef::<leptos::html::Div>::new();

    // Watched once the element exists. An observer rather than the scroll event:
    // scrolling fires per pixel and would have to work out which box is doing the
    // scrolling — the page's own here, the window's elsewhere — where this only has
    // to be told that the foot of the list came into view.
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
                            "collection.reading",
                            shown = super::thousands(shown.get() as i64),
                            total = super::thousands(total.get().unwrap_or_default()),
                        )
                    }}
                </span>
            </Show>
        </div>
    }
}
