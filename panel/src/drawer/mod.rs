// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! More about the thing you pressed.
//!
//! A panel down the right of the window rather than a screen of its own, because
//! going somewhere else and coming back would cost the list its place and the search
//! its words — and looking at one track out of nine hundred is something people do
//! several times in a row.
//!
//! **One at a time.** Which is open is a single signal held above the router, so
//! opening one closes whatever else was over the screen, including the queue. Two
//! panels stacked on the same edge is a thing to unstack rather than to read, and a
//! stack that remembered its way back would answer a close by opening something.
//!
//! **Nothing here is a route.** The address bar says which section you are in, and
//! that stays true: a drawer is a thing you have open, not a place you have gone. It
//! closes on going anywhere.
//!
//! What each drawer is made of lives beside it. What is here is the frame they share
//! and the two pieces every one of them repeats — a run of figures, and a name
//! against a value that draws nothing at all when there is no value. That last one is
//! the rule of these panels in one place: a row with nothing in it is not a row.

pub mod album;
pub mod track;

use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use rust_i18n::t;

/// Which panel is over the screen.
#[derive(Clone, PartialEq, Eq)]
pub enum Open {
    /// One track, by identifier.
    Track(String),
    /// One record.
    Album(String),
}

/// The one signal that says. Held above the router so it survives nothing and
/// reaches everything.
pub type Opened = RwSignal<Option<Open>>;

/// Reaches it. Provided by the shell, which is above every screen that opens one.
pub fn opened() -> Opened {
    use_context::<Opened>().expect("the shell provides what is open")
}

/// Puts one over the screen.
pub fn open(what: Open) {
    opened().set(Some(what));
}

/// Takes it away.
pub fn shut() {
    opened().set(None);
}

/// Whichever is open, drawn.
///
/// Mounted once, outside the router, so the thing on screen outlives the row that
/// opened it — a track's panel does not belong to the fifty rows that happened to be
/// fetched, and a scroll that fetched fifty more must not close it.
#[component]
pub fn Drawers() -> impl IntoView {
    let opened = opened();

    // Going somewhere else closes it. The panel is about a thing on the screen you
    // were looking at, and it has nothing to say over the next one.
    let here = leptos_router::hooks::use_location();
    Effect::new(move |_| {
        here.pathname.track();
        opened.set(None);
    });

    view! {
        {move || {
            opened
                .get()
                .map(|what| match what {
                    Open::Track(id) => view! { <track::Track id /> }.into_any(),
                    Open::Album(id) => view! { <album::Album id /> }.into_any(),
                })
        }}
    }
}

/// The frame every one of them comes in: what catches a press outside, and the panel
/// itself.
///
/// The press outside is a way out that needs no aiming, which matters most where the
/// panel is at its widest and the close button at its furthest.
#[component]
pub fn Frame(children: Children) -> impl IntoView {
    view! {
        <div class="scrim" on:click=move |_| shut()></div>
        <aside class="drawer">{children()}</aside>
    }
}

/// The head of one: what kind of thing this is, what it is called, and the way out.
#[component]
pub fn Head(
    icon: Icon,
    /// A round frame round the glyph rather than a square one, for the one of these
    /// that is a person.
    #[prop(optional)]
    round: bool,
    /// The record whose cover stands for this thing, once it is known.
    ///
    /// A signal because it is not known when the panel opens: the identifier arrives
    /// with everything else, a moment later. The glyph is what is there in the
    /// meantime, and what stays where there is no cover to be had.
    #[prop(optional, into)]
    cover: Signal<Option<String>>,
    heading: Signal<String>,
    /// The line under it, which every one of these has and none of them needs: it is
    /// empty until what was asked for arrives.
    lead: Signal<String>,
) -> impl IntoView {
    // Whether asking came back with nothing. Asked for rather than checked first, for
    // the same reason a shelf of records asks: the flag a listing carries says a cover
    // has been *found* already, and the finding is what the asking does.
    let missing = RwSignal::new(false);

    view! {
        <header>
            <span class="emblem" class:round=round>
                <Show
                    when=move || cover.get().is_some() && !missing.get()
                    fallback=move || view! { <Glyph icon /> }
                >
                    <img
                        class="art"
                        src=move || crate::api::cover(&cover.get().unwrap_or_default())
                        alt=""
                        on:error=move |_| missing.set(true)
                    />
                </Show>
            </span>

            <div>
                <h2>{heading}</h2>
                <p class="quiet">{lead}</p>
            </div>

            <button class="tap" title=t!("common.close") on:click=move |_| shut()>
                <Glyph icon=Icon::Close />
            </button>
        </header>
    }
}

/// A name against a value — and nothing whatever when there is no value.
///
/// Which is the rule these panels are read by. A file that never said who arranged it
/// has no arranger, and a row reading "Arranger —" is the panel inventing a question
/// nobody asked and answering it with a dash. So what is on screen is what is known,
/// and the length of the list is itself the answer to how much was tagged.
#[component]
pub fn Fact(
    name: String,
    value: Option<String>,
    /// Set for a value that is an identifier rather than words: an ISRC, a
    /// MusicBrainz id, a path. Monospaced and allowed to break anywhere, because
    /// none of them has a space to break at.
    #[prop(optional)]
    typed: bool,
) -> impl IntoView {
    value.map(|value| {
        view! {
            <div>
                <dt>{name}</dt>
                <dd class:typed=typed>{value}</dd>
            </div>
        }
    })
}

/// One figure over the word for it, and nothing where there is no figure.
///
/// The run they sit in fills whatever width it has with however many of them there
/// are, so a file that reports no bitrate leaves three across the panel rather than a
/// gap where the fourth was.
#[component]
pub fn Figure(value: Option<String>, name: String) -> impl IntoView {
    value.map(|value| {
        view! {
            <div>
                <span class="figure">{value}</span>
                <span class="quiet">{name}</span>
            </div>
        }
    })
}

/// What went wrong, where a panel has nothing else to show.
#[component]
pub fn Failed(why: crate::api::Failure) -> impl IntoView {
    view! { <p class="failure" role="alert">{crate::pages::said(&why)}</p> }
}
