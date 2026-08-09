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
use leptos_router::components::A;
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
///
/// One number, read two ways — as a margin for the observer and as a distance to
/// measure against by hand — because the two have to agree about where "nearly
/// reached" is.
const AHEAD: f64 = 600.0;

/// The same distance as the observer wants it written.
fn ahead() -> String {
    format!("{AHEAD}px")
}

/// Whether the foot of the list is close enough to the bottom of the window to be
/// worth filling past.
///
/// The same question the observer answers, asked by hand, because the observer only
/// answers it when the answer changes. Measured against the window and not against
/// whatever box is doing the scrolling, for the same reason the observer is used at
/// all: which box that is differs by screen.
fn within_reach(node: &web_sys::Element) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };

    let height = window
        .inner_height()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or_default();

    reached(node.get_bounding_client_rect().top(), height)
}

/// The arithmetic of the above, apart from the browser so that it can be checked.
///
/// `top` is where the foot of the list sits relative to the top of the window, which
/// is negative once it has been scrolled past.
fn reached(top: f64, window_height: f64) -> bool {
    top <= window_height + AHEAD
}

/// One window of a listing: how many there are in total, and the rows themselves.
pub type Window<T> = Pin<Box<dyn Future<Output = Result<(i64, Vec<T>), Failure>>>>;

/// How a screen fetches a window: what the listing is narrowed by, from where, and
/// how many.
///
/// What narrows it is what was typed, on the four screens that have a search field.
/// On the one listing that has no field — a genre's tracks, inside its panel — it is
/// the genre, which is fixed when the panel opens and never changes after. Either way
/// it is the one thing the window is *about*, which is what the machinery here needs:
/// a change of it starts the listing over, and every answer says which one it belongs
/// to so a late one cannot land on the wrong list.
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
    /// How many there were when nothing had been typed.
    ///
    /// What a search that matched nothing says as its reassurance — "all 1 842 albums
    /// are still there" — and the only place it can come from without a second call:
    /// the first window of every screen is fetched with an empty field, so the figure
    /// has already been through here.
    pub held: RwSignal<Option<i64>>,
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
        Self::about(String::new(), window, expired)
    }

    /// The same, for a listing that is about one thing and has no field to type in.
    ///
    /// The subject is handed to the fetcher exactly as a search would be, so a panel
    /// listing a genre's tracks gets the debouncing it will never use and the two
    /// things it does need: a window at a time, and a refetch when a scan changes what
    /// there is.
    pub fn about(subject: String, window: Fetch<T>, expired: Callback<()>) -> Self {
        let reel = Self {
            rows: RwSignal::new(Vec::new()),
            total: RwSignal::new(None),
            fetching: RwSignal::new(false),
            failure: RwSignal::new(None),
            held: RwSignal::new(None),
            typing: RwSignal::new(String::new()),
            asked: StoredValue::new(subject),
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

        // A scan changes what there is, so a listing refetches when one finishes rather
        // than sitting on the answer from before it. Which matters most on the screen
        // saying the collection is empty: pressing the scan it offers would otherwise
        // fill the database and leave that sentence on screen.
        after_a_scan(move || reel.fetch(true));

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
    /// Called by the foot of the list being within reach, which is asked both when it
    /// comes into view and again after every window arrives — see [`Foot`] for why
    /// the second is not the same question as the first.
    pub fn more(&self) {
        if !self.fetching.get_untracked() && !self.all_of_it() {
            self.fetch(false);
        }
    }

    /// Asks again for whatever failed.
    ///
    /// Which window that is depends on how far it got: nothing on screen means the
    /// first one never arrived, and rows on screen mean the list stopped partway. The
    /// second case must not start over — everything already read stays where it is,
    /// which is the whole difference between a list that broke and a list that stopped.
    pub fn again(&self) {
        self.failure.set(None);
        self.fetch(self.rows.with_untracked(Vec::is_empty));
    }

    /// Clears the search and asks for everything again.
    pub fn clear(&self) {
        let reel = *self;

        if let Some(pending) = reel.waiting.get_value() {
            pending.take().clear();
        }

        reel.typing.set(String::new());
        reel.search(String::new());
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

                    // Only while nothing is being searched for, which is what makes it
                    // the count of everything rather than of whatever was last typed.
                    if reel.asked.get_value().is_empty() {
                        reel.held.set(Some(total));
                    }
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

/// Runs something each time a scan finishes.
///
/// Keyed on the finishing stamp changing rather than on the running flag going from true
/// to false, and that is not a nicety. Ten files are scanned in well under a second, so a
/// panel watching for the flag can easily never observe it true at all — which is exactly
/// how the button offering the scan came to say "Scanning…" for ever on a small library,
/// while the list behind it stayed empty. The stamp is written once per run and pushed by
/// the same code that clears the flag, so it is seen whether or not anybody saw the
/// middle.
fn after_a_scan(and_then: impl Fn() + 'static) {
    let Some(status) = use_context::<ReadSignal<Option<tocata::types::Status>>>() else {
        return;
    };

    Effect::new(move |seen: Option<Option<String>>| {
        let finished = status.get().and_then(|status| status.finished_at);

        // The first run only writes down what it found. A panel connecting long after a
        // scan is sent that scan's stamp straight away, and old news is not news.
        if let Some(before) = seen
            && before != finished
            && finished.is_some()
        {
            and_then();
        }

        finished
    });
}

/// Which of the four listings this is, for the sentences that have to name it.
///
/// The states below are the same shape on all four screens and the same words on none
/// of them: "no album matches" and "nobody here matches" are the same state, and a
/// panel that said "no results" would be a panel that had given up explaining.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    Tracks,
    Albums,
    Artists,
    Genres,
}

impl Listing {
    /// How many of this kind the server holds altogether, whoever is asking.
    ///
    /// Read from the figures rather than from the listing, and that difference is the
    /// whole diagnosis: a listing that came back empty while the server holds thousands
    /// is an account that may not reach them, and one that came back empty because
    /// there are none is a collection with nothing of that kind in it.
    fn held_by_the_server(self, stats: &tocata::types::Stats) -> i64 {
        match self {
            Self::Tracks => stats.tracks,
            Self::Albums => stats.albums,
            Self::Artists => stats.artists,
            Self::Genres => stats.genres,
        }
    }

    /// What it says when the server has none of this kind at all — which for three of
    /// the four is a fact about the tags rather than about the music.
    fn none_of_this_kind(self) -> String {
        match self {
            Self::Tracks => t!("empty.no_tracks"),
            Self::Albums => t!("empty.no_albums"),
            Self::Artists => t!("empty.no_artists"),
            Self::Genres => t!("empty.no_genres"),
        }
        .to_string()
    }

    /// And what the figure beside "tracks read" is counting.
    fn with_a_tag(self) -> String {
        match self {
            Self::Tracks => t!("empty.with_a_title"),
            Self::Albums => t!("empty.with_an_album"),
            Self::Artists => t!("empty.with_an_artist"),
            Self::Genres => t!("empty.with_a_genre"),
        }
        .to_string()
    }

    fn nothing_matches(self) -> String {
        match self {
            Self::Tracks => t!("empty.no_track_matches"),
            Self::Albums => t!("empty.no_album_matches"),
            Self::Artists => t!("empty.no_artist_matches"),
            Self::Genres => t!("empty.no_genre_matches"),
        }
        .to_string()
    }

    /// The reassurance under it: that narrowing a list did not lose anything.
    fn all_still_there(self, held: i64) -> String {
        let count = super::thousands(held);

        match self {
            Self::Tracks => t!("empty.tracks_still_there", count = count),
            Self::Albums => t!("empty.albums_still_there", count = count),
            Self::Artists => t!("empty.artists_still_there", count = count),
            Self::Genres => t!("empty.genres_still_there", count = count),
        }
        .to_string()
    }
}

/// Why a listing has nothing in it, which is never one answer.
///
/// "Empty" is four different things needing four different things done about them: a
/// server nothing has been read into, a search that narrowed too far, a collection
/// whose files never said what they are, and an account that may not reach any of it.
/// A single "nothing found" would be the panel knowing which and not saying.
///
/// Nothing here is a card floating mid-page. It sits where the first row would have
/// been, on the same hairline block every run of rows in this panel starts with.
#[component]
pub fn Nothing(
    which: Listing,
    /// What is in the search field, which is what tells a narrowed list from an empty
    /// collection. The pieces of the reel rather than the reel itself: this component
    /// draws no rows, so making it generic over them would be a type parameter earning
    /// nothing.
    typing: RwSignal<String>,
    /// How many there were before anything was typed.
    held: RwSignal<Option<i64>>,
    on_clear: Callback<()>,
    /// Whether the account may do anything about it. Only an administrator is offered
    /// a scan, and only they are told which directory is being read — a listener with
    /// nothing to look at needs a sentence they can repeat, not a path.
    admin: bool,
) -> impl IntoView {
    // Asked for here rather than by the screen, because this is the only thing that
    // wants it and it only exists when a listing came back empty. The figures are not
    // filtered by which libraries an account may see, which is what makes them the
    // measure the diagnosis needs: the server's own total against yours.
    let stats = RwSignal::new(None::<tocata::types::Stats>);
    let libraries = RwSignal::new(Vec::<tocata::types::Library>::new());

    spawn_local(async move {
        if let Ok(read) = crate::api::stats().await {
            stats.set(Some(read));
        }
    });

    // Only for whoever could act on it, and only to name the directory on a first run.
    if admin {
        spawn_local(async move {
            if let Ok(read) = crate::api::libraries().await {
                libraries.set(read);
            }
        });
    }

    // Whether this account can reach any music at all, asked of the one listing that
    // answers it for all four: no tracks means no albums, no artists and no genres
    // either, whatever the figures say the server holds.
    //
    // It is what stops the wrong story being told. A listener given nothing, opening
    // Albums on a server whose files carry no album tags, would otherwise read that the
    // files say nothing — true of the server, and not why *she* is looking at an empty
    // screen. Asked only here, so it costs one request in the one case that needs it.
    let reaches = RwSignal::new(None::<i64>);

    spawn_local(async move {
        if let Ok(page) = crate::api::tracks("", 0, 1).await {
            reaches.set(Some(page.total));
        }
    });

    let searching = move || !typing.with(String::is_empty);

    view! {
        <div class="diagnosis">
            <Show when=searching>
                <h2>{move || which.nothing_matches()}</h2>

                <p class="quiet">
                    {move || {
                        which.all_still_there(held.get().unwrap_or_default())
                    }}
                </p>

                // The one action an empty search has, and it is the whole of what is
                // wrong: a filter is on. No link to another section — without carrying
                // the words across it would only be the sidebar again.
                <button class="plainly" on:click=move |_| on_clear.run(())>
                    {t!("empty.clear_the_search")}
                </button>
            </Show>

            <Show when=move || !searching() && stats.get().is_some() && reaches.get().is_some()>
                {move || {
                    let Some(read) = stats.get() else { return ().into_any() };
                    let within_reach = reaches.get().unwrap_or_default();

                    // Nothing has been read at all, which is every server on its first
                    // run and the only empty state with something to press.
                    if read.tracks == 0 {
                        return view! { <Unread admin libraries /> }.into_any();
                    }

                    // There is music and this account reaches none of it, which is two
                    // different situations wanting opposite sentences. An administrator
                    // reaches everything that is switched on, so for them it means every
                    // library is off — something they did and may well have forgotten. A
                    // listener cannot see the switch, so for them it is about access.
                    if within_reach == 0 {
                        if admin {
                            return view! {
                                <h2>{t!("empty.all_switched_off")}</h2>
                                <p class="quiet">{t!("empty.switch_one_back_on")}</p>

                                <div class="remedy">
                                    <A href="/libraries" attr:class="plainly">
                                        {t!("nav.libraries")}
                                    </A>
                                </div>
                            }
                                .into_any();
                        }

                        return view! {
                            <h2>{t!("empty.nothing_you_reach")}</h2>
                            <p class="quiet">{t!("empty.ask_an_administrator")}</p>
                        }
                            .into_any();
                    }

                    // The server holds none of this kind though it holds music, so the
                    // files never said. Nothing to press: the fix is in the tags.
                    if which.held_by_the_server(&read) == 0 {
                        return view! {
                            <h2>{which.none_of_this_kind()}</h2>
                            <p class="quiet">{t!("empty.outside_tocata")}</p>

                            <dl class="facts">
                                <div>
                                    <dt>{t!("empty.tracks_read")}</dt>
                                    <dd>{super::thousands(read.tracks)}</dd>
                                </div>
                                <div>
                                    <dt>{which.with_a_tag()}</dt>
                                    <dd>{super::thousands(0)}</dd>
                                </div>
                            </dl>
                        }
                            .into_any();
                    }

                    // Music within reach, some of this kind on the server, and none of it
                    // in this listing. Nothing known says why, so it says only that.
                    view! { <h2>{which.none_of_this_kind()}</h2> }.into_any()
                }}
            </Show>
        </div>
    }
}

/// The first run: a server with libraries and nothing read out of them.
#[component]
fn Unread(admin: bool, libraries: RwSignal<Vec<tocata::types::Library>>) -> impl IntoView {
    // Whether a scan is running, from the stream the whole panel already listens to
    // rather than from a flag of this component's own. A flag set on the press and never
    // cleared is exactly what this said before: "Scanning…" for ever, whatever the
    // server was doing. Read from the stream it cannot say that — and it also says so
    // when the scan was started from somewhere else.
    let status = use_context::<ReadSignal<Option<tocata::types::Status>>>();
    let running =
        move || status.is_some_and(|status| status.get().is_some_and(|status| status.scanning));

    // The gap between pressing and the stream saying so, which is a moment of network
    // and would otherwise show as the label flicking back to an invitation.
    let asked = RwSignal::new(false);

    // Cleared once a scan is actually running, so what holds the label from then on is
    // the stream; a scan that finishes takes the whole sentence away with it, because
    // the listing refetches and stops being empty.
    Effect::new(move |_| {
        if running() {
            asked.set(false);
        }
    });

    let scan = move |_| {
        asked.set(true);

        spawn_local(async move {
            // A refusal here is why this is not simply set and forgotten: the button has
            // to come back, or the one action an empty collection offers is spent.
            if crate::api::start_scan(false).await.is_err() {
                asked.set(false);
            }
        });
    };

    let scanning = Signal::derive(move || asked.get() || running());

    view! {
        <h2>{t!("empty.nothing_yet")}</h2>

        // Which directory, in monospace, because the useful question on a first run is
        // whether it points where the music actually is. Only for whoever can change
        // it: a listener reading a path has been told where somebody else's disk is.
        <Show
            when=move || admin
            fallback=|| view! { <p class="quiet">{t!("empty.nothing_added_yet")}</p> }
        >
            <p class="quiet">
                {move || {
                    let held = libraries.get();

                    // Read and empty is not the same as never read, and it is the
                    // commoner of the two: `TOCATA_LIBRARY_PATHS` registers a library and
                    // the server scans on start, so a directory with nothing in it has
                    // already been walked by the time anybody looks at this. Saying it
                    // has never been read would send somebody to press a button that has
                    // already been pressed, and leave them none the wiser.
                    //
                    // Every one of them, not any: with one library read and another added
                    // since and never scanned, the thing worth saying is that something
                    // has not been read yet — because that is the one somebody can do
                    // something about. "Reading them found nothing" is only true once
                    // there is nothing left to read.
                    //
                    // Which is also why the plural sentence says "not all of them have
                    // been read" rather than "none of them has": with a mix, none would
                    // be false of the one that was.
                    let read = held.iter().all(|one| one.last_scanned_at.is_some());

                    match (held.len(), read) {
                        (0, _) => t!("empty.no_libraries").to_string(),
                        (1, false) => {
                            t!("empty.one_library", path = held[0].path.clone()).to_string()
                        }
                        (1, true) => {
                            t!("empty.one_library_read", path = held[0].path.clone()).to_string()
                        }
                        (count, false) => {
                            t!("empty.many_libraries", count = super::thousands(count as i64))
                                .to_string()
                        }
                        (count, true) => {
                            t!(
                                "empty.many_libraries_read",
                                count = super::thousands(count as i64),
                            )
                                .to_string()
                        }
                    }
                }}
            </p>

            <div class="remedy">
                <Show when=move || !libraries.with(Vec::is_empty)>
                    <button
                        class="pill solid"
                        disabled=move || scanning.get()
                        on:click=scan
                    >
                        {move || {
                            if scanning.get() {
                                t!("empty.scanning").to_string()
                            } else {
                                t!("empty.scan_now").to_string()
                            }
                        }}
                    </button>
                </Show>

                <A href="/libraries" attr:class="plainly">{t!("nav.libraries")}</A>
            </div>
        </Show>
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
    /// Whether the last window did not arrive. Shown here rather than at the top of the
    /// screen, which is where it used to go: a list that stopped partway is not a broken
    /// screen, and everything already read stays on it. The message belongs where the
    /// reading stopped, with the way to carry on.
    stumbled: Signal<bool>,
    on_reach: Callback<()>,
    on_retry: Callback<()>,
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
        options.set_root_margin(&ahead());

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

    // And asked again after every window lands, which is not the same question the
    // observer answers.
    //
    // An observer reports a *change*: something came into view, something left. The
    // foot of a list that never left the screen is never reported again — and a list
    // whose window does not fill the screen leaves it there for good. That was the
    // artists screen: rows of one line in two columns, so a hundred names fitted on a
    // large display, the page did not overflow, there was no scrolling to be done, and
    // the listing stopped at a hundred of eight hundred with no way to ask for more.
    //
    // Songs and albums never showed it. Fifty rows of either overflow any screen, so
    // the foot always went out of view and scrolling always brought it back.
    //
    // After the paint rather than during it: what is being asked is where the foot
    // ended up, and during the effect the rows that push it down are not laid out yet.
    Effect::new(move |_| {
        // The dependency, and the point: this runs again each time the list grows.
        let _ = shown.get();
        let Some(node) = edge.get() else { return };

        request_animation_frame(move || {
            if within_reach(&node) {
                on_reach.run(());
            }
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
            // One line and a way to carry on, and no alarm anywhere: the page is not
            // broken, one request is. It says how far it got, because that is what turns
            // "it failed" into something somebody can act on.
            <Show when=move || stumbled.get() && !fetching.get()>
                <span>
                    {move || {
                        t!(
                            "collection.stopped_at",
                            shown = super::thousands(shown.get() as i64),
                        )
                    }}
                </span>

                <button class="plainly" on:click=move |_| on_retry.run(())>
                    {t!("collection.try_again")}
                </button>
            </Show>

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

#[cfg(test)]
mod tests {
    use super::{AHEAD, ahead, reached};

    /// The case the artists screen ran into: a hundred names of one line each, in two
    /// columns, on a display tall enough to hold them. The page does not overflow, so
    /// there is no scrolling to be done and the observer has nothing left to report —
    /// and the foot is still sitting there in plain view, above the fold.
    #[test]
    fn a_foot_that_fits_on_the_screen_is_within_reach() {
        assert!(
            reached(900.0, 1440.0),
            "a hundred artists on a tall display"
        );
        assert!(reached(1440.0, 1440.0), "exactly at the bottom edge");
    }

    /// And the ordinary case, which is why the distance exists: not on screen yet, but
    /// close enough that the rows should be on their way.
    #[test]
    fn a_foot_just_below_the_fold_is_within_reach_too() {
        assert!(reached(1500.0, 1440.0));
        assert!(reached(1440.0 + AHEAD, 1440.0), "at the far edge of it");
    }

    /// A long list still to be read through. Asking for more here would fetch the
    /// whole collection on the way down.
    #[test]
    fn a_foot_far_below_is_not() {
        assert!(!reached(1440.0 + AHEAD + 1.0, 1440.0));
        assert!(!reached(12_000.0, 1440.0), "fifty songs to scroll through");
    }

    /// Scrolled past, which reads as a negative offset.
    #[test]
    fn a_foot_already_gone_by_is_reached() {
        assert!(reached(-2000.0, 1440.0));
    }

    /// The observer's margin and the measurement have to mean the same distance: one
    /// number, written two ways.
    #[test]
    fn the_margin_says_the_distance_it_measures() {
        assert_eq!(ahead(), format!("{AHEAD}px"));
        assert_eq!(ahead(), "600px");
    }
}
