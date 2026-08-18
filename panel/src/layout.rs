// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The frame every screen sits in.
//!
//! One column down the left and the screen beside it. Everything that is
//! permanently there lives in that column: the name of the thing, where you can
//! go, a scan while one is running, and who you are. There used to be a header
//! across the top holding the last three of those, and it cost a strip of every
//! screen to hold what each of them has a place for down here.
//!
//! The sections are one list, in two runs under headings. They used to be two
//! lists that swapped — the server's, or your own account's, depending on where
//! you were — and swapping them meant the panel changed shape under somebody as
//! they walked into it. Your own account is reached from the block with your name
//! on it instead, which is where somebody looks for it anyway.
//!
//! Nothing here is a box. The column is divided from the screen by one line, and
//! the blocks at its foot from each other by one line each, and that is the whole
//! of the furniture.
//!
//! On a narrow screen the column slides in from the left behind a button in a bar
//! that exists only at that width. That was a `checkbox` and a CSS rule, on the
//! grounds that the browser already knows how to remember whether something is
//! open — but it does not know that you have navigated, so choosing a section left
//! the sections sitting over the screen they had just taken you to. Whatever holds
//! this has to hear about the one thing CSS cannot see, so it is a signal.

use crate::api;
use crate::icon::{Glyph, Icon};
use crate::pages;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use rust_i18n::t;
use tocata::types::{Account, Status};

/// A place to go, and what to call it.
///
/// The label is a function rather than a string so it is fetched at render time:
/// it has to come out in whatever language is in force.
struct Section {
    path: &'static str,
    label: fn() -> String,
    /// Whether the mark belongs to this entry only when the path is exactly this.
    /// Anything with sections under it needs this, or it lights up while somebody
    /// is inside one of its children and two entries claim to be where you are.
    exact: bool,
}

/// What anybody with a session can reach, above every heading.
///
/// On its own rather than under one, because it is not part of anything: it is the
/// screen you land on.
const EVERYONE: [Section; 1] = [Section {
    path: "/",
    label: || t!("nav.home").to_string(),
    exact: true,
}];

/// The music itself, four ways into the same collection.
///
/// Everybody sees these: a listener is here for the music, and an administrator
/// wants to look at what they are administering.
const COLLECTION: [Section; 4] = [
    Section {
        path: "/tracks",
        label: || t!("nav.tracks").to_string(),
        exact: false,
    },
    Section {
        path: "/albums",
        label: || t!("nav.albums").to_string(),
        exact: false,
    },
    Section {
        path: "/artists",
        label: || t!("nav.artists").to_string(),
        exact: false,
    },
    Section {
        path: "/genres",
        label: || t!("nav.genres").to_string(),
        exact: false,
    },
];

/// What is the account's own rather than the server's.
///
/// A heading of its own because the sections are grouped by whose the contents are:
/// the collection is the server's, and this is yours.
const YOURS: [Section; 2] = [
    Section {
        path: "/favourites",
        label: || t!("nav.favourites").to_string(),
        exact: false,
    },
    Section {
        path: "/playlists",
        label: || t!("nav.playlists").to_string(),
        exact: false,
    },
];

/// What only an administrator can reach.
///
/// Under a heading that says what they have in common — they are the server — which
/// is a word that fits all four where "settings" fitted two of them.
const SERVER: [Section; 4] = [
    Section {
        path: "/libraries",
        label: || t!("nav.libraries").to_string(),
        exact: false,
    },
    Section {
        path: "/accounts",
        label: || t!("nav.accounts").to_string(),
        // One account of somebody else's lives under this, and the list is where
        // you came from rather than where you are.
        exact: false,
    },
    Section {
        path: "/settings",
        label: || t!("nav.settings").to_string(),
        exact: false,
    },
    Section {
        path: "/maintenance",
        label: || t!("nav.maintenance").to_string(),
        exact: false,
    },
];

/// Where your own account starts. Everything under it is yours; nothing under it
/// is administration, including for an administrator.
pub const MINE_PATH: &str = "/account";

/// The parts of your own account, reached from your name rather than from the
/// sections.
const MINE: [Section; 3] = [
    Section {
        path: "/account",
        label: || t!("nav.profile").to_string(),
        // The other two live under this path, so without this it would be lit
        // while somebody is on either of them.
        exact: true,
    },
    Section {
        path: "/account/access",
        label: || t!("nav.access").to_string(),
        exact: false,
    },
    Section {
        path: "/account/preferences",
        label: || t!("nav.preferences").to_string(),
        exact: false,
    },
];

#[component]
pub fn Shell(
    admin: bool,
    on_out: Callback<()>,
    scan: ReadSignal<Option<Status>>,
    children: Children,
) -> impl IntoView {
    let (folded_out, fold) = signal(false);
    // Whether the player is open over the screen, and whether the queue is. Both
    // client-only and deliberately not routes: what is behind them keeps its scroll
    // and its search.
    let sheet = RwSignal::new(false);
    let queue = RwSignal::new(false);

    // What the collection has open over it, held here because it has to outlive every
    // screen: a panel about one track does not belong to the fifty rows that happened
    // to be fetched when it was opened.
    let opened: crate::drawer::Opened = RwSignal::new(None);
    provide_context(opened);

    // And how often a panel has changed a favourite mark, for the one screen that is
    // about those marks: it has to read itself again when one goes, or it keeps a row
    // that has stopped belonging to it.
    provide_context(crate::drawer::Marks(RwSignal::new(0)));

    // And the same for a playlist changed in its own panel, which the screen of lists
    // behind it has to hear about: its rows carry four figures and a date the server
    // works out.
    provide_context(crate::drawer::Lists(RwSignal::new(0)));

    // One thing over the screen at a time. Both of these are drawers on the same edge,
    // and two of them stacked is a thing to unstack rather than to read — so whichever
    // was asked for last is the one that is there.
    //
    // Two effects and not a ping-pong: shutting the queue does not answer the second,
    // which only fires while it is open, and closing a panel does not answer the first.
    //
    // **The player at full height is not one of them, on purpose.** It is not a drawer
    // on that edge — it is the whole of a phone's screen, and on a phone it is where a
    // panel is opened from: the title and the credit on it both lead to one. Closing it
    // on the way, which this did at first, put somebody back on the bar at the foot when
    // they shut the panel, having lost the view they pressed from. So it stays open
    // underneath and the panel simply covers it, which is what the stacking already
    // says — the bar is at 3, the player at 5, the scrim at 6 and the panel at 7 — and
    // what the queue opened from that same player has always done.
    Effect::new(move |_| {
        if opened.get().is_some() {
            queue.set(false);
        }
    });
    Effect::new(move |_| {
        if queue.get() {
            opened.set(None);
        }
    });

    view! {
        <div class="shell">
            <aside class="side" class:out=move || folded_out.get()>
                // The name goes home. It is the one thing on every screen that
                // everybody already tries to click.
                <A href="/" attr:class="brand" on:click=move |_| fold.set(false)>
                    <Glyph icon=Icon::Logo />
                    {t!("app.name")}
                </A>

                <nav>
                    {EVERYONE.iter().map(|section| view! { <Entry section fold /> }).collect_view()}

                    <p class="nav-title">{t!("nav.collection")}</p>
                    {COLLECTION
                        .iter()
                        .map(|section| view! { <Entry section fold /> })
                        .collect_view()}

                    <p class="nav-title">{t!("nav.yours")}</p>
                    {YOURS.iter().map(|section| view! { <Entry section fold /> }).collect_view()}

                    <Show when=move || admin>
                        <p class="nav-title">{t!("nav.server")}</p>
                        {SERVER.iter().map(|section| view! { <Entry section fold /> }).collect_view()}
                    </Show>
                </nav>

                // Against the bottom, in the order of how long each has been
                // there: what the server is doing, then what is sounding, then
                // who is asking. Scanning above playback, never sharing an edge
                // with it.
                <div class="foot">
                    <Show when=move || admin>
                        <ScanStrip scan />
                    </Show>
                    <Dock queue />
                    <WhoAmI admin on_out fold />
                </div>
            </aside>

            // Over the screen while the sections are out, so a touch anywhere
            // else folds them rather than landing on what is underneath.
            <Show when=move || folded_out.get()>
                <div class="menu-shade" on:click=move |_| fold.set(false)></div>
            </Show>

            // Only at the width where the column has folded away. Its whole job is
            // the way back to it, so it carries nothing else.
            <header class="bar">
                <button
                    class="menu-button"
                    title=t!("nav.menu")
                    aria-expanded=move || folded_out.get().to_string()
                    on:click=move |_| fold.update(|out| *out = !*out)
                >
                    <Glyph icon=Icon::Menu />
                </button>

                <A href="/" attr:class="brand">
                    <Glyph icon=Icon::Logo />
                    {t!("app.name")}
                </A>

                // The corner nothing else was using. The bar is always there and
                // the column is not, so the one block in it that is not navigation
                // — who you are and the way out — costs nothing to reach from here,
                // where it used to cost opening the sections and dismissing them.
                <WhoAmI admin on_out fold cornered=true />
            </header>

            <main class="body">{children()}</main>

            // Outside the column on purpose. Both of these belong to the panel and
            // not to the sections: the machine has to outlive the drawer opening and
            // closing, and the bar has to be reachable while it is shut.
            <Sound />
            <Afoot on_open=Callback::new(move |()| sheet.set(true)) />
            <Sheet open=sheet queue />
            <crate::queue::Queue open=queue />

            // And whichever panel the collection has open, for the same reason: it is
            // the panel's and not the section's.
            <crate::drawer::Drawers />
        </div>
    }
}

/// One place to go. Going there folds the sections away again, which on a wide
/// screen changes nothing because there is nothing folded.
///
/// No icon. Nine entries each with a glyph beside it was nine glyphs to read past
/// on the way to the words, and the words were doing the work.
#[component]
fn Entry(section: &'static Section, fold: WriteSignal<bool>) -> impl IntoView {
    view! {
        <A href=section.path exact=section.exact on:click=move |_| fold.set(false)>
            {(section.label)()}
        </A>
    }
}

/// A scan, while there is one, at the foot of the column.
///
/// A scan takes minutes and people go and do something else while it runs. Here it
/// is in front of them wherever they went, and here is also where it can be looked
/// at in full and stopped — which used to be on the screen you land on, so watching
/// a scan meant going back to watch it.
///
/// Closed it says three things: that a scan is running, how much it has found, and
/// what it is reading. Open it adds what the run has done so far and the way to
/// stop it. There is no chevron: the line is the colour of a link and moves, and a
/// second hint would be furniture.
///
/// Nothing at all when nothing is running. A strip saying "idle" is a strip saying
/// nothing, and the screen you land on keeps what the last scan did.
#[component]
fn ScanStrip(scan: ReadSignal<Option<Status>>) -> impl IntoView {
    let (open, set_open) = signal(false);
    let (stopping, set_stopping) = signal(false);

    let running = move || scan.get().filter(|status| status.scanning);

    let stop = move |_| {
        set_stopping.set(true);
        spawn_local(async move {
            let _ = api::cancel_scan().await;
            set_stopping.set(false);
        });
    };

    view! {
        <Show when=move || running().is_some()>
            <div class="strip" class:open=move || open.get()>
                <button
                    class="strip-head working"
                    aria-expanded=move || open.get().to_string()
                    on:click=move |_| set_open.update(|shown| *shown = !*shown)
                >
                    <Glyph icon=Icon::Scan />
                    {t!("scan.running")}
                    <span class="counted">
                        {move || thousands(running().map(|status| status.tracks).unwrap_or_default())}
                    </span>
                </button>

                // The button that stops it is a sibling of the one that opens this,
                // never a child: a control inside a control means one click doing
                // two things, and the two things here are "look" and "stop".
                <div class="strip-stats">
                    <div>
                        <Pair label=t!("scan.folders").to_string() figure=Signal::derive(move || {
                            thousands(running().map(|status| status.folders).unwrap_or_default())
                        }) />
                        <Pair label=t!("scan.added").to_string() figure=Signal::derive(move || {
                            let status = running();
                            let seen = status.as_ref().map(|status| status.tracks).unwrap_or_default();
                            let known = status.map(|status| status.unchanged).unwrap_or_default();
                            thousands(seen.saturating_sub(known))
                        }) />
                        <Pair label=t!("scan.failed").to_string() figure=Signal::derive(move || {
                            thousands(running().map(|status| status.failed).unwrap_or_default())
                        }) />

                        <button class="stop" disabled=stopping on:click=stop>
                            {t!("scan.cancel")}
                        </button>
                    </div>
                </div>

                <span class="where">
                    {move || {
                        running().and_then(|status| status.path.or(status.library)).unwrap_or_default()
                    }}
                </span>
            </div>
        </Show>
    }
}

/// What is sounding, at the foot of the column.
///
/// Here rather than across the bottom of the window on the principle the whole
/// column runs on: everything permanently live lives in the left column, and the
/// screen keeps its full height. A strip across the foot would take a band off every
/// screen to hold what this holds.
///
/// Nothing at all until something has been played. An idle player showing a blank
/// square and a dead progress line is furniture.
///
/// What actually plays, drawn nowhere.
///
/// **Two elements, one sounding.** A record made without pauses between its tracks
/// — which is most of Genesis and all of Dark Side of the Moon — used to fall
/// silent at every join, and the silence was mostly not the decoder's: changing one
/// element's `src` starts a fresh HTTP request, and against a server over the
/// internet that measured 107 to 185 milliseconds before the first byte, plus
/// whatever the browser buffers before it will start. So the track after this one
/// is already loaded in the other element, and the join is a `play()` on something
/// that is ready rather than a fetch.
///
/// What is left is the file's own doing. An MP3 carries silence at each end because
/// the encoder puts it there, and the only way to trim it is to be told how much by
/// the LAME header — which the files this was built against do not carry. So this
/// closes the gap it made and not the one that was already in the music.
///
/// Which of the two is sounding is a signal, and the sources are written rather
/// than derived: at the handover the element that was holding the next track keeps
/// exactly the source it already has, and reassigning it — even to the same string
/// — would throw away the buffer that is the whole point.
#[component]
fn Sound() -> impl IntoView {
    let player = crate::player::player();

    let first = NodeRef::<leptos::html::Audio>::new();
    let second = NodeRef::<leptos::html::Audio>::new();

    // What each element has actually been given, which is memory rather than
    // markup: `src` is set here by hand instead of being an attribute that
    // follows a signal.
    //
    // It had to stop being an attribute. Both the source and the `play()` that
    // follows it come from signals that change in the same breath, and nothing
    // says which of the two effects runs first — so play was being asked of an
    // element that had not been given its file yet. It did nothing, `playing`
    // stayed true, and nothing ever asked again: pressing play on anything at all
    // sounded only after a pause and a second press.
    let holding = (RwSignal::new(String::new()), RwSignal::new(String::new()));

    // False is the first element. Flipped at the end of a track, never anywhere
    // else: everything that jumps — a button, a row, a new queue — keeps whichever
    // is sounding and gives it the new track, because there is nothing prepared to
    // hand over to.
    let second_one = RwSignal::new(false);

    let live = move || if second_one.get() { second } else { first };

    // Who holds what, and who is sounding, decided together and in that order.
    //
    // The live element gets the track that should be playing and is asked to play
    // it; the other gets the one after and is kept quiet. Neither is given a
    // source it already holds — which is what makes the handover free, since the
    // element being made live is already holding exactly this.
    Effect::new(move |_| {
        let wanted = player
            .current()
            .map(|id| api::audio(&id))
            .unwrap_or_default();
        let next = player
            .upcoming()
            .map(|id| api::audio(&id))
            .unwrap_or_default();
        let playing = player.playing.get();
        let second_is_live = second_one.get();

        let (live, idle) = match second_is_live {
            true => (second, first),
            false => (first, second),
        };
        let (live_holds, idle_holds) = match second_is_live {
            true => (holding.1, holding.0),
            false => (holding.0, holding.1),
        };

        if let Some(audio) = live.get() {
            hand(&audio, live_holds, &wanted);

            // Asking, not trusting: a browser can refuse — a file that will not
            // decode, a gesture it wanted first — and what comes back is whatever
            // the element then reports through `on:play` and `on:pause`.
            if playing {
                let _ = audio.play();
            } else {
                let _ = audio.pause();
            }
        }

        if let Some(audio) = idle.get() {
            // Silent, always. Reaching the end of a track does not pause the
            // element that played it — `paused` stays false — so the one that has
            // just handed over would start sounding the moment it is given the
            // track after next, and two would be playing at once.
            let _ = audio.pause();
            hand(&audio, idle_holds, &next);
        }
    });

    // Where somebody has asked to be, against where the element actually is. A drag
    // along the bar writes the first; `timeupdate` writes it back from the second a
    // few times a second, so this fires constantly and almost always finds the two
    // already agreeing.
    //
    // A second of slack, which is what keeps it from fighting its own clock: the two
    // are never exactly equal while a file plays, and closing that gap would mean
    // seeking on every tick. Nothing is lost — under a second is under the width of
    // a finger on this bar.
    Effect::new(move |_| {
        let Some(audio) = live().get() else { return };
        let wanted = player.elapsed.get();

        if (wanted - audio.current_time()).abs() > 1.0 {
            audio.set_current_time(wanted);
        }
    });

    view! {
        <Half node_ref=first mine=false sounding=second_one />
        <Half node_ref=second mine=true sounding=second_one />
    }
}

/// Gives one element a file to load, and only when it is not the one it has.
///
/// Reassigning the source an element already holds throws away everything it has
/// buffered, which at a handover is the whole point of there being two.
///
/// Nothing to load is the attribute taken away rather than set to nothing: `src=""`
/// is a request for the page this panel is served from, read as audio, and that is
/// what the end of a queue would otherwise ask for.
fn hand(audio: &web_sys::HtmlAudioElement, holding: RwSignal<String>, wanted: &str) {
    if holding.get_untracked() == wanted {
        return;
    }

    holding.update_untracked(|held| *held = wanted.to_string());

    if wanted.is_empty() {
        let _ = audio.remove_attribute("src");
    } else {
        audio.set_src(wanted);
    }

    // Which is what makes either of those take effect: without it the element goes
    // on with whatever it was doing until it happens to notice.
    audio.load();
}

/// One of the two, which is only ever the clock while it is the one sounding.
///
/// Every event asks that first. The other element is loading a track nobody is
/// listening to yet, and left to speak it would report a length that is not the one
/// on screen, or a pause that nobody asked for.
#[component]
fn Half(
    node_ref: NodeRef<leptos::html::Audio>,
    /// Which of the two this is, against the flag that says which is sounding.
    mine: bool,
    sounding: RwSignal<bool>,
) -> impl IntoView {
    let player = crate::player::player();
    let is_mine = move || sounding.get_untracked() == mine;

    view! {
        // No `src` here: what each of these is loading is handed to it above,
        // beside the `play()` that has to follow it. See `Sound`.
        <audio
            node_ref=node_ref
            // The whole point: the one that is not sounding fetches anyway, so that
            // when its turn comes there is nothing left to wait for.
            preload="auto"
            on:timeupdate:target=move |e| {
                if is_mine() {
                    player.ticked(e.target().current_time(), e.target().duration())
                }
            }
            on:play=move |_| {
                if is_mine() {
                    player.playing.set(true)
                }
            }
            on:pause=move |_| {
                if is_mine() {
                    player.playing.set(false)
                }
            }
            on:ended=move |_| {
                if !is_mine() {
                    return;
                }

                // Hand over first, then step. The two land together, so the effect
                // that hands out sources sees the new pair at once and finds the
                // element it is about to make live already holding the right track.
                if player.upcoming().is_some() {
                    sounding.set(!mine);
                }

                player.next();
            }
        ></audio>
    }
}

/// What is sounding, as the column shows it.
///
/// **All three things it says lead somewhere.** They were three lines of text for as
/// long as this existed, which is a block about a song that cannot answer a single
/// question about the song. The cover opens the cover, the title opens what is known
/// about the track, and the credit opens whoever is playing it — the same three
/// panels a row in a listing opens, reached from the one place that is on screen
/// wherever you have wandered off to.
#[component]
fn Dock(queue: RwSignal<bool>) -> impl IntoView {
    let player = crate::player::player();

    let title = move || {
        player
            .now
            .get()
            .map(|track| track.title)
            .unwrap_or_else(|| t!("player.loading").to_string())
    };

    let who = move || {
        player
            .now
            .get()
            .and_then(|track| track.artists)
            .unwrap_or_default()
    };

    // Which song these lines are about, and nothing at all in the moment between
    // stepping onto a track and the server saying what it is.
    let sounding = move || player.now.get().map(|track| track.id);

    // Where its cover is, and whether asking for it came back with nothing.
    //
    // The second half is what a drawer's heading asks too, and for the same reason: a
    // record nothing has looked at yet is indistinguishable from one with no art at
    // all until something fetches it. Without it the press would open a broken picture
    // full screen, which is worse than a picture that cannot be pressed.
    let art = move || {
        player
            .now
            .get()
            .and_then(|track| track.album_id)
            .map(|album| api::cover(&album))
    };
    let missing = RwSignal::new(false);

    // Asked again for every song, so one record without a cover does not leave the
    // next one showing a glyph over art that is there.
    Effect::new(move |_| {
        art();
        missing.set(false);
    });

    let there = move || art().is_some() && !missing.get();

    // What the picture is of, for whatever reads the page aloud: the record, since
    // that is what a cover is a picture of, and the song only where there is no record
    // to name.
    let showing = RwSignal::new(false);
    let named = move || {
        player
            .now
            .get()
            .and_then(|track| track.album.or(Some(track.title)))
            .unwrap_or_default()
    };

    view! {
        <Show when=move || player.loaded()>
            <div class="dock">
                <div class="sounding">
                    // The cover of the record it is from, which is the one picture
                    // the sidebar has room for. A button only where there is one to
                    // look at: the glyph standing in for a missing cover leads
                    // nowhere, exactly as in the heading of a drawer.
                    {move || {
                        if there() {
                            view! {
                                <button
                                    class="see-art"
                                    title=t!("player.this_cover")
                                    on:click=move |_| showing.set(true)
                                >
                                    <img
                                        class="art"
                                        src=move || art().unwrap_or_default()
                                        alt=""
                                        on:error=move |_| missing.set(true)
                                    />
                                </button>
                            }
                                .into_any()
                        } else {
                            view! {
                                <span class="art">
                                    <Glyph icon=Icon::Albums />
                                </span>
                            }
                                .into_any()
                        }
                    }}

                    <span class="what">
                        <button
                            class="bare"
                            title=t!("player.this_track")
                            on:click=move |_| {
                                if let Some(id) = sounding() {
                                    crate::drawer::open(crate::drawer::Open::Track(id));
                                }
                            }
                        >
                            {title}
                        </button>

                        // Only where the file credited somebody. A line with nothing
                        // in it is not a line, and an empty one that could be pressed
                        // is worse than that.
                        {move || {
                            let line = who();
                            (!line.is_empty())
                                .then(|| {
                                    view! {
                                        <button
                                            class="bare by"
                                            title=t!("player.this_artist")
                                            on:click=move |_| {
                                                if let Some(id) = sounding() {
                                                    crate::drawer::open_artist_of(id);
                                                }
                                            }
                                        >
                                            {line}
                                        </button>
                                    }
                                })
                        }}
                    </span>
                </div>

                <crate::drawer::Enlarged
                    showing
                    picture=Signal::derive(art)
                    heading=Signal::derive(named)
                />

                <div class="along">
                    <span>{move || pages::length(player.elapsed.get() as i64)}</span>
                    <Along player />
                    <span>{move || pages::length(player.duration.get() as i64)}</span>
                </div>

                <div class="transport">
                    <button
                        class="step"
                        title=t!("player.previous")
                        on:click=move |_| player.previous()
                    >
                        <Glyph icon=Icon::Previous />
                    </button>

                    <button
                        class="toggle"
                        title=t!("player.toggle")
                        on:click=move |_| player.toggle()
                    >
                        {move || {
                            if player.playing.get() {
                                view! { <Glyph icon=Icon::Pause /> }
                            } else {
                                view! { <Glyph icon=Icon::Play /> }
                            }
                        }}
                    </button>

                    <button class="step" title=t!("player.next") on:click=move |_| player.next()>
                        <Glyph icon=Icon::Next />
                    </button>

                    // How much is queued behind this, and the way to see it. Lit while
                    // the queue is open, so the button says where the thing on screen
                    // came from.
                    <button
                        class="queued"
                        class:showing=move || queue.get()
                        title=t!("player.queued")
                        on:click=move |_| queue.update(|open| *open = !*open)
                    >
                        <Glyph icon=Icon::Playlists />
                        {move || thousands(player.ahead() as u64)}
                    </button>
                </div>
            </div>
        </Show>
    }
}

/// What is sounding, as a phone shows it: a bar across the foot of the screen.
///
/// Not in the column, because on a phone the column is a drawer — it is not there
/// until you ask for it, and something you are listening to cannot live behind a
/// button. So it comes out of the drawer entirely and sits where nothing else
/// competes for the room.
///
/// It says less than the block in the column does, and everything it drops is
/// something a thumb cannot use well anyway: no elapsed and no total, because the
/// line along its top edge already says where you are; no previous, because at 44px
/// a row of four is a row of mistakes; no queue count, because there is nowhere yet
/// to look at a queue. Pause and next, both 44px, and that is the bar.
#[component]
fn Afoot(on_open: Callback<()>) -> impl IntoView {
    let player = crate::player::player();

    let title = move || {
        player
            .now
            .get()
            .map(|track| track.title)
            .unwrap_or_else(|| t!("player.loading").to_string())
    };

    let who = move || {
        player
            .now
            .get()
            .and_then(|track| track.artists)
            .unwrap_or_default()
    };

    view! {
        <Show when=move || player.loaded()>
            <div class="afoot">
                // On the bar's own top edge rather than as a row of its own: where
                // you are is worth a glance and not a line of figures. It reaches a
                // little above and below that edge so a finger can find it, which is
                // the eight pixels it steals from the top of what opens the sheet.
                <Along player />

                <div class="holding">
                    // Everything but the two buttons opens the player. A button of
                    // its own would be a fourth 44px target on a bar that has room
                    // for three; the art and the title are what somebody presses
                    // when they want to see what is playing.
                    <button class="most" title=t!("player.open") on:click=move |_| on_open.run(())>
                    {move || match player.now.get().and_then(|track| track.album_id) {
                        Some(album) => {
                            view! { <img class="art" src=api::cover(&album) alt="" /> }.into_any()
                        }
                        None => {
                            view! {
                                <span class="art">
                                    <Glyph icon=Icon::Albums />
                                </span>
                            }
                                .into_any()
                        }
                    }}

                    <span class="what">
                        <span>{title}</span>
                        <span class="by">{who}</span>
                    </span>
                    </button>

                    <button class="tap" title=t!("player.toggle") on:click=move |_| player.toggle()>
                        {move || {
                            if player.playing.get() {
                                view! { <Glyph icon=Icon::Pause /> }
                            } else {
                                view! { <Glyph icon=Icon::Play /> }
                            }
                        }}
                    </button>

                    <button class="tap quiet" title=t!("player.next") on:click=move |_| player.next()>
                        <Glyph icon=Icon::Next />
                    </button>
                </div>
            </div>
        </Show>
    }
}

/// How far the sheet has to be pushed down before letting go closes it.
///
/// A fifth of a tall phone. Short enough to be an easy flick, far enough that reaching
/// for the cover and slipping does not put the player away.
const SHOVED: f64 = 160.0;

/// The player at full height, over whatever was on screen.
///
/// A sheet and not a route, which is the whole of why it works: the listing behind it
/// keeps its scroll and its search, so closing this puts somebody back exactly where
/// they were rather than at the top of nine hundred tracks.
///
/// What it says that the bar cannot: the cover at the size a cover is worth looking
/// at, the title as a heading rather than as a line, room for previous, and the
/// format at its foot — this is an administration panel before it is a listening
/// client, and the foot of the player is where somebody notices that a file is not
/// the quality they thought.
///
/// The time on the right counts down rather than up. While something plays, "how much
/// is left" is the question; the total is a fact about the file and is already in
/// every listing.
#[component]
fn Sheet(open: RwSignal<bool>, queue: RwSignal<bool>) -> impl IntoView {
    let player = crate::player::player();

    // Pushed back down to close, which is the gesture a sheet asks for by being one.
    // Only downwards: there is nothing above it to reach.
    let push = crate::drag::Drag::new();

    let title = move || {
        player
            .now
            .get()
            .map(|track| track.title)
            .unwrap_or_else(|| t!("player.loading").to_string())
    };

    let who = move || {
        player
            .now
            .get()
            .and_then(|track| track.artists)
            .unwrap_or_default()
    };

    // Which song the two lines are about, as in the column.
    let sounding = move || player.now.get().map(|track| track.id);

    // Which record it is from, over the top of the sheet. Nothing at all for a track
    // that belongs to no album, rather than a heading with an empty name in it.
    let from = move || {
        player
            .now
            .get()
            .and_then(|track| track.album)
            .map(|album| t!("player.from", album = album).to_string())
            .unwrap_or_default()
    };

    // What is left, not what there is. Never positive, and never a stray minus in
    // front of a zero before the metadata has arrived.
    let left = move || {
        let (elapsed, whole) = (player.elapsed.get(), player.duration.get());
        let over = (whole - elapsed).max(0.0);

        if whole > 0.0 {
            format!("-{}", pages::length(over as i64))
        } else {
            pages::MISSING.to_string()
        }
    };

    // What the file is, which is two facts joined only where there are two: a format
    // that reports no rate should read "flac", not "flac ·".
    let file = move || {
        let Some(track) = player.now.get() else {
            return String::new();
        };

        let format = track.suffix.to_uppercase();

        match track.bit_rate {
            Some(rate) => format!("{format} · {}", t!("player.kbps", rate = rate)),
            None => format,
        }
    };

    view! {
        <Show when=move || open.get() && player.loaded()>
            <div
                class="sheet-player"
                class:pushed=move || push.going.get()
                style=move || format!("transform: translateY({}px)", push.down().max(0.0))
                on:pointerdown=move |e: web_sys::PointerEvent| {
                    push.begin(&e);
                }
                on:pointermove=move |e: web_sys::PointerEvent| push.moved(&e)
                on:pointerup=move |_| {
                    // Far enough down and it goes; short of that it springs back,
                    // which is what clearing the offset does on its own.
                    if let Some((_, down)) = push.end()
                        && down >= SHOVED
                    {
                        open.set(false);
                    }
                }
                on:pointercancel=move |_| {
                    push.end();
                }
            >
                <header>
                    <button
                        class="tap"
                        title=t!("common.close")
                        on:click=move |_| open.set(false)
                    >
                        // The chevron this panel already has, turned to point the way
                        // the sheet will go.
                        <span class="downward">
                            <Glyph icon=Icon::Chevron />
                        </span>
                    </button>

                    <span class="whence">{from}</span>

                    // The third place in the row stays empty: the frame puts a menu
                    // for the track here, and what it would open is the drawer that
                    // does not exist yet. An empty cell keeps the name centred.
                    <span class="tap"></span>
                </header>

                <div class="middle">
                    {move || match player.now.get().and_then(|track| track.album_id) {
                        Some(album) => {
                            view! { <img class="art" src=api::cover(&album) alt="" /> }.into_any()
                        }
                        None => {
                            view! {
                                <span class="art">
                                    <Glyph icon=Icon::Albums />
                                </span>
                            }
                                .into_any()
                        }
                    }}

                    // Both lines lead where the column's do, and the cover does not:
                    // it is already being looked at at the width of the screen, and
                    // there is nothing full screen can add to that.
                    //
                    // The sheet stays open behind whatever they open, so shutting that
                    // panel comes back here rather than to the bar at the foot. See the
                    // shell, where the two things that do close each other are.
                    <div class="naming">
                        <h1>
                            <button
                                class="bare"
                                title=t!("player.this_track")
                                on:click=move |_| {
                                    if let Some(id) = sounding() {
                                        crate::drawer::open(crate::drawer::Open::Track(id));
                                    }
                                }
                            >
                                {title}
                            </button>
                        </h1>

                        {move || {
                            let line = who();
                            (!line.is_empty())
                                .then(|| {
                                    view! {
                                        <p class="quiet">
                                            <button
                                                class="bare"
                                                title=t!("player.this_artist")
                                                on:click=move |_| {
                                                    if let Some(id) = sounding() {
                                                        crate::drawer::open_artist_of(id);
                                                    }
                                                }
                                            >
                                                {line}
                                            </button>
                                        </p>
                                    }
                                })
                        }}
                    </div>

                    <div class="along">
                        <Along player />
                        <div class="clock">
                            <span>{move || pages::length(player.elapsed.get() as i64)}</span>
                            <span>{left}</span>
                        </div>
                    </div>

                    <div class="transport">
                        <button
                            class="step"
                            title=t!("player.previous")
                            on:click=move |_| player.previous()
                        >
                            <Glyph icon=Icon::Previous />
                        </button>

                        <button
                            class="toggle"
                            title=t!("player.toggle")
                            on:click=move |_| player.toggle()
                        >
                            {move || {
                                if player.playing.get() {
                                    view! { <Glyph icon=Icon::Pause /> }
                                } else {
                                    view! { <Glyph icon=Icon::Play /> }
                                }
                            }}
                        </button>

                        <button
                            class="step"
                            title=t!("player.next")
                            on:click=move |_| player.next()
                        >
                            <Glyph icon=Icon::Next />
                        </button>
                    </div>
                </div>

                // The way into the queue, and what the file is.
                <div class="foot-note">
                    <button class="into-queue" on:click=move |_| queue.set(true)>
                        <Glyph icon=Icon::Playlists />
                        {t!("queue.heading")}
                        " · "
                        {move || pages::thousands(player.ahead() as i64)}
                    </button>

                    <span class="quiet">{file}</span>
                </div>
            </div>
        </Show>
    }
}

/// Where in the track we are, and the way to be somewhere else.
///
/// One of these in all three views, which is why it is a component rather than three
/// spans: a bar you can drag is more than a filled rectangle, and having written it
/// once it may as well be the only one.
///
/// It is a real `range` input under a line drawn by its wrapper. Which buys the whole
/// of the behaviour for nothing: dragging, tapping anywhere along it, arrow keys, and
/// a control that says what it is to anything reading the page aloud. Written as a
/// div with a click handler it would have had the first of those and none of the rest.
///
/// The wrapper draws the line because `::before` does nothing on an `input` — it is a
/// replaced element — and the input itself is transparent over the top of it. Which is
/// also what lets the thing be comfortable to hit: the line is two pixels and the
/// input is sixteen, so a finger has somewhere to land without the bar looking like a
/// slab.
#[component]
fn Along(player: crate::player::Player) -> impl IntoView {
    view! {
        <span
            class="along-track"
            style=move || format!("--at: {}%", player.share() * 100.0)
        >
            <input
                type="range"
                min="0"
                // Never zero, or the browser refuses the whole control while a track
                // is still saying how long it is.
                max=move || player.duration.get().max(1.0).to_string()
                step="1"
                prop:value=move || player.elapsed.get().to_string()
                title=t!("player.seek")
                on:input:target=move |e| {
                    if let Ok(at) = e.target().value().parse::<f64>() {
                        player.seek_to(at);
                    }
                }
            />
        </span>
    }
}

/// A label and a figure, apart from each other on one line.
#[component]
fn Pair(label: String, figure: Signal<String>) -> impl IntoView {
    view! {
        <span class="pair">
            <span class="quiet">{label}</span>
            <span>{move || figure.get()}</span>
        </span>
    }
}

/// Who you are now, as against who you were when the panel was built.
///
/// The identity that arrives with the session is what the panel is built from, and
/// building it again is what would stop the music — so anything about yourself that
/// can change while you are looking at the panel has to be read from here instead.
/// Two things can, and an administrator can change both of their own: the name of
/// the account, and the name it asks to be called by.
///
/// The first of those is not decoration. Every call a screen makes about you names
/// the account in its path, so a copy taken when the panel was built is a copy that
/// asks the server about somebody who no longer exists the moment you rename
/// yourself — which is what Profile and Access did, one save later.
///
/// A type of its own rather than two loose signals, because a context is looked up
/// by type: a second `RwSignal<String>` provided anywhere in the panel would
/// silently become this one.
#[derive(Clone, Copy)]
pub struct Me {
    /// The name of the account, which is what the server is asked about.
    pub username: RwSignal<String>,
    /// What to be called, or nothing to be called by the account's name.
    pub called: RwSignal<Option<String>>,
}

impl Me {
    /// Who the server has just said you are.
    ///
    /// Both names out of the one answer, rather than each being remembered
    /// separately: forgetting the first leaves the panel asking about a name that is
    /// gone, and forgetting the second leaves a change looking like it was not saved.
    pub fn is_now(&self, account: &Account) {
        self.username.set(account.username.clone());
        self.called.set(account.display_name.clone());
    }
}

/// The name to address somebody by: what they chose, and their account's name until
/// they choose.
///
/// The two places that address anybody — the greeting and the account menu — read it
/// from here, so they cannot disagree about which name that is, and neither of them
/// is still saying the old one after a rename.
pub fn called_me() -> Signal<String> {
    let Me { username, called } = expect_context::<Me>();

    Signal::derive(move || called.get().unwrap_or_else(|| username.get()))
}

/// Who is asking, and what that lets them reach.
///
/// Twice in the frame and never twice on the screen: at the foot of the column,
/// where the whole row opens the menu — a target the size of the column instead of
/// the size of a coin — and in the bar at the top of a narrow screen, where the row
/// will not fit and the disc alone stands in the corner. The stylesheet keeps
/// whichever of the two belongs to the width, so the one that is not there is not
/// there at all rather than folded away somewhere.
///
/// It opens upward from the column, because that is the last thing in it and there
/// is nothing below it, and downward from the bar for the same reason the other way
/// round. Closing it was `focusout` on the container, which reads well and does not
/// work: the order is mousedown, then focusout, then click, so the menu went away
/// before the click could land on anything in it. So it closes the way the folded
/// sections do — a sheet behind it catches anything aimed elsewhere — and every
/// entry closes it on the way out, since choosing one is finishing with it.
#[component]
fn WhoAmI(
    admin: bool,
    on_out: Callback<()>,
    fold: WriteSignal<bool>,
    /// In the corner of the bar rather than at the foot of the column: the disc on
    /// its own, and the name it stands for said in a label instead, since a name
    /// nobody can see is a name nothing reads out either.
    #[prop(optional)]
    cornered: bool,
) -> impl IntoView {
    let (open, set_open) = signal(false);

    // What they asked to be called, falling back to the name of the account, and read
    // from the signal so that changing either on the profile screen shows here without
    // a reload. Both the letter and the name come from the same string, so the initial
    // of somebody called Óscar is not the initial of an account called ogarcia.
    let name = called_me();

    // The first letter, and only ever one: a name is text in a language we do
    // not know, so a character is taken rather than a byte sliced off.
    let initial = Signal::derive(move || {
        name.get()
            .chars()
            .next()
            .map(|first| first.to_uppercase().to_string())
            .unwrap_or_default()
    });

    let away = move |_| {
        set_open.set(false);
        fold.set(false);
    };

    // Said twice on a narrow screen and once anywhere else: the block in the column
    // has it written across it, and the disc in the corner has room for a letter.
    let role = move || {
        if admin {
            t!("header.administrator").to_string()
        } else {
            t!("header.listener").to_string()
        }
    };

    view! {
        <div class="dropdown" class:cornered=cornered>
            <Show when=move || open.get()>
                <div class="veil" on:click=move |_| set_open.set(false)></div>
                <div class="mine">
                    // Whose account it is, before what can be done to it. Only up in
                    // the bar, where what opened this is a disc with one letter on it
                    // and the sheet comes up from the opposite edge of the screen: it
                    // has to say what it belongs to, because nothing around it does.
                    // At the foot of the column the block it hangs off says both.
                    {cornered
                        .then(|| {
                            view! {
                                <p class="mine-who">
                                    <span class="avatar">{initial}</span>
                                    <span class="who">
                                        <span>{name}</span>
                                        <span class="role">{role}</span>
                                    </span>
                                </p>
                            }
                        })}

                    {MINE
                        .iter()
                        .map(|section| {
                            view! {
                                <A href=section.path on:click=away>
                                    {(section.label)()}
                                </A>
                            }
                        })
                        .collect_view()}

                    // A line between going somewhere and leaving, in the box that
                    // has no other division in it. The sheet divides every row from
                    // the next, so a line here would be the second one in a row and
                    // say nothing: up there the space under the last of them is what
                    // sets leaving apart.
                    {(!cornered).then(|| view! { <hr /> })}

                    <button
                        class="menu-item leave"
                        on:click=move |_| {
                            set_open.set(false);
                            spawn_local(async move {
                                api::log_out().await;
                                on_out.run(());
                            });
                        }
                    >
                        <Glyph icon=Icon::LogOut />
                        {t!("header.log_out")}
                    </button>
                </div>
            </Show>

            <button
                class="whoami"
                aria-label=move || cornered.then(|| name.get())
                aria-expanded=move || open.get().to_string()
                on:click=move |_| set_open.update(|shown| *shown = !*shown)
            >
                <span class="avatar">{initial}</span>
                <span class="who">
                    <span>{name}</span>
                    <span class="role">{role}</span>
                </span>
                <span class="chevron">
                    <Glyph icon=Icon::Chevron />
                </span>
            </button>
        </div>
    }
}

/// Grouped with a space, which every language this speaks agrees on and no
/// language mistakes for a decimal point.
///
/// The same rule as the figures on the screen you land on. It is here rather than
/// shared with them because it is four lines, and a module for four lines that two
/// screens use is a module to go and look up.
fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }

    out
}
