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

    // Which list has been asked about, and the whole row rather than its identifier:
    // what the question has to say — private or public, and how much of it there is —
    // is on the row that opened it, and asking the server again for what is already on
    // screen would leave the sentence to arrive after the question.
    let asking = RwSignal::new(None::<Playlist>);

    // One dialogue for the screen and not one per row. It is a question about whichever
    // row was pressed, and twenty dialogues in the markup to ask it once is twenty.
    let remove = Callback::new(move |id: String| {
        spawn_local(async move {
            match api::remove_playlist(&id).await {
                Ok(()) => afresh(),
                Err(api::Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => failure.set(Some(super::said(&why))),
            }
        });
    });

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
                <Row one asking afresh=Callback::new(move |()| afresh()) on_expired />
            </For>
        </ul>

        <Show when=move || !theirs().is_empty()>
            <p class="part">{t!("playlists.public")}</p>
            <ul class="made">
                <For each=theirs key=|one| one.clone() let:one>
                    <Row one asking afresh=Callback::new(move |()| afresh()) on_expired />
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

        <Making making on_made=Callback::new(move |_: String| afresh()) on_expired />
        <Removing asking remove />
    }
}

/// One list as a row: what it is, how much of it there is, and what may be done to it.
#[component]
fn Row(
    one: Playlist,
    /// Where a row says which list is being asked about before it goes.
    asking: RwSignal<Option<Playlist>>,
    afresh: Callback<()>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let id = StoredValue::new(one.id.clone());
    let called = StoredValue::new(one.name.clone());
    // The row as it stands, for the question to read. Taken before the markup takes the
    // pieces of it.
    let row = StoredValue::new(one.clone());
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

    // A copy of it, made by the server: naming a list to copy is one field on the request
    // that makes one, so nothing here has to read a list out a page at a time — and a
    // public list of somebody else's can be taken this way, which is the whole of what
    // sharing one is for.
    let duplicate = move |_| {
        let asked = tocata::types::NewPlaylist {
            name: t!("playlists.copy_of", name = called.get_value()).to_string(),
            from: Some(id.get_value()),
            ..Default::default()
        };

        spawn_local(async move {
            match api::make_playlist(asked).await {
                Ok(_) => afresh.run(()),
                Err(api::Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => {}
            }
        });
    };

    // Asks rather than does. A list is gathered by hand, one track at a time, and it is
    // the one thing on this screen that a mis-aimed press cannot hand back.
    let delete = move |_| asking.set(Some(row.get_value()));

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
            // A menu on every row, because there is one thing worth offering about a
            // list that is not yours: taking a copy of it. The rest — who may see it,
            // and being rid of it — is the owner's.
            <span class="doing" on:click=move |e: web_sys::MouseEvent| e.stop_propagation()>
                <super::Dots title=t!("playlists.doings").to_string()>
                    <Show when=move || mine>
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
                    </Show>

                    <button class="menu-item" on:click=duplicate>
                        {t!("playlists.duplicate")}
                    </button>

                    <Show when=move || mine>
                        <hr />

                        <button class="menu-item risky" on:click=delete>
                            {t!("playlists.delete")}
                        </button>
                    </Show>
                </super::Dots>
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
    /// The name of the list that was made, for whoever wants to say so.
    on_made: Callback<String>,
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
            let asked = tocata::types::NewPlaylist {
                name: name.get_untracked(),
                tracks: (!held.is_empty()).then_some(held),
                ..Default::default()
            };

            match api::make_playlist(asked).await {
                Ok(made) => {
                    on_made.run(made.name);
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

/// Being rid of one, asked before it happens.
///
/// Nothing to type, unlike the same question about an account. What is lost is a handful
/// of rows somebody can gather again — the tracks themselves are not going anywhere — so
/// what this owes them is the name of the list and what it held, and not an exercise.
///
/// It says which one and how much of it there is because the question is asked in the
/// middle of the screen, where the row that would have answered both is no longer beside
/// it.
#[component]
fn Removing(
    /// The list being asked about, and nothing at all while nobody is being asked.
    asking: RwSignal<Option<Playlist>>,
    /// What to do once it is settled, by identifier.
    remove: Callback<String>,
) -> impl IntoView {
    let dialog: NodeRef<leptos::html::Dialog> = NodeRef::new();

    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        match asking.get() {
            Some(_) => {
                let _ = element.show_modal();
            }
            None => element.close(),
        }
    });

    // What it holds, with the preposition: the sentence it goes into reads about a list
    // and not about a number, and a list with nothing in it is not a list with a count of
    // nought.
    let held = move |one: &Playlist| match one.tracks {
        0 => t!("playlists.holds_none").to_string(),
        1 => t!("playlists.holds_one").to_string(),
        many => t!("playlists.holds_many", count = super::thousands(many)).to_string(),
    };

    let settled = move |_| {
        let Some(one) = asking.get() else { return };

        remove.run(one.id);
        asking.set(None);
    };

    view! {
        // Narrow, the same as the question about a library: a sentence and two answers.
        <dialog node_ref=dialog class="sheet narrow" on:close=move |_| asking.set(None)>
            <div class="sheet-body">
                <h2>
                    {move || {
                        asking
                            .get()
                            .map(|one| t!("playlists.delete_this", name = one.name).to_string())
                    }}
                </h2>

                // Private or public, because a list going is one thing when nobody else
                // could see it and another when the whole server could.
                <p class="sheet-lead">
                    {move || {
                        asking
                            .get()
                            .map(|one| {
                                let held = held(&one);

                                if one.public {
                                    t!("playlists.delete_public", held = held).to_string()
                                } else {
                                    t!("playlists.delete_private", held = held).to_string()
                                }
                            })
                    }}
                </p>
            </div>

            <div class="sheet-foot">
                // "Leave it be", not "Cancel": what the safe answer does is worth saying
                // when the other one cannot be undone.
                <button type="button" class="away" on:click=move |_| asking.set(None)>
                    {t!("common.keep")}
                </button>
                <button type="button" class="pill solid undoing" on:click=settled>
                    {t!("playlists.delete_yes")}
                </button>
            </div>
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
