// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Everything about one artist.
//!
//! The one of these three panels with nothing of its own on disk: an artist is not a
//! file and not a directory, only the thing every track that credits them has in
//! common. So there is no path to show, no size, and nothing to read out of anything —
//! and what is worth showing instead is what they amount to here, which is their
//! records and what of theirs actually gets played.
//!
//! "Theirs" is every track that credits them, which is what the listing counts too, so
//! the panel and the row that opened it cannot disagree. It takes in the records they
//! only guest on, and that is the honest reading of what somebody means by opening a
//! name.

use super::{Adding, Fact, Failed, Figure, Frame, Head, Heart};
use crate::api;
use crate::icon::{Glyph, Icon};
use crate::pages;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{ArtistAlbum, ArtistDetail, Attribution, PlayedTrack};

#[component]
pub fn Artist(id: String) -> impl IntoView {
    let player = crate::player::player();
    let id = StoredValue::new(id);

    let detail = RwSignal::new(None::<ArtistDetail>);
    let failure = RwSignal::new(None::<api::Failure>);

    spawn_local(async move {
        match api::artist(&id.get_value()).await {
            Ok(read) => detail.set(Some(read)),
            Err(why) => failure.set(Some(why)),
        }
    });

    // Everything of theirs, shuffled — which is the only ordering that makes sense of
    // a queue drawn from a whole discography. Played in order it would be four hours of
    // their first record before anything else of theirs was reached.
    let play = move |_| {
        let mine = id.get_value();

        spawn_local(async move {
            if let Ok(queue) = api::queue(
                api::Narrowing {
                    artist: Some(mine),
                    ..Default::default()
                },
                true,
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
                icon=Icon::Artists
                round=true
                picture=Signal::derive(move || {
                    // Asked for only where the answer says there is one, unlike a
                    // record's cover: nothing goes looking for a picture of an artist
                    // inside a music file, so the flag here really does mean "there is
                    // one" rather than "nobody has looked".
                    detail.with(|read| {
                        read.as_ref()
                            .filter(|read| read.image)
                            .map(|read| api::portrait(&read.id))
                    })
                })
                heading=Signal::derive(move || {
                    detail
                        .with(|read| read.as_ref().map(|read| read.name.clone()))
                        .unwrap_or_else(|| t!("common.loading").to_string())
                })
                lead=move || detail.with(placing)
            />

            <div class="leafing">
                {move || failure.get().map(|why| view! { <Failed why /> })}
                {move || detail.get().map(|read| view! { <Said read /> })}
            </div>

            <footer>
                // What the picture asks for, where it is somebody else's work,
                // and what this panel is otherwise for where it is not. One line
                // either way: the foot of a drawer says where what is on screen
                // came from, and for a fetched photograph that is a name and a
                // licence rather than a sentence about tags.
                <span class="quiet">
                    {move || {
                        detail
                            .get()
                            .and_then(|read| read.credit)
                            .map(|credit| view! { <Credit credit /> }.into_any())
                            .unwrap_or_else(|| t!("artist.credited_on").into_any())
                    }}
                </span>

                <span class="deeds">
                    <Heart
                        what=api::Marking::Artist
                        id=Signal::derive(move || {
                            detail.with(|read| read.as_ref().map(|read| read.id.clone()))
                        })
                        marked=Signal::derive(move || {
                            detail.with(|read| read.as_ref().and_then(|read| read.starred_at.clone()))
                        })
                    />

                    <Adding
                        what=api::Marking::Artist
                        id=Signal::derive(move || {
                            detail.with(|read| read.as_ref().map(|read| read.id.clone()))
                        })
                    />

                    <Show when=move || {
                        detail.with(|read| read.as_ref().is_some_and(|read| read.tracks > 0))
                    }>
                        <button class="leading" on:click=play>
                            {t!("artist.play_everything")}
                        </button>
                    </Show>
                </span>
            </footer>
        </Frame>
    }
}

/// What the database holds about them.
#[component]
fn Said(read: ArtistDetail) -> impl IntoView {
    let records = read.records.clone();
    let played = read.played_most.clone();
    let has_records = !records.is_empty();
    let has_played = !played.is_empty();

    view! {
        <div class="figures">
            <Figure
                value=Some(pages::thousands(read.albums))
                name=t!("artist.albums").to_string()
            />
            <Figure
                value=Some(pages::thousands(read.tracks))
                name=t!("artist.tracks").to_string()
            />
            <Figure value=read.duration.map(pages::length) name=t!("artist.length").to_string() />
            // Nothing rather than a nought where nobody has played them: a zero here
            // reads as a fact about the artist, and it is a fact about this server.
            <Figure
                value=(read.plays > 0).then(|| pages::thousands(read.plays))
                name=t!("artist.plays").to_string()
            />
        </div>

        <dl class="spelt">
            <Fact name=t!("artist.genre").to_string() value=read.genres.clone() />
        </dl>

        <Show when=move || has_played>
            <p class="lettering">{t!("artist.played_most")}</p>

            <div class="charted">
                {played.clone().into_iter().map(|track| view! { <Heard track /> }).collect_view()}
            </div>

            // Whose plays these are, which is worth saying on a server other people
            // listen to as well.
            <p class="quiet remark">{t!("artist.across_everyone")}</p>
        </Show>

        <Show when=move || has_records>
            <p class="lettering">{t!("artist.records")}</p>

            <div class="discography">
                {records.clone().into_iter().map(|record| view! { <Record record /> }).collect_view()}
            </div>
        </Show>
    }
}

/// Who to credit for the picture above, and under what terms.
///
/// Both of them links out to somebody else's site, which nothing else in this
/// panel does — and here it is the point rather than a leak: an attribution
/// that cannot be followed back to the file it names is not an attribution.
/// They open in a tab of their own, because pressing one is a detour and
/// nobody meant to leave the panel.
#[component]
fn Credit(credit: Attribution) -> impl IntoView {
    let terms = credit.license.clone();

    view! {
        {match credit.author {
            Some(author) => t!("artist.picture_by", author = author).to_string(),
            None => t!("artist.picture_from").to_string(),
        }}
        " "
        <a href=credit.source_url target="_blank" rel="noreferrer">
            {t!("artist.on_commons")}
        </a>
        " · "
        {match credit.license_url {
            Some(url) => {
                view! {
                    <a href=url target="_blank" rel="noreferrer">
                        {terms}
                    </a>
                }
                    .into_any()
            }
            None => terms.into_any(),
        }}
    }
}

/// One of their most played songs: what it is, what record it is off, and how often.
#[component]
fn Heard(track: PlayedTrack) -> impl IntoView {
    let id = StoredValue::new(track.id.clone());

    view! {
        <div on:click=move |_| super::open(super::Open::Track(id.get_value()))>
            <span class="what">
                {track.title}
                <span class="by">{track.album.unwrap_or_else(|| pages::MISSING.to_string())}</span>
            </span>

            <span class="figure">
                {if track.plays == 1 {
                    t!("artist.one_play").to_string()
                } else {
                    t!("artist.many_plays", count = pages::thousands(track.plays)).to_string()
                }}
            </span>

            <span class="figure">
                {track.duration.map(pages::length).unwrap_or_else(|| pages::MISSING.to_string())}
            </span>
        </div>
    }
}

/// One of their records, which opens its own panel.
#[component]
fn Record(record: ArtistAlbum) -> impl IntoView {
    let id = StoredValue::new(record.id.clone());
    let broken = RwSignal::new(false);

    // What it says under its name: when it came out and how much of it there is, joined
    // rather than laid out — a record with no year is not a record whose year is blank.
    let counted = if record.tracks == 1 {
        t!("collection.one_track").to_string()
    } else {
        t!(
            "collection.many_tracks",
            count = pages::thousands(record.tracks)
        )
        .to_string()
    };

    let gone = (record.missing > 0).then(|| {
        if record.missing == 1 {
            t!("artist.one_missing").to_string()
        } else {
            t!(
                "artist.many_missing",
                count = pages::thousands(record.missing)
            )
            .to_string()
        }
    });

    let under = [
        record.year.map(|year| year.to_string()),
        Some(counted),
        gone,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    view! {
        <div on:click=move |_| super::open(super::Open::Album(id.get_value()))>
            // The cover at the size a row has room for, and the same reasoning as the
            // shelf: asked for whatever the answer said, because the flag means one has
            // been found rather than one exists.
            <span class="art">
                <Show
                    when=move || !broken.get()
                    fallback=|| view! { <Glyph icon=Icon::Albums /> }
                >
                    <img
                        src=api::cover(&record.id)
                        alt=""
                        loading="lazy"
                        on:error=move |_| broken.set(true)
                    />
                </Show>
            </span>

            <span class="what">
                {record.name} <span class="by">{under}</span>
            </span>

            <span class="figure">
                {record.duration.map(pages::runs).unwrap_or_else(|| pages::MISSING.to_string())}
            </span>
        </div>
    }
}

/// The line under their name: how much of them there is, and what they are.
///
/// How many tracks in how many records rather than the two figures repeated from the
/// block below, because this line is read first and answers "is there much of them
/// here" in one glance.
fn placing(read: &Option<ArtistDetail>) -> String {
    let Some(read) = read else {
        return String::new();
    };

    let held = if read.tracks == 1 {
        t!(
            "artist.one_track_in",
            albums = pages::thousands(read.albums)
        )
        .to_string()
    } else {
        t!(
            "artist.many_tracks_in",
            count = pages::thousands(read.tracks),
            albums = pages::thousands(read.albums)
        )
        .to_string()
    };

    match &read.genres {
        Some(genres) => format!("{held} · {genres}"),
        None => held,
    }
}
