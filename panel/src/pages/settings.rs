// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How the server behaves for everybody.
//!
//! The line between this screen and Preferences is whose answer it is. A theme is
//! yours and takes effect where you are looking; the hour a scan runs is the
//! server's and takes effect for people who are asleep. The two were confusable in
//! the old panel, so the lead says which is which before anything else does.
//!
//! One Save for the whole screen, not one per block. Every value here is a
//! sentence about the same server, somebody who came to change the scan hour will
//! fix the quarantine while they are here, and a screen that made them press Save
//! twice would be a screen that let them leave with half of it stored.

use crate::api::{self, Failure};
use crate::pages::{Setting, said};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Settings, SettingsChanges};

/// The hour a schedule starts at when somebody turns one on, which is the hour a
/// server has least to do.
const SMALL_HOURS: &str = "04:00";

/// How long a quarantine lasts when somebody chooses to have one and has not said
/// how long yet. A week is long enough to survive a disk that failed to mount
/// over a weekend.
const A_WEEK: i64 = 7;

/// What is done with something a scan can no longer find.
///
/// Three answers rather than a number with two magic values, because they are
/// three different intentions and only one of them wants a number: "never", "when
/// it has been gone a while", and "the moment a scan notices".
#[derive(Clone, Copy, PartialEq, Eq)]
enum Absent {
    Kept,
    After,
    AtOnce,
}

impl Absent {
    /// What the server said, read as one of the three.
    fn of(days: Option<i64>) -> Self {
        match days {
            None => Self::Kept,
            Some(0) => Self::AtOnce,
            Some(_) => Self::After,
        }
    }
}

#[component]
pub fn Settings(on_expired: Callback<()>) -> impl IntoView {
    let (settings, set_settings) = signal(Option::<Settings>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (note, set_note) = signal(Option::<String>::None);

    spawn_local(async move {
        match api::settings().await {
            Ok(loaded) => set_settings.set(Some(loaded)),
            Err(Failure::Unauthenticated) => on_expired.run(()),
            Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
        }
    });

    let save = Callback::new(move |changes: SettingsChanges| {
        set_failure.set(None);
        set_note.set(None);

        spawn_local(async move {
            match api::set_settings(changes).await {
                Ok(fresh) => {
                    set_note.set(Some(t!("common.saved").to_string()));
                    set_settings.set(Some(fresh));
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
        });
    });

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.settings")}</h1>
                <p class="quiet lead">{t!("settings.lead")}</p>
            </div>
        </header>

        {move || match settings.get() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(settings) => view! { <Knobs settings save /> }.into_any(),
        }}

        {move || note.get().map(|said| view! { <p class="note">{said}</p> })}
        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}
    }
}

/// Everything on the screen, and the one button that stores it.
///
/// The blocks lay themselves out: a grid of tracks that ask for 420 pixels each,
/// so a wide screen gets two columns and a narrow one gets a single column,
/// without a width written down anywhere.
#[component]
fn Knobs(settings: Settings, save: Callback<SettingsChanges>) -> impl IntoView {
    let (at_startup, set_at_startup) = signal(settings.scan_at_startup);
    // Two signals for one nullable value: whether there is a schedule at all, and
    // what hour it is. Kept apart so that turning the schedule off and on again
    // brings back the hour that was there rather than the default.
    let (scheduled, set_scheduled) = signal(settings.scan_at.is_some());
    let (at, set_at) = signal(settings.scan_at.unwrap_or_else(|| SMALL_HOURS.to_string()));

    let (absent, set_absent) = signal(Absent::of(settings.absent_grace_days));
    // What is written in the box rather than what it parses to, so that an empty
    // box is a state the field can be in. Reading it back as a number and writing
    // the number out again is what made it impossible to clear: the moment the
    // last digit went, a default reappeared under the cursor, and nobody could get
    // from thirty to one. Nothing needs it to hold a number anyway — the pattern
    // and `required` are what stop an empty one being saved.
    let (days, set_days) = signal(match settings.absent_grace_days {
        Some(days) if days > 0 => days.to_string(),
        // No quarantine yet, so the box opens on a suggestion rather than on
        // nothing.
        _ => A_WEEK.to_string(),
    });

    let (articles, set_articles) = signal(settings.ignored_articles.join(" "));
    let (portraits, set_portraits) = signal(settings.fetch_portraits);
    let (session_days, set_session_days) = signal(settings.session_days);

    // The spans offered, and whatever this server actually has if it is not one of
    // them. The API takes any number of days, and a field that quietly dropped the
    // value it was showing would store something nobody chose the next time
    // anybody pressed Save.
    let spans = Signal::derive(move || {
        let mut spans = LIFETIMES.to_vec();
        let current = session_days.get();

        if !spans.contains(&current) {
            spans.push(current);
            spans.sort_unstable();
        }

        spans
    });

    // What the answer means, in a sentence, and it has to follow the answer: a row
    // that said "after a month a scan removes them" while the chosen answer was
    // "keep it" would be describing a server nobody has.
    let explained = Signal::derive(move || match absent.get() {
        Absent::Kept => t!("settings.gone_kept").to_string(),
        Absent::AtOnce => t!("settings.gone_at_once").to_string(),
        Absent::After => t!("settings.gone_after", after = spelled(typed(&days.get()))).to_string(),
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();

        save.run(SettingsChanges {
            ignored_articles: Some(
                articles
                    .get()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
            ),
            scan_at_startup: Some(at_startup.get()),
            scan_at: Some(scheduled.get().then(|| at.get())),
            absent_grace_days: Some(match absent.get() {
                Absent::Kept => None,
                Absent::AtOnce => Some(0),
                Absent::After => Some(typed(&days.get()).unwrap_or(A_WEEK)),
            }),
            session_days: Some(session_days.get()),
            fetch_portraits: Some(portraits.get()),
        });
    };

    view! {
        <form on:submit=submit>
            <div class="knobs">
                <section>
                    <h2 class="part">{t!("settings.scanning")}</h2>

                    <div class="settings">
                        <Setting
                            label=t!("settings.at_startup").to_string()
                            why=t!("settings.at_startup_why").to_string()
                        >
                            <label class="checkbox">
                                <input
                                    type="checkbox"
                                    prop:checked=at_startup
                                    on:change:target=move |e| set_at_startup.set(e.target().checked())
                                />
                                <span>{t!("settings.at_startup_on")}</span>
                            </label>
                        </Setting>

                        // The tick and the hour on one line, because neither says
                        // anything without the other: a schedule with no hour is not
                        // a schedule, and an hour with the tick off is a note about
                        // what would happen.
                        <Setting
                            label=t!("settings.on_a_schedule").to_string()
                            why=t!("settings.on_a_schedule_why").to_string()
                        >
                            <div class="beside">
                                <label class="checkbox">
                                    <input
                                        type="checkbox"
                                        prop:checked=scheduled
                                        on:change:target=move |e| set_scheduled.set(e.target().checked())
                                    />
                                    <span>{t!("settings.every_day")}</span>
                                </label>
                                // A time field rather than a text one: it is the
                                // browser that knows how this reader writes an hour,
                                // and it is the browser that stops them writing an
                                // hour that does not exist.
                                <input
                                    type="time"
                                    required=move || scheduled.get()
                                    disabled=move || !scheduled.get()
                                    prop:value=at
                                    on:input:target=move |e| set_at.set(e.target().value())
                                />
                            </div>
                        </Setting>

                        <Setting label=t!("settings.what_is_gone").to_string() why=explained>
                            <div class="options">
                                <Answer
                                    label=t!("settings.keep_it").to_string()
                                    this=Absent::Kept
                                    chosen=absent
                                    set=set_absent
                                />
                                <Answer
                                    label=t!("settings.after_a_while").to_string()
                                    this=Absent::After
                                    chosen=absent
                                    set=set_absent
                                />
                                <Answer
                                    label=t!("settings.at_once").to_string()
                                    this=Absent::AtOnce
                                    chosen=absent
                                    set=set_absent
                                />
                            </div>

                            // Only under the answer that needs it. The other two
                            // have said everything they have to say.
                            //
                            // The field sits inside the phrase rather than in front
                            // of a unit, and it is a plain box: the little arrows a
                            // number field grows are the loudest thing on a screen
                            // made of hairlines, and nobody steps a quarantine up
                            // one day at a time. What it will only accept is said
                            // in the pattern instead.
                            <Show when=move || absent.get() == Absent::After>
                                <p class="hint after">
                                    <span>{t!("settings.after")}</span>
                                    <input
                                        type="text"
                                        class="count"
                                        inputmode="numeric"
                                        pattern="[1-9][0-9]*"
                                        required
                                        prop:value=days
                                        on:input:target=move |e| set_days.set(e.target().value())
                                    />
                                    <span>{move || counted(typed(&days.get()))}</span>
                                </p>
                            </Show>
                        </Setting>
                    </div>
                </section>

                // One setting each, so they share a column rather than taking one
                // apiece: two blocks of a single row, side by side, would read as a
                // screen with more columns than it has to say.
                <div class="stack">
                    <section>
                        <h2 class="part">{t!("settings.collection")}</h2>

                        <div class="settings">
                            <Setting
                                label=t!("settings.ignored_articles").to_string()
                                why=t!("settings.ignored_articles_why").to_string()
                            >
                                <input
                                    prop:value=articles
                                    placeholder=t!("settings.no_articles")
                                    on:input:target=move |e| set_articles.set(e.target().value())
                                />
                            </Setting>
                        </div>
                    </section>

                    // Its own block rather than a line under the collection,
                    // because it is not a fact about the music: it is the one
                    // switch on this screen that decides whether this server
                    // talks to anybody at all.
                    <section>
                        <h2 class="part">{t!("settings.reaching_out")}</h2>

                        <div class="settings">
                            <Setting
                                label=t!("settings.portraits").to_string()
                                why=t!("settings.portraits_why").to_string()
                            >
                                <label class="checkbox">
                                    <input
                                        type="checkbox"
                                        prop:checked=portraits
                                        on:change:target=move |e| set_portraits.set(e.target().checked())
                                    />
                                    <span>{t!("settings.portraits_on")}</span>
                                </label>
                            </Setting>
                        </div>
                    </section>

                    <section>
                        <h2 class="part">{t!("settings.sessions")}</h2>

                        <div class="settings">
                            <Setting
                                label=t!("settings.session_lifetime").to_string()
                                why=t!("settings.session_lifetime_why").to_string()
                            >
                                <select
                                    class="narrow"
                                    prop:value=move || session_days.get().to_string()
                                    on:change:target=move |e| {
                                        if let Ok(days) = e.target().value().parse() {
                                            set_session_days.set(days);
                                        }
                                    }
                                >
                                    <For each=move || spans.get() key=|days| *days let(days)>
                                        <option value=days.to_string()>{lasting(days)}</option>
                                    </For>
                                </select>
                            </Setting>
                        </div>
                    </section>
                </div>
            </div>

            // One, for the screen. Outside the grid rather than in the last column
            // of it, since it acts on all three blocks.
            <div class="saving">
                <button type="submit" class="pill solid">{t!("common.save")}</button>
            </div>
        </form>
    }
}

/// The spans a session may last. Days throughout, because the field underneath is
/// days and a list that mixed hours into it would be a list of two units.
const LIFETIMES: [i64; 4] = [1, 7, 30, 90];

/// One of a small set of answers, said as a word.
///
/// A word rather than a radio button: three of these are a choice between three
/// states of the same thing, and the panel already says that with the accent
/// wherever it comes up.
#[component]
fn Answer(
    label: String,
    this: Absent,
    chosen: ReadSignal<Absent>,
    set: WriteSignal<Absent>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="option"
            class:chosen=move || chosen.get() == this
            aria-pressed=move || (chosen.get() == this).to_string()
            on:click=move |_| set.set(this)
        >
            {label}
        </button>
    }
}

/// The number in the box, if there is one yet.
///
/// An empty box is a state somebody passes through on the way from thirty to one,
/// not a mistake, so it reads as no number rather than as a zero or a default.
fn typed(written: &str) -> Option<i64> {
    written.trim().parse().ok().filter(|days| *days > 0)
}

/// The word after the field: one day, and days for everything else, including
/// nothing at all.
///
/// A unit rather than a phrase, so it stays beside the box while the sentence
/// underneath does the talking.
fn counted(days: Option<i64>) -> String {
    if days == Some(1) {
        t!("settings.day").to_string()
    } else {
        t!("settings.days_plain").to_string()
    }
}

/// The quarantine written out, for the sentence that says what it means.
///
/// The four spans somebody would say in words get said in words, and any other
/// number is a number of days. Nothing here is a plural rule: rust-i18n has none,
/// so each of these is its own literal key.
///
/// With the box empty the sentence still has to read, so it says the span without
/// naming it rather than naming one nobody typed.
fn spelled(days: Option<i64>) -> String {
    match days {
        None => t!("settings.after_unsaid").to_string(),
        Some(1) => t!("settings.after_a_day").to_string(),
        Some(7) => t!("settings.after_a_week").to_string(),
        Some(30) => t!("settings.after_a_month").to_string(),
        Some(365) => t!("settings.after_a_year").to_string(),
        Some(days) => t!("settings.after_days", count = days).to_string(),
    }
}

/// How long a session lasts, said the way somebody would say it. Only the four
/// offered have a name of their own; anything else arrived through the API and is
/// shown as what it is.
fn lasting(days: i64) -> String {
    match days {
        1 => t!("settings.a_day").to_string(),
        7 => t!("settings.a_week").to_string(),
        30 => t!("settings.a_month").to_string(),
        90 => t!("settings.three_months").to_string(),
        days => t!("settings.days", count = days).to_string(),
    }
}
