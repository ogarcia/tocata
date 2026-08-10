// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Everything about one record.
//!
//! One tab, unlike a track's three, because there is only one place the answer comes
//! from: a record is not a file, so there are no tags to read out of it and no words
//! to look for beside it. What it is, what is on it and who played are all in the
//! database, and they arrive together.
//!
//! The running order shows the files that have gone as well as the ones that are
//! there, which is the one screen where that is right: it is where somebody comes to
//! find out what is missing. The figures above it count only what can be played, and
//! say how many cannot on a line of their own.

use super::{Fact, Failed, Figure, Frame, Head, Onward, Open, Piece, credited};
use crate::api;
use crate::icon::Icon;
use crate::pages;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{AlbumDetail, AlbumTrack};

#[component]
pub fn Album(id: String) -> impl IntoView {
    let player = crate::player::player();
    let id = StoredValue::new(id);

    let detail = RwSignal::new(None::<AlbumDetail>);
    let failure = RwSignal::new(None::<api::Failure>);

    spawn_local(async move {
        match api::album(&id.get_value()).await {
            Ok(read) => detail.set(Some(read)),
            Err(why) => failure.set(Some(why)),
        }
    });

    // Whether what is sounding came off this record, read from the track playing rather
    // than remembered, so it is still right after the queue has walked on.
    let sounding = move || {
        player
            .now
            .get()
            .and_then(|track| track.album_id)
            .is_some_and(|from| from == id.get_value())
    };

    // A record is a narrowing of its own, so the whole of it goes in the queue however
    // long it is — the same reading the shelf's own play button makes.
    //
    // And it pauses the record it is already playing, which the shelf's button does too.
    // That matters most where the shelf has no button at all: on a touch screen this is
    // the only way to start a record, so it had better also be a way to stop it.
    let play = move |_| {
        if sounding() {
            player.toggle();
            return;
        }

        let mine = id.get_value();

        spawn_local(async move {
            if let Ok(queue) = api::queue(
                api::Narrowing {
                    album: Some(mine),
                    ..Default::default()
                },
                false,
                None,
            )
            .await
            {
                player.play(queue, 0);
            }
        });
    };

    view! {
        <Frame>
            <Head
                icon=Icon::Albums
                picture=Signal::derive(move || Some(api::cover(&id.get_value())))
                heading=Signal::derive(move || {
                    detail
                        .with(|read| read.as_ref().map(|read| read.name.clone()))
                        .unwrap_or_else(|| t!("common.loading").to_string())
                })
                lead=move || view! { <Placing detail /> }
            />

            <div class="leafing">
                {move || failure.get().map(|why| view! { <Failed why /> })}
                {move || detail.get().map(|read| view! { <Said read /> })}
            </div>

            <footer>
                <span class="quiet">{move || detail.with(where_they_are)}</span>

                <span class="deeds">
                    <Show when=move || {
                        detail.with(|read| read.as_ref().is_some_and(|read| read.tracks > 0))
                    }>
                        <button class="leading" on:click=play>
                            {move || {
                                if sounding() && player.playing.get() {
                                    t!("album.pause_record").to_string()
                                } else {
                                    t!("player.play_record").to_string()
                                }
                            }}
                        </button>
                    </Show>
                </span>
            </footer>
        </Frame>
    }
}

/// What the database holds about it.
#[component]
fn Said(read: AlbumDetail) -> impl IntoView {
    let gone = read.missing;
    let players = read.players.clone();
    let listing = read.listing.clone();
    // Read before the markup takes them: a `Show` moves whatever its condition
    // touches, and both of these are wanted afterwards.
    let has_listing = !listing.is_empty();
    let has_players = !players.is_empty();
    // Which disc each row belongs to only matters where there is more than one, and
    // then it matters a lot: a running order that counts to twelve and starts again is
    // unreadable without it.
    let discs = read.discs.unwrap_or(1);

    view! {
        <p class="lettering">{t!("album.the_album")}</p>
        <div class="figures">
            <Figure
                value=Some(pages::thousands(read.tracks))
                name=t!("album.tracks").to_string()
            />
            <Figure value=read.duration.map(pages::runs) name=t!("album.length").to_string() />
            <Figure
                value=read.year.map(|year| year.to_string())
                name=t!("album.year").to_string()
            />
            <Figure
                value=(read.size > 0).then(|| pages::bytes(read.size))
                name=t!("album.on_disc").to_string()
            />
        </div>

        // Said on a line of its own rather than folded into the figures above, which
        // count what can be played. A record missing four of its files is not a record
        // with four fewer tracks.
        <Show when=move || { gone > 0 }>
            <p class="absent">
                {move || {
                    if gone == 1 {
                        t!("album.one_gone").to_string()
                    } else {
                        t!("album.many_gone", count = pages::thousands(gone)).to_string()
                    }
                }}
            </p>
        </Show>

        <dl class="spelt">
            <Fact name=t!("album.artist").to_string() value=read.artist.clone() />
            <Fact name=t!("album.genre").to_string() value=read.genres.clone() />
            <Fact name=t!("album.label").to_string() value=read.label.clone() />
            <Fact
                name=t!("album.discs").to_string()
                value=read.discs.filter(|discs| *discs > 1).map(pages::thousands)
            />
            <Fact name=t!("album.where").to_string() value=read.path.clone() typed=true />
            <Fact
                name=t!("album.library_read").to_string()
                value=Some(match &read.read_at {
                    Some(read_at) => format!("{} · {}", read.library, pages::since(read_at)),
                    None => read.library.clone(),
                })
            />
        </dl>

        <Show when=move || has_listing>
            <p class="lettering">{t!("album.running_order")}</p>

            <div class="running">
                {listing
                    .clone()
                    .into_iter()
                    .scan(None::<i64>, |shown, track| {
                        // The disc heading goes in front of the first track of each,
                        // which is a decision about a row that only the row before it
                        // can answer.
                        let heading = (discs > 1 && *shown != track.disc_number)
                            .then_some(track.disc_number)
                            .flatten();
                        *shown = track.disc_number;
                        Some((heading, track))
                    })
                    .map(|(heading, track)| {
                        view! {
                            {heading
                                .map(|disc| {
                                    view! {
                                        <p class="disc">
                                            {t!("album.disc", nth = pages::thousands(disc))}
                                        </p>
                                    }
                                })}
                            <Row track />
                        }
                    })
                    .collect_view()}
            </div>

            <p class="quiet remark">{t!("album.a_title_opens_it")}</p>
        </Show>

        <Show when=move || has_players>
            <p class="lettering">{t!("album.who_plays")}</p>

            // Every one of them opens. A guest is the name somebody is most likely to
            // want to follow — it is how they find out what else of that person is
            // here — and on a compilation this list is the whole of the record.
            <div class="pills">
                {players
                    .clone()
                    .into_iter()
                    .map(|who| view! { <Onward what=Open::Artist(who.id) name=who.name /> })
                    .collect_view()}
            </div>

            <p class="quiet remark">{t!("album.from_the_tags")}</p>
        </Show>
    }
}

/// One track in the running order: where it sits, what it is called, how long it runs.
#[component]
fn Row(track: AlbumTrack) -> impl IntoView {
    let player = crate::player::player();
    let id = StoredValue::new(track.id.clone());
    let sounding = move || player.current().as_deref() == Some(&id.get_value());

    view! {
        <div
            class:sounding=sounding
            class:gone=track.missing
            on:click=move |_| super::open(super::Open::Track(id.get_value()))
        >
            <span class="figure">
                {track.track_number.map(pages::thousands).unwrap_or_default()}
            </span>
            <span class="what">{track.title}</span>
            <span class="figure">
                {track.duration.map(pages::length).unwrap_or_else(|| pages::MISSING.to_string())}
            </span>
        </div>
    }
}

/// The line under a record's name: who made it, when, and what it is.
///
/// The name it is filed under leads to them, the same as a song's does. A record is
/// where somebody arrives from a shelf, and "what else did they make" is the next
/// question — asking it should not mean going back and typing the name again.
#[component]
fn Placing(detail: RwSignal<Option<AlbumDetail>>) -> impl IntoView {
    view! {
        {move || {
            detail
                .get()
                .map(|read| {
                    // Cut the way a track's credit is cut, and for the same reason: a
                    // record filed under two names is filed under whatever the tag
                    // says about the two of them.
                    let who = read
                        .artist
                        .as_deref()
                        .map(|line| {
                            credited(line, &read.credits)
                                .into_iter()
                                .map(|piece| match piece {
                                    Piece::Words(said) => said.into_any(),
                                    Piece::Name(who) => {
                                        view! { <Onward what=Open::Artist(who.id) name=who.name /> }
                                            .into_any()
                                    }
                                })
                                .collect_view()
                                .into_any()
                        });

                    let year = read.year.map(|year| year.to_string().into_any());
                    let genres = read.genres.map(|genres| genres.into_any());

                    [who, year, genres]
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(nth, part)| view! { {(nth > 0).then_some(" · ")} {part} })
                        .collect_view()
                })
        }}
    }
}

/// How many files it is, said at the foot where a track's panel says where its own
/// answer came from.
///
/// Files rather than tracks, because that is what the sentence is for: a record is
/// this many things in one directory, and the count in the figures above has already
/// said how much music there is.
fn where_they_are(read: &Option<AlbumDetail>) -> String {
    let Some(read) = read else {
        return String::new();
    };

    let held = read.tracks + read.missing;

    if held == 1 {
        t!("album.one_file").to_string()
    } else {
        t!("album.many_files", count = pages::thousands(held)).to_string()
    }
}
