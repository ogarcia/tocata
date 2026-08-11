// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! One list, and its running order.
//!
//! A panel over the screen of lists rather than a screen of its own, like every other
//! thing in this collection you can open: what is behind keeps its place, and a list is
//! something you have open rather than somewhere you have gone.
//!
//! **The same rows as the queue** — a handle, the position, the track, and a way out —
//! because reordering a list and reordering the queue are the same gesture and nobody
//! should have to learn it twice. Which is also why it belongs in a drawer: that is the
//! width those rows were drawn for.
//!
//! **Nothing to save.** The name, who may see it, the order and what is in it all go as
//! they are changed, and the foot says so once. A button there would imply the rest is
//! unsaved.
//!
//! **Positions travel from the server rather than being counted on screen.** A list can
//! hold tracks from a library this account cannot reach: they are not drawn, they keep
//! their place, and a move sends the positions the server itself reported. A panel that
//! sent its own row numbers would quietly throw them out.
//!
//! Reading somebody else's public list is reading, so the field, the handles and the
//! crosses are simply absent on one — including the column the handles would sit in.

use super::{Failed, Frame, Head};
use crate::api;
use crate::icon::{Glyph, Icon};
use crate::pages;
use crate::pages::endless::{Fetch, Foot, Reel, Window};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Playlist, PlaylistChanges, PlaylistEntry};

/// How tall a row is, in pixels, for working out where a dragged one would land.
///
/// The same 61 as the queue, and for the same reason: twelve pixels of padding either
/// side of two lines at 1.3, which is the shape every row in this panel has. Written
/// here as well as in the stylesheet, which is the one thing in this module that has to
/// be kept in step with it — being a little out only means a long drag lands a row early
/// or late, so this is worth knowing rather than worth measuring on every move.
const ROW: f64 = 61.0;

fn window(id: String, offset: usize, limit: i64) -> Window<PlaylistEntry> {
    Box::pin(async move {
        api::playlist_tracks(&id, offset, limit)
            .await
            .map(|page| (page.total, page.tracks))
    })
}

#[component]
pub fn OnePlaylist(id: String) -> impl IntoView {
    let id = StoredValue::new(id);
    let about = RwSignal::new(None::<Playlist>);
    let failure = RwSignal::new(None::<api::Failure>);

    // What is being dragged and where it would land, in rows on screen. Held by the
    // list rather than by a row, because the gap that opens up is every other row
    // stepping aside.
    let moving = RwSignal::new(None::<(usize, usize)>);

    // Picked up here and not inside the requests below: see `watching`.
    let lists = super::watching::<super::Lists>().map(|lists| lists.0);

    let read = move || {
        spawn_local(async move {
            match api::playlist(&id.get_value()).await {
                Ok(read) => about.set(Some(read)),
                Err(why) => failure.set(Some(why)),
            }
        });
    };

    Effect::new(move |first: Option<()>| {
        if first.is_none() {
            read();
        }
    });

    let reel = Reel::about(
        id.get_value(),
        window as Fetch<PlaylistEntry>,
        Callback::new(move |()| failure.set(Some(api::Failure::Unauthenticated))),
    );

    // Every change to the order or the contents is followed by reading both again: the
    // figures in the head are the server's arithmetic and the positions are the
    // server's numbering, and a panel that patched either in place could disagree with
    // it about what just happened.
    let again = move || {
        read();
        reel.afresh();
        super::tell(lists);
    };

    let mine = move || about.get().is_some_and(|about| about.mine);

    // The rows with the place each holds on screen beside it, which is what a drag is
    // measured in. Worked out here rather than inside the view, where the turbofish a
    // `collect` needs is not something the macro can read.
    let numbered = move || {
        reel.rows
            .get()
            .into_iter()
            .enumerate()
            .collect::<Vec<(usize, PlaylistEntry)>>()
    };

    // Saved when the field is left rather than as it is typed: a rename is one decision,
    // and a request per keystroke would be the row's date moving under somebody's hands.
    //
    // Nothing is saved where nothing changed, and nothing where the field was emptied: a
    // list keeps the name it has rather than losing it to a backspace, and putting the
    // name back is what makes the heading say so again.
    let rename = move |written: String| {
        let written = written.trim().to_string();
        let before = about.get_untracked().map(|about| about.name);

        if written.is_empty() || before.as_deref() == Some(written.as_str()) {
            about.update(|about| {
                if let (Some(about), Some(before)) = (about.as_mut(), before) {
                    about.name = before;
                }
            });
            return;
        }

        spawn_local(async move {
            let changes = PlaylistChanges {
                name: Some(written),
                ..Default::default()
            };

            if let Ok(read) = api::change_playlist(&id.get_value(), &changes).await {
                about.set(Some(read));
                super::tell(lists);
            }
        });
    };

    let play = move |shuffle: bool| {
        let player = crate::player::player();

        spawn_local(async move {
            if let Ok(page) = api::playlist_tracks(&id.get_value(), 0, 200).await {
                let queue: Vec<String> = page
                    .tracks
                    .into_iter()
                    .filter(|entry| !entry.track.missing)
                    .map(|entry| entry.track.id)
                    .collect();

                if queue.is_empty() {
                    return;
                }

                // Shuffled is an order, not a mode. Putting the list on and then pressing
                // the queue's own shuffle would leave the first track first — that button
                // mixes what is *coming*, which is everything after the one now sounding
                // — and it would leave the queue lit as mixed, offering to put back an
                // order nobody asked to keep. This draws the order first and hands over a
                // plain queue.
                if shuffle {
                    player.play_mixed(queue);
                } else {
                    player.play(queue, 0);
                }
            }
        });
    };

    view! {
        <Frame>
            <Head
                icon=Icon::Playlists
                heading=Signal::derive(move || {
                    about
                        .get()
                        .map(|about| about.name)
                        .unwrap_or_else(|| t!("common.loading").to_string())
                })
                renaming=Signal::derive(mine)
                on_renamed=Callback::new(rename)
                lead=move || summary(about.get())
            />

            <div class="leafing">
                {move || failure.get().map(|why| view! { <Failed why /> })}

                // Who may see it, which is the one thing about a list that other
                // people can notice. Its name is edited in the heading above.
                <Show when=mine>
                    <Seen about id=id.get_value() />
                </Show>

                {move || {
                    reel.failure
                        .get()
                        .filter(|_| reel.rows.with(Vec::is_empty))
                        .map(|why| view! { <p class="failure" role="alert">{why}</p> })
                }}

                <Show when=move || !reel.rows.with(Vec::is_empty)>
                    <ul class="order" class:reordering=move || moving.get().is_some()>
                        <For each=numbered key=|row| (row.1.at, row.1.track.id.clone()) let:row>
                            <Entry
                                nth=row.0
                                entry=row.1
                                rows=reel.rows
                                mine=Signal::derive(mine)
                                id=id.get_value()
                                moving
                                after=Callback::new(move |()| again())
                            />
                        </For>
                    </ul>

                    <Show when=mine>
                        <p class="quiet remark">{t!("playlists.saved_as_you_go")}</p>
                    </Show>
                </Show>

                <Show when=move || reel.total.get().is_some_and(|held| held == 0)>
                    <div class="nothing">
                        <p>{t!("playlists.empty")}</p>
                        <p class="quiet">{t!("playlists.fill_it")}</p>
                    </div>
                </Show>

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
            </div>

            <footer>
                <span class="quiet">{move || whose(about.get())}</span>

                <span class="deeds">
                    <Show when=move || about.get().is_some_and(|about| about.tracks > 0)>
                        <button on:click=move |_| play(true)>{t!("queue.shuffle")}</button>
                        <button class="leading" on:click=move |_| play(false)>
                            {t!("playlists.play")}
                        </button>
                    </Show>
                </span>
            </footer>
        </Frame>
    }
}

/// One entry: where it sits, what it is, and the two things that can be done to it.
#[component]
fn Entry(
    /// Which row on screen this is, which is what a drag is measured in.
    nth: usize,
    entry: PlaylistEntry,
    /// Everything on screen, so a drag can turn a row it landed on into the position
    /// the server knows it by.
    rows: RwSignal<Vec<PlaylistEntry>>,
    mine: Signal<bool>,
    id: String,
    moving: RwSignal<Option<(usize, usize)>>,
    after: Callback<()>,
) -> impl IntoView {
    let player = crate::player::player();
    let hold = crate::drag::Drag::new();
    let id = StoredValue::new(id);
    let at = entry.at;
    let track = entry.track;
    let which = StoredValue::new(track.id.clone());
    let gone = track.missing;

    let sounding = move || player.current().as_deref() == Some(&which.get_value());

    // Which row on screen the finger is over, and then which position that row holds.
    // Two steps and not one, because the two only agree while every entry is visible:
    // a list holding tracks from a library this account cannot reach has gaps in its
    // numbering, and a drop has to name the number the server uses.
    let landing = move |down: f64| {
        let travelled = (down / ROW).round() as i64;
        let last = rows.with_untracked(Vec::len).saturating_sub(1);

        (nth as i64 + travelled).clamp(0, last as i64) as usize
    };

    let position_of = move |row: usize| rows.with_untracked(|rows| rows.get(row).map(|e| e.at));

    // Making room: the rows between where the held one came from and where it is going
    // step one place the other way, which is the gap travelling with the finger.
    let making_room = move || {
        let Some((from, to)) = moving.get() else {
            return 0.0;
        };

        if nth == from {
            0.0
        } else if from < to && nth > from && nth <= to {
            -ROW
        } else if from > to && nth >= to && nth < from {
            ROW
        } else {
            0.0
        }
    };

    // The whole list from this row down, so pressing play on the fourth track keeps the
    // three above it behind: going back is part of what a running order is.
    let start = move |_| {
        if gone {
            return;
        }

        if sounding() {
            player.toggle();
            return;
        }

        let queue: Vec<String> = rows.with_untracked(|rows| {
            rows.iter()
                .filter(|entry| !entry.track.missing)
                .map(|entry| entry.track.id.clone())
                .collect()
        });

        if let Some(from) = queue.iter().position(|one| *one == which.get_value()) {
            player.play(queue, from);
        }
    };

    let drop_it = move |_| {
        spawn_local(async move {
            if api::drop_from_playlist(&id.get_value(), at).await.is_ok() {
                after.run(());
            }
        });
    };

    view! {
        <li
            class:gone=gone
            class:sounding=sounding
            class:lifted=move || hold.going.get()
            class:borrowed=move || !mine.get()
            style=move || {
                if hold.going.get() {
                    format!("transform: translateY({}px)", hold.down())
                } else {
                    format!("transform: translateY({}px)", making_room())
                }
            }
        >
            // Only on your own list — and the column goes with it, rather than standing
            // empty down the left of one you are only reading.
            <Show when=move || mine.get()>
                <button
                    class="handle"
                    title=t!("playlists.reorder")
                    on:pointerdown=move |e: web_sys::PointerEvent| {
                        e.stop_propagation();
                        hold.begin(&e);
                    }
                    on:pointermove=move |e: web_sys::PointerEvent| {
                        hold.moved(&e);
                        let to = landing(hold.down());
                        moving.set((to != nth).then_some((nth, to)));
                    }
                    on:pointerup=move |_| {
                        moving.set(None);

                        if let Some((_, down)) = hold.end() {
                            let to = landing(down);

                            if to != nth
                                && let Some(there) = position_of(to)
                            {
                                spawn_local(async move {
                                    if api::move_in_playlist(&id.get_value(), at, there)
                                        .await
                                        .is_ok()
                                    {
                                        after.run(());
                                    }
                                });
                            }
                        }
                    }
                    on:pointercancel=move |_| {
                        moving.set(None);
                        hold.end();
                    }
                >
                    <Glyph icon=Icon::Handle />
                </button>
            </Show>

            // The list's own numbering, and the way to play from here. A track whose
            // file has gone keeps its number and offers nothing: it cannot play, and it
            // is exactly the row somebody came to see.
            <Show
                when=move || !gone
                fallback=move || view! { <span class="starter">{at + 1}</span> }
            >
                <button class="starter" title=t!("player.play_this") on:click=start>
                    <span class="resting">
                        <Show when=sounding fallback=move || view! { {at + 1} }>
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
            </Show>

            <span class="what">
                {track.title.clone()}
                <span class="by">{credited(&track)}</span>
            </span>

            <span class="figure">
                {track.duration.map(pages::length).unwrap_or_else(|| pages::MISSING.to_string())}
            </span>

            <Show when=move || mine.get()>
                <button class="drop" title=t!("playlists.take_out") on:click=drop_it>
                    <Glyph icon=Icon::Close />
                </button>
            </Show>
        </li>
    }
}

/// Who may see it: a switch, because it is a state and not an errand.
#[component]
fn Seen(about: RwSignal<Option<Playlist>>, id: String) -> impl IntoView {
    let id = StoredValue::new(id);
    let public = move || about.get().is_some_and(|about| about.public);
    // Its own, because this is its own component: the screen of lists is behind this
    // panel and a row of it says whether everybody here can see the list.
    let lists = super::watching::<super::Lists>().map(|lists| lists.0);

    let switch = move |_| {
        let wanted = !public();

        about.update(|about| {
            if let Some(about) = about {
                about.public = wanted;
            }
        });

        spawn_local(async move {
            let changes = PlaylistChanges {
                public: Some(wanted),
                ..Default::default()
            };

            match api::change_playlist(&id.get_value(), &changes).await {
                Ok(read) => {
                    about.set(Some(read));
                    super::tell(lists);
                }
                Err(_) => {
                    about.update(|about| {
                        if let Some(about) = about {
                            about.public = !wanted;
                        }
                    });
                }
            }
        });
    };

    view! {
        <p class="privacy">
            <button class:shared=public aria-pressed=move || public().to_string() on:click=switch>
                {move || {
                    if public() {
                        t!("playlists.is_public").to_string()
                    } else {
                        t!("playlists.is_private").to_string()
                    }
                }}
            </button>

            <span class="quiet">
                {move || {
                    if public() {
                        t!("playlists.public_note").to_string()
                    } else {
                        t!("playlists.private_note").to_string()
                    }
                }}
            </span>
        </p>
    }
}

/// Who made the track, and whether its file is still there.
fn credited(track: &tocata::types::Track) -> String {
    match (track.artists.clone(), track.missing) {
        (Some(who), true) => format!("{who} · {}", t!("playlists.will_be_skipped")),
        (Some(who), false) => who,
        (None, true) => t!("playlists.will_be_skipped").to_string(),
        (None, false) => pages::MISSING.to_string(),
    }
}

/// The line under the name: how much of it there is, and when it last changed.
fn summary(about: Option<Playlist>) -> String {
    let Some(about) = about else {
        return String::new();
    };

    let mut said = vec![if about.tracks == 1 {
        t!("collection.one_track").to_string()
    } else {
        t!(
            "collection.many_tracks",
            count = pages::thousands(about.tracks)
        )
        .to_string()
    }];

    if let Some(runs) = about.duration {
        said.push(pages::runs(runs));
    }

    if about.missing > 0 {
        said.push(if about.missing == 1 {
            t!("playlists.one_missing").to_string()
        } else {
            t!("playlists.many_missing", count = about.missing).to_string()
        });
    }

    said.push(t!("playlists.changed", when = pages::since(&about.changed)).to_string());

    said.join(" · ")
}

/// What the foot of the panel says: whose list this is, which is the one fact about it
/// that is not on any of its rows.
fn whose(about: Option<Playlist>) -> String {
    match about {
        Some(about) if about.mine => t!("playlists.yours").to_string(),
        Some(about) => t!("playlists.made_by", who = about.owner).to_string(),
        None => String::new(),
    }
}
