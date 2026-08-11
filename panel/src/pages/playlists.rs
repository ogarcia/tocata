// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The lists somebody made, which is the one part of a collection the server did not
//! put there.
//!
//! **Rows and not cards.** A list is known by its name and judged by its length, and
//! both read better in a line than under a square with no artwork to put in it.
//!
//! **Two groups, because there are two kinds of row here and only one of them is
//! yours.** What you made can be changed; what somebody else made public can be read
//! and played and nothing else, so those rows carry the owner's name where yours carry
//! what may be done to them. A server where nobody shared anything shows one group and
//! never mentions the other.
//!
//! Not paged and not searched: a collection has thousands of tracks and an account has
//! a handful of lists.

use crate::api;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Playlist, PlaylistChanges};

#[component]
pub fn Playlists(on_expired: Callback<()>) -> impl IntoView {
    let held = RwSignal::new(None::<Vec<Playlist>>);
    let failure = RwSignal::new(None::<String>);
    let making = RwSignal::new(false);

    // Read again after every change rather than patched in place. A list's row carries
    // four figures the server works out, and a screen that guessed at them after a
    // rename would be a screen that can disagree with the server about what it just
    // did.
    let afresh = move || {
        spawn_local(async move {
            match api::playlists().await {
                Ok(read) => held.set(Some(read.playlists)),
                Err(api::Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => failure.set(Some(super::said(&why))),
            }
        });
    };

    // Read on arriving, and again whenever a panel over this screen changes a list:
    // renaming one, publishing it or adding to it moves figures this screen is showing.
    // Compared rather than merely read, so the first run is the one fetch that draws it.
    let changed = use_context::<crate::drawer::Lists>();
    Effect::new(move |before: Option<u64>| {
        let now = changed.map(|changed| changed.0.get()).unwrap_or_default();

        if before != Some(now) {
            afresh();
        }

        now
    });

    let mine = move || {
        held.get()
            .map(|all| all.into_iter().filter(|one| one.mine).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let theirs = move || {
        held.get()
            .map(|all| all.into_iter().filter(|one| !one.mine).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let none_at_all = move || held.get().is_some_and(|all| all.is_empty());

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.playlists")}</h1>
                <p class="quiet lead">{move || summary(held.get())}</p>
            </div>

            <button class="pill solid" on:click=move |_| making.set(true)>
                {t!("playlists.new")}
            </button>
        </header>

        {move || {
            failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        // The heading over your own only appears when there is a second group to tell
        // it from. On a server where nobody shared anything, a heading saying "yours"
        // over the only list there is says nothing.
        <Show when=move || !theirs().is_empty()>
            <p class="part">{t!("playlists.mine")}</p>
        </Show>

        <ul class="made">
            // Keyed on the whole row and not on its identifier. A keyed `For` leaves a
            // row alone while its key is the same, and `Row` is handed a value rather
            // than a signal — so publishing a list, renaming it or adding to it changed
            // nothing on screen until something made its id disappear. Everything about
            // it is the key, so a list that reads differently is a row drawn again.
            <For each=mine key=|one| one.clone() let:one>
                <Row one afresh=Callback::new(move |()| afresh()) on_expired />
            </For>
        </ul>

        <Show when=move || !theirs().is_empty()>
            <p class="part">{t!("playlists.public")}</p>
            <ul class="made">
                <For each=theirs key=|one| one.clone() let:one>
                    <Row one afresh=Callback::new(move |()| afresh()) on_expired />
                </For>
            </ul>
        </Show>

        // No button in here: the way to make one is four lines above. What this owes
        // the reader is the way in nobody finds on their own.
        <Show when=none_at_all>
            <div class="nothing">
                <p>{t!("playlists.none")}</p>
                <p class="quiet">{t!("playlists.whence")}</p>
            </div>
        </Show>

        <Making making on_made=Callback::new(move |()| afresh()) on_expired />
    }
}

/// One list as a row: what it is, how much of it there is, and what may be done to it.
#[component]
fn Row(one: Playlist, afresh: Callback<()>, on_expired: Callback<()>) -> impl IntoView {
    let id = StoredValue::new(one.id.clone());
    let mine = one.mine;
    let public = RwSignal::new(one.public);

    // What the second line says. A description if its owner wrote one; otherwise, and
    // only where something is wrong, what is wrong — a list whose files have gone is
    // one somebody can go and settle, and saying so where the description would be is
    // saying it where they are already looking.
    let said = match (one.comment.clone(), one.missing) {
        (Some(comment), _) => Some(comment),
        (None, 0) => None,
        (None, 1) => Some(t!("playlists.one_missing").to_string()),
        (None, gone) => Some(t!("playlists.many_missing", count = gone).to_string()),
    };

    let switch = move |wanted: bool| {
        public.set(wanted);

        spawn_local(async move {
            let changes = PlaylistChanges {
                public: Some(wanted),
                ..Default::default()
            };

            match api::change_playlist(&id.get_value(), &changes).await {
                Ok(_) => afresh.run(()),
                Err(api::Failure::Unauthenticated) => on_expired.run(()),
                // Back where it was, and nothing said: what failed is a word on a row,
                // and the word going back is the answer.
                Err(_) => public.set(!wanted),
            }
        });
    };

    let delete = move |_| {
        spawn_local(async move {
            match api::remove_playlist(&id.get_value()).await {
                Ok(()) => afresh.run(()),
                Err(api::Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => {}
            }
        });
    };

    view! {
        // The whole row opens the list, the way a row does in Artists and in Genres. It
        // was the name in a button, and every button in this panel is a centred
        // `inline-flex` by one rule at the top of the sheet — so the name sat in the
        // middle of its column. Nothing here wants to fight that rule: a row that opens
        // something is a shape this panel already has, and it gives a finger the whole
        // row to land on instead of eight characters of it.
        <li on:click=move |_| {
            crate::drawer::open(crate::drawer::Open::Playlist(id.get_value()))
        }>
            <span class="what">
                <span>{one.name}</span>
                {said.map(|said| view! { <span class="quiet">{said}</span> })}
            </span>

            // Four cells of the grid, wrapped only so that they can become one line
            // together when the columns go — the same trick the roster of accounts
            // plays, and on a wide screen the wrapper is not there as far as layout is
            // concerned.
            //
            // Not `.facts`, which is the panel's block of name-against-value rows and
            // gives every child of its own a border and a padding: a word being on the
            // shared list means it already has a meaning, not that it is free.
            <span class="sums">
                <span class="figure">{counted(one.tracks)}</span>
                <span class="figure">{one.duration.map(super::runs).unwrap_or_default()}</span>
                <span class="figure">{super::since(&one.changed)}</span>

                // Whose it is, said in whichever way is the useful one. On yours, that
                // everybody here can see it — and only when they can, since private is
                // what a list is unless somebody said otherwise. On somebody else's,
                // whose it is.
                <span class="owning">
                    <Show
                        when=move || mine
                        fallback=move || view! { <span class="quiet">{one.owner.clone()}</span> }
                    >
                        <Show when=move || public.get()>
                            <span class="shared">{t!("playlists.is_public")}</span>
                        </Show>
                    </Show>
                </span>
            </span>

            // Nothing to offer on a list that is not yours: it can be read and played,
            // and a menu of things that would all be refused is worse than no menu.
            //
            // The dots are the panel's own, which is what puts the menu over whatever
            // clips this row rather than inside it.
            // Keeps its presses to itself: the row opens the list, and reaching for the
            // menu must not also open it.
            <span class="doing" on:click=move |e: web_sys::MouseEvent| e.stop_propagation()>
                <Show when=move || mine>
                    <super::Dots title=t!("playlists.doings").to_string()>
                        <button
                            class="menu-item"
                            on:click=move |_| switch(!public.get_untracked())
                        >
                            {move || {
                                if public.get() {
                                    t!("playlists.make_private").to_string()
                                } else {
                                    t!("playlists.make_public").to_string()
                                }
                            }}
                        </button>

                        <hr />

                        <button class="menu-item risky" on:click=delete>
                            {t!("playlists.delete")}
                        </button>
                    </super::Dots>
                </Show>
            </span>
        </li>
    }
}

/// Making one: a name, and nothing else.
///
/// What it says about itself and what goes in it are both changed where the list itself
/// is, which is where somebody has it in front of them. A form asking for three things
/// at once to make an empty list would be asking two of them of somebody who has not
/// seen it yet.
#[component]
pub fn Making(
    making: RwSignal<bool>,
    /// What it holds from the start. Empty from the button on this screen, and the whole
    /// queue from the foot of the queue — read when the form is sent rather than when the
    /// sheet opened, because the music carries on while somebody types a name.
    #[prop(optional, into)]
    tracks: Signal<Vec<String>>,
    /// The line under the title, for the one caller whose list will not be empty.
    #[prop(optional, into)]
    lead: Option<Signal<String>>,
    on_made: Callback<()>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let dialog: NodeRef<leptos::html::Dialog> = NodeRef::new();
    let name = RwSignal::new(String::new());
    let failure = RwSignal::new(None::<String>);
    let waiting = RwSignal::new(false);

    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        if making.get() {
            name.set(String::new());
            failure.set(None);
            let _ = element.show_modal();
        } else {
            element.close();
        }
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        waiting.set(true);
        failure.set(None);

        spawn_local(async move {
            let held = tracks.get_untracked();
            let held = (!held.is_empty()).then_some(held);

            match api::make_playlist(name.get_untracked(), held).await {
                Ok(_) => {
                    on_made.run(());
                    making.set(false);
                }
                Err(api::Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => failure.set(Some(super::said(&why))),
            }
            waiting.set(false);
        });
    };

    view! {
        <dialog node_ref=dialog class="sheet" on:close=move |_| making.set(false)>
            <form on:submit=submit>
                <div class="sheet-body">
                    <h2>{t!("playlists.new")}</h2>
                    <p class="sheet-lead">
                        {move || match lead {
                            Some(lead) => lead.get(),
                            None => t!("playlists.new_lead").to_string(),
                        }}
                    </p>

                    <div class="sheet-content">
                        <label>
                            <span>{t!("playlists.name")}</span>
                            <input
                                autocomplete="off"
                                autofocus
                                required
                                prop:value=name
                                on:input:target=move |e| name.set(e.target().value())
                            />
                        </label>
                    </div>
                </div>

                <div class="sheet-foot">
                    <button
                        type="button"
                        class="away"
                        disabled=waiting
                        on:click=move |_| making.set(false)
                    >
                        {t!("common.cancel")}
                    </button>
                    <button type="submit" class="pill solid" disabled=waiting>
                        {move || {
                            if waiting.get() {
                                t!("login.working").to_string()
                            } else {
                                t!("playlists.make").to_string()
                            }
                        }}
                    </button>
                </div>

                {move || {
                    failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
                }}
            </form>
        </dialog>
    }
}

/// How many lists there are and what they add up to, for the line under the title.
///
/// Only over your own. What somebody else shared is not part of how much you have, and
/// adding the two together would be a figure about nothing in particular.
fn summary(held: Option<Vec<Playlist>>) -> String {
    let Some(held) = held else {
        return t!("common.loading").to_string();
    };

    let mine: Vec<&Playlist> = held.iter().filter(|one| one.mine).collect();

    if mine.is_empty() {
        return t!("playlists.none_yet").to_string();
    }

    let lists = if mine.len() == 1 {
        t!("playlists.one").to_string()
    } else {
        t!("playlists.many", count = mine.len()).to_string()
    };

    let tracks: i64 = mine.iter().map(|one| one.tracks).sum();
    let runs: i64 = mine.iter().filter_map(|one| one.duration).sum();

    let mut said = vec![lists, counted(tracks)];

    if runs > 0 {
        said.push(super::runs(runs));
    }

    said.join(" · ")
}

/// How many tracks, in the words the rest of the collection uses for it.
fn counted(tracks: i64) -> String {
    if tracks == 1 {
        t!("collection.one_track").to_string()
    } else {
        t!("collection.many_tracks", count = super::thousands(tracks)).to_string()
    }
}
