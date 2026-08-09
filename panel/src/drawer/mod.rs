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
pub mod artist;
pub mod track;

use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use rust_i18n::t;
use tocata::types::Credit;

/// Which panel is over the screen.
#[derive(Clone, PartialEq, Eq)]
pub enum Open {
    /// One track, by identifier.
    Track(String),
    /// One record.
    Album(String),
    /// One artist.
    Artist(String),
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
                    Open::Artist(id) => view! { <artist::Artist id /> }.into_any(),
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
    /// Where the picture of this thing is, once that is known.
    ///
    /// A URL and not an identifier, because the two panels that have one get theirs
    /// from different places: a record's cover and an artist's portrait are different
    /// endpoints, and the alternative was this component knowing which kind of thing it
    /// was heading.
    ///
    /// A signal because it is not known when the panel opens — the identifier arrives
    /// with everything else, a moment later. The glyph is what is there in the meantime,
    /// and what stays where there is no picture to be had.
    #[prop(optional, into)]
    picture: Signal<Option<String>>,
    heading: Signal<String>,
    /// The line under it, which every one of these has and none of them needs: it is
    /// empty until what was asked for arrives.
    ///
    /// A view rather than a string, because on a track it is not only words: the name
    /// of an artist and the name of a record are places to go, and the line is where
    /// somebody who opened a song goes looking for either.
    #[prop(into)]
    lead: ViewFn,
) -> impl IntoView {
    // Whether asking came back with nothing. Asked for rather than checked first, for
    // the same reason a shelf of records asks: the flag a listing carries says a cover
    // has been *found* already, and the finding is what the asking does.
    let missing = RwSignal::new(false);

    view! {
        <header>
            <span class="emblem" class:round=round>
                <Show
                    when=move || picture.get().is_some() && !missing.get()
                    fallback=move || view! { <Glyph icon /> }
                >
                    <img
                        class="art"
                        src=move || picture.get().unwrap_or_default()
                        alt=""
                        on:error=move |_| missing.set(true)
                    />
                </Show>
            </span>

            <div>
                <h2>{heading}</h2>
                <p class="quiet">{move || lead.run()}</p>
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

/// One name in that line that leads somewhere, drawn as the words it stands for.
#[component]
pub fn Onward(what: Open, name: String) -> impl IntoView {
    let what = StoredValue::new(what);

    view! {
        <button class="toward" on:click=move |_| open(what.get_value())>
            {name}
        </button>
    }
}

/// A piece of a credit line.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    /// The tagger's own words: "feat.", an ampersand, the comma between two names.
    Words(String),
    /// A name, and who it is.
    Name(Credit),
}

/// The credit line, cut into the names this server knows and whatever the tagger
/// wrote around them.
///
/// The line stays exactly as it is — this only says which stretches of it are names.
/// A name the line does not spell the way the database does ("Beatles, The" against
/// "The Beatles") is simply not found, and reads as the words it always was: a piece
/// of the sentence that leads nowhere is better than a piece that leads to the wrong
/// place, and better than a line rewritten to make the matching easy.
///
/// Longest name first, so a group whose name contains a member's — "Bob" inside "Bob
/// Dylan" — cannot claim the letters that belong to the longer one. What each name
/// claims is one stretch, and the second time a name appears is left as words: a
/// credit names somebody once, and the repetition is the tagger's.
pub fn credited(line: &str, credits: &[Credit]) -> Vec<Piece> {
    let mut found: Vec<(usize, usize, &Credit)> = Vec::new();

    let mut by_length: Vec<&Credit> = credits.iter().collect();
    by_length.sort_by_key(|who| std::cmp::Reverse(who.name.len()));

    for who in by_length {
        let claimed = line.match_indices(&who.name).find(|(at, name)| {
            let ends = at + name.len();
            !found.iter().any(|(from, to, _)| at < to && &ends > from)
        });

        if let Some((at, name)) = claimed {
            found.push((at, at + name.len(), who));
        }
    }

    found.sort_by_key(|(at, _, _)| *at);

    let mut pieces = Vec::new();
    let mut read_to = 0;

    for (at, ends, who) in found {
        if at > read_to {
            pieces.push(Piece::Words(line[read_to..at].to_string()));
        }
        pieces.push(Piece::Name(who.clone()));
        read_to = ends;
    }

    if read_to < line.len() {
        pieces.push(Piece::Words(line[read_to..].to_string()));
    }

    pieces
}

/// What went wrong, where a panel has nothing else to show.
#[component]
pub fn Failed(why: crate::api::Failure) -> impl IntoView {
    view! { <p class="failure" role="alert">{crate::pages::said(&why)}</p> }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit(id: &str, name: &str) -> Credit {
        Credit {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    /// What is on screen, whatever each piece leads to: putting the line back
    /// together must give the line back.
    fn read_as(pieces: &[Piece]) -> String {
        pieces
            .iter()
            .map(|piece| match piece {
                Piece::Words(said) => said.as_str(),
                Piece::Name(who) => who.name.as_str(),
            })
            .collect()
    }

    /// The whole rule of this in one test: what the tagger wrote survives, and the
    /// names in it are the only thing that leads anywhere.
    #[test]
    fn a_credit_keeps_its_words_and_opens_its_names() {
        let line = "Above & Beyond feat. Zoë Johnston";
        let who = [credit("a1", "Above & Beyond"), credit("a2", "Zoë Johnston")];
        let pieces = credited(line, &who);

        assert_eq!(read_as(&pieces), line, "the tagger's sentence is untouched");
        assert_eq!(
            pieces,
            vec![
                Piece::Name(who[0].clone()),
                Piece::Words(" feat. ".to_string()),
                Piece::Name(who[1].clone()),
            ],
            "the ampersand is part of a name and the 'feat.' is not"
        );
    }

    /// A name the line spells another way is not in the line. Leading nowhere is the
    /// answer; leading to the wrong artist is not.
    #[test]
    fn a_name_the_line_does_not_spell_leads_nowhere() {
        let line = "Beatles, The";
        let pieces = credited(line, &[credit("a1", "The Beatles")]);

        assert_eq!(pieces, vec![Piece::Words(line.to_string())]);
    }

    /// The short name must not claim the letters of the long one, which is what
    /// happens on any collaboration between a band and one of its members.
    #[test]
    fn the_longer_name_claims_its_own_letters() {
        let line = "Bob Dylan & Bob";
        let short = credit("a2", "Bob");
        let long = credit("a1", "Bob Dylan");

        // Given in the order that gets it wrong if length is not what decides.
        let pieces = credited(line, &[short.clone(), long.clone()]);

        assert_eq!(
            pieces,
            vec![
                Piece::Name(long),
                Piece::Words(" & ".to_string()),
                Piece::Name(short),
            ]
        );
    }

    /// Three names and two of the tagger's words between them, which is what a
    /// collaboration on a real shelf looks like.
    #[test]
    fn every_name_in_a_long_credit_is_its_own_way_out() {
        let line = "Alejandro Sanz con Juan Habichuela y Ketama";
        let who = [
            credit("a1", "Alejandro Sanz"),
            credit("a2", "Juan Habichuela"),
            credit("a3", "Ketama"),
        ];

        assert_eq!(
            credited(line, &who),
            vec![
                Piece::Name(who[0].clone()),
                Piece::Words(" con ".to_string()),
                Piece::Name(who[1].clone()),
                Piece::Words(" y ".to_string()),
                Piece::Name(who[2].clone()),
            ]
        );
    }

    /// A tagger who wrote a name twice wrote it twice. One of them opens, and the
    /// other stays what it is: two ways to the same panel, a foot apart, would read
    /// as two different people.
    #[test]
    fn a_name_written_twice_is_opened_once() {
        let line = "Sarah Brightman with Andrzej Lampert / Sarah Brightman";
        let sarah = credit("a1", "Sarah Brightman");
        let guest = credit("a2", "Andrzej Lampert");
        let pieces = credited(line, &[sarah.clone(), guest.clone()]);

        assert_eq!(read_as(&pieces), line);
        assert_eq!(
            pieces,
            vec![
                Piece::Name(sarah),
                Piece::Words(" with ".to_string()),
                Piece::Name(guest),
                Piece::Words(" / Sarah Brightman".to_string()),
            ]
        );
    }

    /// The ordinary file: one name, and the whole line is it.
    #[test]
    fn one_name_is_the_whole_line() {
        let who = credit("a1", "Alice");
        assert_eq!(
            credited("Alice", std::slice::from_ref(&who)),
            vec![Piece::Name(who)]
        );
    }
}
