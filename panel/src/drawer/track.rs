// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Everything about one song.
//!
//! Three tabs, because the three answers come from three places and only the first is
//! free. **What it says** is the database — what the last scan read and kept, drawn
//! the moment the panel opens. **Lyrics** and **Every tag** are the file itself, read
//! on the server when asked, which is why neither is fetched until the first answer
//! says there is a file to read.
//!
//! Splitting them is not tidiness. Lyrics are long enough to bury a list of fields in
//! one scroll, and a tag list is a hundred rows of names nobody reads unless they came
//! looking for one.
//!
//! Every row obeys the same rule: a field the file never filled in has no row. So the
//! length of what is on screen is itself the answer to how well tagged a song is, and
//! there is no wall of dashes to read past to find the two things that are there.

use super::{Fact, Failed, Figure, Frame, Head};
use crate::api;
use crate::icon::Icon;
use crate::pages;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{LyricSource, Lyrics, Tags, TrackDetail};

/// Which of the three is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// The database's answer, which is the one that is always there.
    Said,
    Words,
    Every,
}

#[component]
pub fn Track(id: String) -> impl IntoView {
    let player = crate::player::player();
    let id = StoredValue::new(id);

    let detail = RwSignal::new(None::<TrackDetail>);
    let tags = RwSignal::new(None::<Tags>);
    let words = RwSignal::new(None::<Lyrics>);
    let failure = RwSignal::new(None::<api::Failure>);
    let tab = RwSignal::new(Tab::Said);

    // The database first, and the file only once that answer says there is one. Two
    // hops rather than three requests at once: a track whose file has gone would
    // otherwise be asked twice for something that cannot be read, and it is exactly
    // the track somebody opens this panel to look at.
    spawn_local(async move {
        match api::detail(&id.get_value()).await {
            Err(why) => failure.set(Some(why)),
            Ok(read) => {
                let gone = read.missing;
                detail.set(Some(read));

                if gone {
                    return;
                }

                // Both at once, and neither's failure is worth a message: what a
                // failure means here is that a tab does not appear, which is the same
                // thing it means when the file simply had nothing in it.
                let which = id.get_value();
                if let Ok(read) = api::tags(&which).await {
                    tags.set(Some(read));
                }
                if let Ok(read) = api::lyrics(&which).await {
                    words.set(Some(read));
                }
            }
        }
    });

    // A file with no tag in it is a file with nothing to list, so there is no tab —
    // and the count goes on the tab, because how many tags a file carries is worth
    // knowing before deciding to look.
    let counted = move || tags.with(|read| read.as_ref().map(|read| read.tags.len()).unwrap_or(0));
    let has_tags = move || counted() > 0;

    // The words get a tab whenever the file could be read at all, including when it
    // holds none: "there are none, and here is where they would go" is the useful
    // answer, and it is only sayable on a tab.
    let has_words = move || words.with(Option::is_some);

    view! {
        <Frame>
            <Head
                icon=Icon::Songs
                heading=Signal::derive(move || {
                    detail.with(|read| read.as_ref().map(|read| read.title.clone()))
                        .unwrap_or_else(|| t!("common.loading").to_string())
                })
                lead=Signal::derive(move || detail.with(placing))
            />

            // Only where there is somewhere else to go. One tab is not a choice, and
            // a strip of one reads as two that failed to load.
            <Show when=move || has_words() || has_tags()>
                <div class="tabs">
                    <button
                        class:chosen=move || tab.get() == Tab::Said
                        on:click=move |_| tab.set(Tab::Said)
                    >
                        {t!("track.said")}
                    </button>

                    <Show when=has_words>
                        <button
                            class:chosen=move || tab.get() == Tab::Words
                            on:click=move |_| tab.set(Tab::Words)
                        >
                            {t!("track.words")}
                        </button>
                    </Show>

                    <Show when=has_tags>
                        <button
                            class:chosen=move || tab.get() == Tab::Every
                            on:click=move |_| tab.set(Tab::Every)
                        >
                            {move || t!("track.every_tag", count = counted())}
                        </button>
                    </Show>
                </div>
            </Show>

            <div class="reading">
                {move || failure.get().map(|why| view! { <Failed why /> })}

                <Show when=move || tab.get() == Tab::Said>
                    {move || detail.get().map(|read| view! { <Said read /> })}
                </Show>

                <Show when=move || tab.get() == Tab::Words>
                    {move || words.get().map(|read| view! { <Words read mine=id.get_value() /> })}
                </Show>

                <Show when=move || tab.get() == Tab::Every>
                    {move || tags.get().map(|read| view! { <Every read /> })}
                </Show>
            </div>

            <footer>
                // What is on screen and where it came from, which changes with the
                // tab because the answer does: one of these is the database and two
                // of them are the file.
                <span class="quiet">
                    {move || match tab.get() {
                        Tab::Said => t!("track.from_the_scan").to_string(),
                        Tab::Words => whence(&words.get()),
                        Tab::Every => t!("track.from_the_file").to_string(),
                    }}
                </span>

                // No way to play a file that is not there, and so nothing here at all
                // on one. The panel is still worth opening on it — it is where you
                // find out what was lost — and offering to play it would be offering
                // nothing.
                //
                // No way to copy the path either, and there never was one worth a
                // button: the path is on screen, and selecting text is something every
                // browser already does better than a button that says it will.
                <span class="deeds">
                    <Show when=move || {
                        detail.with(|read| read.as_ref().is_some_and(|read| !read.missing))
                    }>
                        <button
                            class="leading"
                            on:click=move |_| player.play(vec![id.get_value()], 0)
                        >
                            {t!("player.play_this")}
                        </button>
                    </Show>
                </span>
            </footer>
        </Frame>
    }
}

/// What the database kept.
#[component]
fn Said(read: TrackDetail) -> impl IntoView {
    let has_ids = read.isrc.is_some() || read.mbid_recording.is_some();
    let comment = read.comment.clone();

    view! {
        // Not designed, and the panel would be dishonest without it: this is the one
        // row in a listing somebody opens *because* something is wrong with it.
        <Show when=move || read.missing>
            <p class="absent">{t!("track.file_gone")}</p>
        </Show>

        <p class="lettering">{t!("track.the_recording")}</p>
        <dl class="spelt">
            <Fact name=t!("track.title").to_string() value=Some(read.title.clone()) />
            <Fact name=t!("track.artist").to_string() value=read.artists.clone() />
            <Fact name=t!("track.album").to_string() value=read.album.clone() />
            <Fact name=t!("track.album_artist").to_string() value=read.album_artist.clone() />
            <Fact name=t!("track.track").to_string() value=nth_track(&read) />
            <Fact name=t!("track.disc").to_string() value=nth_disc(&read) />
            <Fact
                name=t!("track.year").to_string()
                value=read.year.map(|year| year.to_string())
            />
            <Fact name=t!("track.genre").to_string() value=read.genres.clone() />
        </dl>

        <p class="lettering">{t!("track.the_file")}</p>
        <div class="figures">
            <Figure
                value=read.duration.map(pages::length)
                name=t!("track.length").to_string()
            />
            <Figure
                value=Some(read.suffix.to_uppercase())
                name=t!("track.format").to_string()
            />
            <Figure
                value=read.bit_rate.map(|rate| t!("player.kbps", rate = rate).to_string())
                name=t!("track.bitrate").to_string()
            />
            <Figure value=sampled(&read) name=t!("track.khz_bits").to_string() />
        </div>

        <dl class="spelt">
            // Relative to the library, which is what the scanner stores and all this
            // needs to say: enough to find one file among the others, without telling
            // everybody who may see a library where it is mounted.
            <Fact name=t!("track.where").to_string() value=Some(read.path.clone()) typed=true />
            <Fact name=t!("track.library").to_string() value=Some(read.library.clone()) />
            <Fact
                name=t!("track.size_read").to_string()
                value=Some(format!("{} · {}", pages::bytes(read.size), pages::since(&read.read_at)))
            />
        </dl>

        <Show when=move || has_ids>
            <p class="lettering">{t!("track.identifiers")}</p>
            <dl class="spelt">
                <Fact name=t!("track.isrc").to_string() value=read.isrc.clone() typed=true />
                <Fact
                    name=t!("track.musicbrainz").to_string()
                    value=read.mbid_recording.clone()
                    typed=true
                />
            </dl>
        </Show>

        {comment
            .map(|said| {
                view! {
                    <p class="lettering">{t!("track.comment")}</p>
                    <p class="quoted">{said}</p>
                }
            })}
    }
}

/// The words, timed or not, or the news that there are none.
#[component]
fn Words(read: Lyrics, mine: String) -> impl IntoView {
    let player = crate::player::player();
    let mine = StoredValue::new(mine);

    // Only the song that is actually sounding gets a line lit. Reading the words of
    // one track while another plays is an ordinary thing to do, and a line following
    // somebody else's playhead would be a lie about which song this is.
    let sounding = move || player.current().as_deref() == Some(&mine.get_value());

    let timings: Vec<i64> = read.lines.iter().filter_map(|line| line.at).collect();

    // The last line whose moment has passed. Nothing before the first one, which is
    // where a song's opening bars are.
    //
    // A signal rather than a closure because every line asks it, and a closure would
    // have to be moved into the first of them.
    let at_the_playhead = Signal::derive(move || {
        if !sounding() {
            return None;
        }

        let elapsed = (player.elapsed.get() * 1000.0) as i64;
        timings.iter().rposition(|at| *at <= elapsed)
    });

    let words = read.lines.clone();
    let plain = StoredValue::new(
        read.lines
            .iter()
            .map(|line| line.value.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let counted = read.lines.len();
    let synced = read.synced;
    let source = read.source.clone();
    let beside = read.beside.clone();

    view! {
        {match source {
            None => {
                view! {
                    <div class="wordless">
                        <p>{t!("track.no_words")}</p>
                        <p class="quiet">{t!("track.where_it_looked")}</p>
                    </div>

                    <dl class="spelt">
                        // The two extensions it would read, so the name is exact
                        // rather than described.
                        <Fact
                            name=t!("track.expected").to_string()
                            value=Some(format!("{beside}.lrc · {beside}.txt"))
                            typed=true
                        />
                    </dl>

                    // No date for when it last looked, because nothing ever looks
                    // ahead of being asked: there is no scan to wait for and nothing
                    // to reload. Put the words there and they are there.
                    <p class="quiet remark">{t!("track.put_them_there")}</p>
                }
                    .into_any()
            }
            Some(source) => {
                view! {
                    <div class="whence">
                        <p class="quiet">{told(&source, synced, counted)}</p>
                        <button class="plain" on:click=move |_| write_out(&plain.get_value())>
                            {t!("track.copy")}
                        </button>
                    </div>

                    {if synced {
                        view! {
                            <div class="timed">
                                {words
                                    .into_iter()
                                    .enumerate()
                                    .map(|(nth, line)| {
                                        // An empty line with a time on it is a
                                        // passage of the song with no words in it,
                                        // which is worth saying: a blank row reads as
                                        // the end of the words rather than as a break
                                        // in them.
                                        let quiet = line.value.trim().is_empty();
                                        let said = if quiet {
                                            t!("track.instrumental").to_string()
                                        } else {
                                            line.value
                                        };

                                        view! {
                                            <div class:sounding=move || {
                                                at_the_playhead.get() == Some(nth)
                                            }>
                                                <span class="figure">
                                                    {line.at.map(at_minute).unwrap_or_default()}
                                                </span>
                                                <span class:quiet=quiet>{said}</span>
                                            </div>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        }
                            .into_any()
                    } else {
                        // Deliberately not the timed layout with the times left out:
                        // an empty column reads as a value that is missing rather
                        // than as one that was never there.
                        view! { <p class="verses">{plain.get_value()}</p> }.into_any()
                    }}
                }
                    .into_any()
            }
        }}
    }
}

/// Every tag the file carries, under the names its own format writes.
#[component]
fn Every(read: Tags) -> impl IntoView {
    let kind = read.kind.clone().unwrap_or_default();

    view! {
        <p class="quiet remark">{t!("track.as_written", kind = kind)}</p>

        <dl class="spelt frames">
            {read
                .tags
                .into_iter()
                .map(|tag| {
                    view! {
                        <div>
                            <dt>{tag.name}</dt>
                            <dd>{tag.value}</dd>
                        </div>
                    }
                })
                .collect_view()}

            // Last, and quiet: the one row whose value is a description of what is in
            // the file rather than what the file says.
            {read
                .picture
                .map(|picture| {
                    view! {
                        <div>
                            <dt>{picture.name}</dt>
                            <dd class="quiet">{picture.value}</dd>
                        </div>
                    }
                })}
        </dl>
    }
}

/// The line under a track's name: who made it, what record it is off, and when.
///
/// Joined rather than laid out, so a song with no year is not a song whose year is
/// blank. Empty until the answer arrives, which is what the heading's own "loading"
/// is already saying.
fn placing(read: &Option<TrackDetail>) -> String {
    let Some(read) = read else {
        return String::new();
    };

    [
        read.artists.clone(),
        read.album.clone(),
        read.year.map(|year| year.to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// Where it sits on its record.
fn nth_track(read: &TrackDetail) -> Option<String> {
    read.track_number
        .map(|number| out_of(number, read.album_tracks))
}

/// Which disc of the set, and only where there is a set.
///
/// A record that came on one disc says nothing here: "1 of 1" is not a fact about a
/// song, it is a blank with numbers in it, and a panel whose rule is that nothing
/// empty gets a row should not make an exception for something empty that happens to
/// be countable. Which is most records most people own.
///
/// A number above one is enough on its own, even where nothing says how many there
/// were: "disc 2" of an unknown set is still a thing worth knowing. A total above one
/// with no number against it is not — there is no disc to name.
fn nth_disc(read: &TrackDetail) -> Option<String> {
    let number = read.disc_number?;
    let held = read.album_discs;

    (number > 1 || held.is_some_and(|held| held > 1)).then(|| out_of(number, held))
}

/// "2 of 10", or bare where there is nothing to be out of.
///
/// A total smaller than the number is not a total of anything this belongs to — five
/// of four is a disagreement between a tag and a directory, and printing it would be
/// passing that disagreement off as a fact. The number is the half that came off this
/// file, so the number is what is left.
fn out_of(number: i64, held: Option<i64>) -> String {
    match held {
        Some(held) if held >= number => t!("track.nth_of", nth = number, held = held).to_string(),
        _ => number.to_string(),
    }
}

/// How well it was recorded, as one figure: "44.1 / 16".
///
/// One figure and not two, because neither means much without the other and a panel
/// four columns wide has better uses for the second. Absent altogether where the file
/// reported neither.
fn sampled(read: &TrackDetail) -> Option<String> {
    let rate = read
        .sampling_rate
        .map(|hertz| format!("{:.1}", hertz as f64 / 1000.0));

    match (rate, read.bit_depth) {
        (Some(rate), Some(bits)) => Some(format!("{rate} / {bits}")),
        (Some(rate), None) => Some(rate),
        (None, Some(bits)) => Some(bits.to_string()),
        (None, None) => None,
    }
}

/// Where the words came from, whether they are timed, and how many there are.
fn told(source: &LyricSource, synced: bool, lines: usize) -> String {
    let whence = match source {
        LyricSource::Beside(name) => t!("track.beside_the_file", name = name).to_string(),
        LyricSource::Frame(frame) => t!("track.in_the_frame", frame = frame).to_string(),
    };

    let timed = if synced {
        t!("track.timed").to_string()
    } else {
        t!("track.untimed").to_string()
    };

    let counted = if lines == 1 {
        t!("track.one_line").to_string()
    } else {
        t!("track.many_lines", count = lines).to_string()
    };

    format!("{whence} · {timed} · {counted}")
}

/// The footnote under the words, which says where they were read from — the one thing
/// this tab is for in an administration panel.
fn whence(read: &Option<Lyrics>) -> String {
    match read.as_ref().and_then(|read| read.source.as_ref()) {
        Some(LyricSource::Beside(_)) => t!("track.from_beside").to_string(),
        Some(LyricSource::Frame(_)) => t!("track.from_the_file").to_string(),
        None => t!("track.looked_in_both").to_string(),
    }
}

/// A moment in a song, as a song's lengths are written everywhere else here.
fn at_minute(millis: i64) -> String {
    pages::length(millis / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track that says nothing about itself, for a test to fill in the one or two
    /// fields it is about.
    fn nothing_much() -> TrackDetail {
        TrackDetail {
            id: "t1".to_string(),
            title: "Song".to_string(),
            artists: None,
            album: None,
            album_id: None,
            album_artist: None,
            genres: None,
            track_number: None,
            album_tracks: None,
            disc_number: None,
            album_discs: None,
            year: None,
            duration: None,
            suffix: "flac".to_string(),
            bit_rate: None,
            sampling_rate: None,
            bit_depth: None,
            path: "song.flac".to_string(),
            library: "kept".to_string(),
            size: 1,
            read_at: "2026-01-01T00:00:00Z".to_string(),
            isrc: None,
            mbid_recording: None,
            comment: None,
            missing: false,
        }
    }

    /// Asserted on the numbers rather than on the wording, because the locale is
    /// global and another test in this binary is setting it.
    fn names_both(said: &str, nth: i64, held: i64) -> bool {
        said.contains(&nth.to_string()) && said.contains(&held.to_string())
    }

    #[test]
    fn a_track_is_out_of_however_many_the_record_holds() {
        let mut read = nothing_much();
        assert_eq!(nth_track(&read), None, "no number, no row");

        read.track_number = Some(2);
        assert_eq!(
            nth_track(&read).as_deref(),
            Some("2"),
            "bare, where nothing says how many there are"
        );

        read.album_tracks = Some(10);
        assert!(names_both(&nth_track(&read).unwrap(), 2, 10));

        // A total smaller than the number is two sources disagreeing — a tag against a
        // directory — and the number is the half that came off this file.
        read.album_tracks = Some(1);
        assert_eq!(nth_track(&read).as_deref(), Some("2"));
    }

    /// The rule that made this a row of its own: most records came on one disc, and for
    /// those there is nothing here to say.
    #[test]
    fn a_disc_is_named_only_where_there_is_a_set() {
        let mut read = nothing_much();

        read.disc_number = Some(1);
        read.album_discs = Some(1);
        assert_eq!(
            nth_disc(&read),
            None,
            "one of one is a blank with numbers in it"
        );

        read.album_discs = Some(2);
        assert!(
            names_both(&nth_disc(&read).unwrap(), 1, 2),
            "the first of two is worth saying"
        );

        // A number above one stands on its own: disc two of an unknown set is still
        // something to know.
        read.disc_number = Some(2);
        read.album_discs = None;
        assert_eq!(nth_disc(&read).as_deref(), Some("2"));

        // A set of two with nothing saying which disc this is names no disc.
        read.disc_number = None;
        read.album_discs = Some(2);
        assert_eq!(nth_disc(&read), None);
    }
}

/// Straight to the clipboard, and nothing said either way.
///
/// The clipboard needs a permission the browser may refuse, and there is nothing
/// useful to do about that here: what would have been copied is on screen to be
/// selected by hand.
fn write_out(text: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().clipboard().write_text(text);
    }
}
