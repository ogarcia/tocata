// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What you have marked, which until now was the one thing in Tocata you could put
//! there and never see.
//!
//! **Not a fifth kind of thing.** A favourite is the collection narrowed by who is
//! reading it, so this screen lists nothing of its own: the tracks are the tracks
//! table, the records are the shelf of covers, the names are the list of names — each
//! borrowed from the screen that owns that shape, over a listing asked for with
//! `starred=true`. What is new here is only the narrowing and the three counts.
//!
//! **One field over three listings.** The search sits above the tabs rather than
//! inside each of them, because it is one question — "among what I have marked" — and
//! a term typed while looking at tracks is still worth having when you look at the
//! records. Each listing follows it and starts already narrowed by it.
//!
//! **Nothing here marks anything yet.** What fills this screen today comes from a
//! client that speaks OpenSubsonic, where starring has always worked. Marking from the
//! panel is one control in three places — a row, a record, a name — and it wants
//! deciding as one thing rather than three.

use super::endless::{Fetch, Foot, Reel};
use crate::api;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Album, Artist, Favourites as Held, Track};

/// Which of the three is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Tracks,
    Albums,
    Artists,
}

#[component]
pub fn Favourites(on_expired: Callback<()>) -> impl IntoView {
    let held = RwSignal::new(None::<Held>);
    let kind = RwSignal::new(Kind::Tracks);
    // One field for all three, held here because it outlives each of them: a tab is
    // mounted while it is showing and gone the moment it is not.
    let needle = RwSignal::new(String::new());

    // The three counts, which the tabs carry before any of them is opened — and again
    // every time a heart is pressed in a panel opened from here, because a mark taken
    // off is a tab that now counts one fewer. Compared rather than merely read, so the
    // first run is the one fetch that draws the screen.
    let marks = use_context::<crate::drawer::Marks>();
    Effect::new(move |before: Option<u64>| {
        let now = marks.map(|marks| marks.0.get()).unwrap_or_default();

        if before != Some(now) {
            spawn_local(async move {
                match api::favourites().await {
                    Ok(counts) => held.set(Some(counts)),
                    Err(api::Failure::Unauthenticated) => on_expired.run(()),
                    // A count that did not arrive leaves the tabs bare and the listings
                    // alone. Every one of them says how much it holds in its own foot,
                    // so the screen is still readable without this.
                    Err(_) => {}
                }
            });
        }

        now
    });

    let counted = move |what: Kind| {
        held.get().map(|counts| match what {
            Kind::Tracks => counts.tracks,
            Kind::Albums => counts.albums,
            Kind::Artists => counts.artists,
        })
    };

    // A kind with nothing behind it gets no tab: a tab that opens an empty listing is
    // an invitation to somewhere there is nothing to see. Somebody who has marked only
    // records therefore sees one tab, not three with two of them reading nought.
    //
    // Until the counts arrive — and if they never do — all three stand. The listings do
    // not depend on that answer, so a screen that hid them over a figure it failed to
    // fetch would hide favourites that are perfectly readable.
    let worth = move |what: Kind| counted(what).is_none_or(|many| many > 0);

    // Nothing marked at all, which is not the same as one kind being empty: there is no
    // tab to show and nothing to list, so the screen says where marks come from instead.
    let bare = move || {
        held.get()
            .is_some_and(|counts| counts.tracks == 0 && counts.albums == 0 && counts.artists == 0)
    };

    // Whatever is showing has to be a kind that is there. Tracks is where this opens,
    // and somebody whose favourites are all records would otherwise land on an empty
    // listing behind a tab that is not drawn.
    Effect::new(move |_| {
        if !worth(kind.get_untracked())
            && let Some(first) = [Kind::Tracks, Kind::Albums, Kind::Artists]
                .into_iter()
                .find(|what| worth(*what))
        {
            kind.set(first);
        }
    });

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.favourites")}</h1>
                <p class="quiet lead">{move || summary(held.get())}</p>
            </div>

            // Nothing to search through while nothing is marked.
            <Show when=move || !bare()>
                <div class="finding">
                    <super::Search
                        placeholder=t!("favourites.search")
                        value=needle
                        on_type=Callback::new(move |typed| needle.set(typed))
                        on_clear=Callback::new(move |()| needle.set(String::new()))
                    />
                </div>
            </Show>
        </header>

        // The ways into the marks, with what each holds beside it, and only the ones
        // there is something behind. Not a tab strip like a drawer's: those divide one
        // thing into parts, and these are lists that happen to be about one account.
        <Show when=move || !bare()>
            <div class="picking">
                <Show when=move || worth(Kind::Tracks)>
                    <Way
                        what=Kind::Tracks
                        kind
                        counted=Signal::derive(move || counted(Kind::Tracks))
                    />
                </Show>
                <Show when=move || worth(Kind::Albums)>
                    <Way
                        what=Kind::Albums
                        kind
                        counted=Signal::derive(move || counted(Kind::Albums))
                    />
                </Show>
                <Show when=move || worth(Kind::Artists)>
                    <Way
                        what=Kind::Artists
                        kind
                        counted=Signal::derive(move || counted(Kind::Artists))
                    />
                </Show>
            </div>
        </Show>

        // One at a time, and mounted only while it is showing: each carries its own
        // reel, and three of them fetching at once would be two listings nobody is
        // looking at.
        <Show when=move || !bare() && kind.get() == Kind::Tracks>
            <Songs needle on_expired />
        </Show>
        <Show when=move || !bare() && kind.get() == Kind::Albums>
            <Records needle on_expired />
        </Show>
        <Show when=move || !bare() && kind.get() == Kind::Artists>
            <Names needle on_expired />
        </Show>

        // With nothing marked anywhere there is no listing to draw an empty state for,
        // so this stands in for all three. The news itself is already under the title —
        // what is left to say is where marks come from, which on a self-hosted server
        // is the part nobody can guess.
        <Show when=bare>
            <div class="nothing">
                <p>{t!("favourites.whence")}</p>
            </div>
        </Show>
    }
}

/// One of the three, with how many it holds.
#[component]
fn Way(what: Kind, kind: RwSignal<Kind>, counted: Signal<Option<i64>>) -> impl IntoView {
    let word = move || match what {
        Kind::Tracks => t!("nav.tracks").to_string(),
        Kind::Albums => t!("nav.albums").to_string(),
        Kind::Artists => t!("nav.artists").to_string(),
    };

    view! {
        <button
            class:chosen=move || kind.get() == what
            on:click=move |_| kind.set(what)
        >
            {word}
            // Absent rather than nought while the answer is on its way: a tab that
            // says nothing is waiting, and one that says nought is wrong.
            {move || {
                counted
                    .get()
                    .map(|many| view! { <span class="figure">{super::thousands(many)}</span> })
            }}
        </button>
    }
}

fn tracks(search: String, offset: usize, limit: i64) -> super::endless::Window<Track> {
    Box::pin(async move {
        api::starred_tracks(&search, offset, limit)
            .await
            .map(|page| (page.total, page.tracks))
    })
}

fn albums(search: String, offset: usize, limit: i64) -> super::endless::Window<Album> {
    Box::pin(async move {
        api::starred_albums(&search, offset, limit)
            .await
            .map(|page| (page.total, page.albums))
    })
}

fn artists(search: String, offset: usize, limit: i64) -> super::endless::Window<Artist> {
    Box::pin(async move {
        api::starred_artists(&search, offset, limit)
            .await
            .map(|page| (page.total, page.artists))
    })
}

#[component]
fn Songs(needle: RwSignal<String>, on_expired: Callback<()>) -> impl IntoView {
    let reel = following(needle, tracks as Fetch<Track>, on_expired);

    view! {
        <Wanting reel needle which=Kind::Tracks />
        <super::tracks::Table reel starred=true />
        <Reading reel />
    }
}

#[component]
fn Records(needle: RwSignal<String>, on_expired: Callback<()>) -> impl IntoView {
    let reel = following(needle, albums as Fetch<Album>, on_expired);

    view! {
        <Wanting reel needle which=Kind::Albums />
        <super::albums::Shelf reel />
        <Reading reel />
    }
}

#[component]
fn Names(needle: RwSignal<String>, on_expired: Callback<()>) -> impl IntoView {
    let reel = following(needle, artists as Fetch<Artist>, on_expired);

    view! {
        <Wanting reel needle which=Kind::Artists />
        <super::artists::Crowd reel />
        <Reading reel />
    }
}

/// A listing that starts narrowed by the field above the tabs and follows it after.
///
/// Two halves of one thing. The subject it starts with is what is in the field, so a
/// tab opened with a term already typed asks for the narrowed listing once rather than
/// fetching everything and then narrowing it — and `typing` is set to match, or the
/// effect below would see them differ and ask a second time for what just arrived.
fn following<T: Send + Sync + 'static>(
    needle: RwSignal<String>,
    window: Fetch<T>,
    on_expired: Callback<()>,
) -> Reel<T> {
    let asked = needle.get_untracked();
    let reel = Reel::about(asked.clone(), window, on_expired);
    reel.typing.set(asked);

    Effect::new(move |_| {
        let now = needle.get();

        if reel.typing.get_untracked() != now {
            reel.typed(now);
        }
    });

    // And again whenever a mark changes anywhere, which on this screen means a row that
    // has stopped being one of its rows: a heart pressed in a panel opened from this
    // very listing. The count of changes is compared rather than merely read, so the
    // first run — which is the listing arriving — does not fetch it twice.
    if let Some(marks) = use_context::<crate::drawer::Marks>() {
        Effect::new(move |before: Option<u64>| {
            let now = marks.0.get();

            if before.is_some_and(|was| was != now) {
                reel.afresh();
            }

            now
        });
    }

    reel
}

/// The foot of any of the three listings: how much has been read, and the way to
/// carry on where something stopped.
#[component]
fn Reading<T: Send + Sync + 'static>(reel: Reel<T>) -> impl IntoView {
    view! {
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

/// What stands where the rows would have been, when there are none.
///
/// Two different pieces of news and the screens about the collection cannot say
/// either: an empty collection is a scan away, and an empty shelf of favourites is not
/// something the server can do anything about. So this says where marks come from,
/// which on a self-hosted server is the answer somebody actually needs.
#[component]
fn Wanting<T: Send + Sync + 'static>(
    reel: Reel<T>,
    needle: RwSignal<String>,
    which: Kind,
) -> impl IntoView {
    let empty = move || reel.total.get().is_some_and(|held| held == 0);
    let searching = move || !needle.with(String::is_empty);

    view! {
        // A failure that left nothing on screen. A listing that stopped partway says
        // so in its own foot instead, beside the way to carry on.
        {move || {
            reel.failure
                .get()
                .filter(|_| reel.rows.with(Vec::is_empty))
                .map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        <Show when=move || empty() && !searching()>
            <div class="nothing">
                <p>
                    {move || match which {
                        Kind::Tracks => t!("favourites.no_tracks").to_string(),
                        Kind::Albums => t!("favourites.no_albums").to_string(),
                        Kind::Artists => t!("favourites.no_artists").to_string(),
                    }}
                </p>
                <p class="quiet">{t!("favourites.whence")}</p>
            </div>
        </Show>

        <Show when=move || empty() && searching()>
            <div class="nothing">
                <p>{t!("favourites.none_found")}</p>
            </div>
        </Show>
    }
}

/// The line under the title: how much of each, and how long the tracks run.
///
/// Joined rather than laid out, so an account with no marked albums does not read as
/// one whose albums are blank — and nothing at all until the answer arrives, which the
/// tabs are already saying by carrying no figures.
fn summary(held: Option<Held>) -> String {
    let Some(held) = held else {
        return t!("common.loading").to_string();
    };

    if held.tracks == 0 && held.albums == 0 && held.artists == 0 {
        return t!("favourites.none_at_all").to_string();
    }

    let mut said = Vec::new();

    for (count, one, many) in [
        (
            held.tracks,
            "collection.one_track",
            "collection.many_tracks",
        ),
        (
            held.albums,
            "collection.one_album",
            "collection.many_albums",
        ),
        (
            held.artists,
            "collection.one_artist",
            "collection.many_artists",
        ),
    ] {
        if count == 1 {
            said.push(t!(one).to_string());
        } else if count > 1 {
            said.push(t!(many, count = super::thousands(count)).to_string());
        }
    }

    if let Some(runs) = held.duration {
        said.push(super::runs(runs));
    }

    said.join(" · ")
}
