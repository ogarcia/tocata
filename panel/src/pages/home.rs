// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Where you land.
//!
//! Four figures across the top, then what the collection is and what the server is,
//! then what the process is costing, then what the last scan did. Everything on the
//! same paper, divided by lines where there is a division to make.
//!
//! The two lists in the middle carry the same number of rows on purpose. They are
//! read side by side, and one of them ending two rows above the other is the first
//! thing the eye finds — which is the complaint this screen was redrawn to answer.
//!
//! Nothing appears twice. The four at the top are the four at the top, and the
//! lists below say what those do not.
//!
//! Everything here is read once except the process, which is the only thing in the
//! panel that moves on its own. It comes down the event stream rather than being
//! asked for on a timer, so the page keeps no clock.

use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use rust_i18n::t;
use tocata::types::{Resources, Stats, Status};

/// The figures, held above the router so a visit to this screen does not pay for them
/// again.
///
/// They were a resource inside the screen, which meant one request per visit: entering,
/// walking off to Albums and coming back asked twice. On a fast machine that is a few
/// milliseconds nobody notices; on the machine this was found on it was eleven seconds
/// each time, because those five figures are aggregates over every track there is.
///
/// Held rather than cached: what is up here is the answer itself, and the one thing that
/// can change it is a scan finishing. Marking a favourite or making a playlist moves two
/// of these figures by one, which is not worth eleven seconds of an Atom — and the screen
/// says when it was read.
#[derive(Clone, Copy)]
pub struct Counted {
    held: RwSignal<Option<Stats>>,
    failed: RwSignal<Option<Failure>>,
}

/// Reads them for the whole panel, and again whenever a scan finishes.
///
/// Not `counted`, which in this module is already the word for turning a scan's figure
/// into a line of text.
///
/// Called where the stream is opened rather than by the screen, so the answer outlives
/// every walk through the sections — and so somebody who is on Artists when a scan ends
/// finds the Overview already right when they get back to it.
pub fn read_the_figures(on_expired: Callback<()>) {
    let counted = Counted {
        held: RwSignal::new(None),
        failed: RwSignal::new(None),
    };
    provide_context(counted);

    let read = move || {
        spawn_local(async move {
            match api::stats().await {
                Ok(stats) => {
                    counted.held.set(Some(stats));
                    counted.failed.set(None);
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                // What was read before stays on screen: a figure from a minute ago is
                // worth more than a screen that empties because one request did not land.
                Err(why) => counted.failed.set(Some(why)),
            }
        });
    };

    read();
    super::endless::after_a_scan(read);
}

#[component]
pub fn Home(
    scan: ReadSignal<Option<Status>>,
    resources: ReadSignal<Option<Resources>>,
    admin: bool,
) -> impl IntoView {
    let counted = use_context::<Counted>().expect("the figures are read above the router");
    // What they asked to be called, and their account's name only if they have not:
    // the greeting is the one line on the panel that addresses somebody. Read from
    // the same place the account menu reads it, so choosing a name shows in both at
    // once and without a reload.
    let name = crate::layout::called_me();

    view! {
        <header class="titled">
            <div>
                // Your name and not the section's. The section is named in the
                // column on the left, where it is also marked as the one you are
                // on, and a screen whose title repeats its own menu entry spends
                // its largest line saying where you already know you are.
                <h1>{move || t!("home.greeting", name = name.get()).to_string()}</h1>
                <p class="quiet lead">
                    <Lead scan />
                </p>
            </div>

            // One button for the whole of it: it starts a scan, it says while one is
            // running, and it is how one is stopped. What the running scan is doing —
            // which folder, how many files — stays in the column on the left, where
            // anything permanently live belongs.
            <Show when=move || admin>
                <StartScan scan />
            </Show>
        </header>

        // What was read, or why it could not be — and the second only where there is no
        // first: a figure from before is worth more than an empty screen.
        {move || match (counted.held.get(), counted.failed.get()) {
            (Some(stats), _) => view! { <Figures stats scan resources admin /> }.into_any(),
            (None, Some(_)) => {
                view! { <p class="failure" role="alert">{t!("login.unreachable")}</p> }.into_any()
            }
            (None, None) => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
        }}
    }
}

fn running(scan: ReadSignal<Option<Status>>) -> bool {
    scan.get().is_some_and(|status| status.scanning)
}

/// What the server is doing, in one line under the greeting.
///
/// The greeting says who is here and this says what is happening, which is the part
/// worth reading again: whether a scan is running changes, and a name does not.
#[component]
fn Lead(scan: ReadSignal<Option<Status>>) -> impl IntoView {
    view! {
        {move || match scan.get() {
            None => t!("common.loading").to_string(),
            Some(status) if status.scanning => t!("home.while_scanning").to_string(),
            Some(status) if status.finished_at.is_none() => t!("home.never_scanned").to_string(),
            Some(status) => t!("home.scanned", when = ago(&status)).to_string(),
        }}
    }
}

#[component]
fn Figures(
    stats: Stats,
    scan: ReadSignal<Option<Status>>,
    resources: ReadSignal<Option<Resources>>,
    admin: bool,
) -> impl IntoView {
    view! {
        <div class="counts">
            <Count figure=stats.tracks label=t!("home.songs").to_string() icon=Icon::Songs />
            <Count figure=stats.albums label=t!("home.albums").to_string() icon=Icon::Albums />
            <Count figure=stats.artists label=t!("home.artists").to_string() icon=Icon::Artists />
            <Count figure=stats.genres label=t!("home.genres").to_string() icon=Icon::Genres />
        </div>

        <div class="panes">
            <section class="pane">
                <h2>{t!("home.collection")}</h2>
                <dl class="facts">
                    <Row label=t!("home.size").to_string() value=super::bytes(stats.total_size) />
                    <Row label=t!("home.duration").to_string() value=length(stats.total_duration) />
                    <Row label=t!("home.playlists").to_string() value=super::thousands(stats.playlists) />
                    // The two problems in one row, and one row because the two panes
                    // carry the same number of them on purpose — see the note at the
                    // head of this file. They are halves of the same idea anyway:
                    // files the last scan could not account for, one sort there and
                    // not in the collection, the other in the collection and not
                    // there. The screen that can afford to tell them apart is the one
                    // this goes to.
                    //
                    // A count that names a problem is a way into it, so above zero it
                    // opens. At zero it is plain like everything around it, because
                    // there is nothing to go and see.
                    <Row
                        label=t!("home.attention").to_string()
                        value=super::thousands(stats.missing + stats.unreadable)
                        to=(stats.missing + stats.unreadable > 0).then_some("/maintenance")
                    />
                    <Row label=t!("home.libraries").to_string() value=super::thousands(stats.libraries) />
                </dl>
            </section>

            <section class="pane">
                <h2>{t!("home.server")}</h2>
                <dl class="facts">
                    <Row label=t!("home.version").to_string() value=stats.version />
                    <Row label=t!("home.database").to_string() value=super::bytes(stats.database_size) />
                    <Row label=t!("home.last_scan").to_string() value=when(scan) />
                    <Row label=t!("home.accounts").to_string() value=super::thousands(stats.users) />
                    <Row label=t!("home.keys").to_string() value=super::thousands(stats.keys) />
                </dl>
            </section>
        </div>

        // Not for a listener. What the server costs the machine is the machine's
        // business, and on somebody else's server it is not theirs.
        <Show when=move || admin>
            <Process resources />
        </Show>

        <LastScan scan />
    }
}

/// One of the four: the figure large and light, and under it its name with a small
/// glyph.
///
/// The glyph goes on the label line, at the size of the label and in its colour.
/// Beside the number it would have to be a coloured disc to balance it, and four
/// coloured discs beside four numbers are four things competing where four things
/// are meant to be counted. Down here it is part of the word.
#[component]
fn Count(figure: i64, label: String, icon: Icon) -> impl IntoView {
    view! {
        <div class="count">
            <span class="figure">{super::thousands(figure)}</span>
            <span class="quiet named-figure">
                <Glyph icon />
                {label}
            </span>
        </div>
    }
}

/// A name and a value, apart from each other on a line of their own.
///
/// The value opens something where it is given somewhere to go, which is only ever
/// where the figure means something is wrong. Drawn as a link and not with an arrow
/// of its own: a link is already what this panel means by "this opens", and one
/// glyph appearing on one row of ten reads as a decoration rather than as an
/// affordance.
#[component]
fn Row(
    label: String,
    value: String,
    #[prop(optional_no_strip)] to: Option<&'static str>,
) -> impl IntoView {
    view! {
        <div>
            <dt>{label}</dt>
            <dd>
                {match to {
                    None => value.into_any(),
                    Some(to) => view! { <A href=to attr:class="link">{value}</A> }.into_any(),
                }}
            </dd>
        </div>
    }
}

/// What the server is costing the machine it runs on, as it costs it.
///
/// The figures arrive on their own every couple of seconds, so this is the one
/// thing here worth looking at twice. Until the first one arrives there is a dash
/// rather than a zero: nothing has been measured yet, and a zero would be a claim.
#[component]
fn Process(resources: ReadSignal<Option<Resources>>) -> impl IntoView {
    // Derived rather than read inside the markup so that a new reading changes the
    // numbers and the length of the bars, and leaves the rest of the block alone.
    let cpu = Signal::derive(move || {
        resources
            .get()
            .map_or_else(|| super::MISSING.to_string(), |read| percentage(read.cpu))
    });
    let cpu_bar = Signal::derive(move || resources.get().map(|read| read.cpu / 100.0));
    let cores = Signal::derive(move || {
        resources
            .get()
            .map(|read| t!("home.cores", count = read.cores).to_string())
            .unwrap_or_default()
    });

    let memory = Signal::derive(move || {
        resources.get().map_or_else(
            || super::MISSING.to_string(),
            |read| super::bytes(read.memory),
        )
    });
    let memory_bar = Signal::derive(move || {
        let read = resources.get()?;
        // No scale, no bar. A machine that will not say how much memory it has
        // leaves a figure worth showing and nothing to show it against.
        let total = read.memory_total.filter(|total| *total > 0)?;

        Some(read.memory as f64 / total as f64)
    });
    let installed = Signal::derive(move || {
        resources
            .get()
            .and_then(|read| read.memory_total)
            .map(|total| t!("home.of_total", total = super::bytes(total)).to_string())
            .unwrap_or_default()
    });

    view! {
        <section class="last-scan">
            <h2 class="part">{t!("home.process")}</h2>
            <div class="gauges">
                <Gauge
                    label=t!("home.processor").to_string()
                    reading=cpu
                    fraction=cpu_bar
                    note=cores
                />
                <Gauge
                    label=t!("home.memory").to_string()
                    reading=memory
                    fraction=memory_bar
                    note=installed
                />
            </div>
        </section>
    }
}

/// One thing being measured: what it is and what it is a share of on the left, the
/// share across the middle, the figure on the right.
///
/// The bar is two elements with a width on the inner one, and not a `meter`. A
/// `meter` is the element that means this and it was the first thing tried, but its
/// track and its fill are vendor pseudo-elements that stop applying the moment
/// `appearance: none` takes the native look away — which leaves a box the colour of
/// the track and no fill in it, so the one thing the row exists to show is the one
/// thing missing. Two spans cannot fail that way, and the width is data rather than
/// styling, which is why it is the one inline style in the panel.
///
/// Hidden from anything that reads the page aloud. The figure beside it is the same
/// number in words, and a bar that announced itself would say everything twice.
///
/// Deliberately no colour change near the top, which is what a `meter` with `low`
/// and `high` would have given for free. A scan is supposed to use the machine it
/// was told to scan with, and a server working hard is not a server in trouble.
#[component]
fn Gauge(
    label: String,
    reading: Signal<String>,
    fraction: Signal<Option<f64>>,
    note: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="gauge">
            <span class="gauge-head">
                <span>{label}</span>
                <Show when=move || !note.get().is_empty()>
                    <span class="note">{move || note.get()}</span>
                </Show>
            </span>

            // The middle column is filled either way, so the figure on the right
            // stays where it is when there is nothing to draw — which is the case
            // when the machine will not say how much memory it has.
            <Show when=move || fraction.get().is_some() fallback=|| view! { <span></span> }>
                <span class="track" aria-hidden="true">
                    <span style=move || {
                        // Clamped here as well as at the server, because a width
                        // over a hundred per cent draws past the end of its track.
                        let share = fraction.get().unwrap_or_default().clamp(0.0, 1.0);
                        format!("width: {:.2}%", share * 100.0)
                    } />
                </span>
            </Show>

            <span class="reading">{move || reading.get()}</span>
        </div>
    }
}

/// What the last scan did, always: four figures on one line.
///
/// This used to be a `details` in the server list, showing when the last scan ended
/// and hiding the four numbers behind a click. A scan having finished is the normal
/// state of a server, and four numbers fit on a line — so there was nothing to
/// save by folding them, and a fold on the normal state is a fold nobody opens.
#[component]
fn LastScan(scan: ReadSignal<Option<Status>>) -> impl IntoView {
    let done = move || scan.get().filter(|status| status.finished_at.is_some());

    view! {
        <Show when=move || done().is_some()>
            <section class="last-scan">
                <h2 class="part">
                    {move || {
                        done()
                            .map(|status| {
                                t!("home.last_scan_when", when = ago(&status)).to_string()
                            })
                            .unwrap_or_default()
                    }}
                </h2>

                <div class="figures">
                    <Figure
                        label=t!("scan.folders").to_string()
                        figure=Signal::derive(move || counted(done(), |status| status.folders))
                    />
                    <Figure
                        label=t!("scan.unchanged").to_string()
                        figure=Signal::derive(move || counted(done(), |status| status.unchanged))
                    />
                    <Figure
                        label=t!("scan.failed").to_string()
                        figure=Signal::derive(move || counted(done(), |status| status.failed))
                        to=into_it(scan, |status| status.failed)
                    />
                    <Figure
                        label=t!("scan.gone").to_string()
                        figure=Signal::derive(move || counted(done(), |status| status.gone))
                        to=into_it(scan, |status| status.gone)
                    />
                </div>
            </section>
        </Show>
    }
}

fn counted(status: Option<Status>, of: fn(&Status) -> u64) -> String {
    super::thousands(status.as_ref().map(of).unwrap_or_default() as i64)
}

/// Where a figure of the last scan goes, while it is worth going there.
///
/// Only the two that mean something is wrong, and only above zero: at zero there is
/// nothing to see, and a link onto an empty screen is a tap somebody does not get
/// back. Read from the status rather than settled when the block is built, because
/// the block is not built again when a scan finishes — the numbers in it change
/// under it, and a link that did not would be a link about the scan before.
fn into_it(
    scan: ReadSignal<Option<Status>>,
    of: fn(&Status) -> u64,
) -> Signal<Option<&'static str>> {
    Signal::derive(move || {
        scan.get()
            .filter(|status| status.finished_at.is_some() && of(status) > 0)
            .map(|_| "/maintenance")
    })
}

/// One figure over its name, and a way in where the figure names a problem.
///
/// The one that opens looks exactly like the three that do not, save for a small
/// arrow after its name. A row of four figures with one of them coloured stops being
/// a row of four figures — what is being read there is how the four compare, and a
/// colour on one of them answers a question nobody was asking.
#[component]
fn Figure(
    label: String,
    figure: Signal<String>,
    #[prop(optional)] to: Option<Signal<Option<&'static str>>>,
) -> impl IntoView {
    let label = StoredValue::new(label);

    move || match to.and_then(|to| to.get()) {
        None => view! {
            <div>
                <span class="figure">{move || figure.get()}</span>
                <span class="quiet">{label.get_value()}</span>
            </div>
        }
        .into_any(),
        Some(to) => view! {
            <A href=to attr:class="amiss">
                <span class="figure">{move || figure.get()}</span>
                <span class="opens">
                    {label.get_value()}
                    <Glyph icon=Icon::Arrow />
                </span>
            </A>
        }
        .into_any(),
    }
}

/// Starting a scan, from the screen that says what the last one did.
///
/// Two kinds, so it is a menu rather than a button: one of them reads every file
/// again and takes as long as the collection is big, which is not something to set
/// off by aiming badly. The chevron is what says so — a pill with a word on it and
/// nothing else would promise one thing and do two.
#[component]
fn StartScan(scan: ReadSignal<Option<Status>>) -> impl IntoView {
    let (open, set_open) = signal(false);

    let start = move |full: bool| {
        set_open.set(false);
        spawn_local(async move {
            let _ = api::start_scan(full).await;
        });
    };

    let cancel = move |_| {
        spawn_local(async move {
            let _ = api::cancel_scan().await;
        });
    };

    view! {
        // While one is running the same button is the scan: the glyph turns, there is
        // no chevron because there is nothing left to choose, and what it says under
        // the pointer is what pressing it would do. Two labels in one grid cell, so
        // the pill is as wide as the longer of them and does not resize under the
        // hand that is aiming at it.
        <Show
            when=move || running(scan)
            fallback=move || {
                view! {
                    <div class="dropdown">
                        <button
                            class="pill"
                            aria-expanded=move || open.get().to_string()
                            on:click=move |_| set_open.update(|shown| *shown = !*shown)
                        >
                            <Glyph icon=Icon::Scan />
                            {t!("scan.start")}
                            <span class="chevron">
                                <Glyph icon=Icon::Chevron />
                            </span>
                        </button>

                        <Menu open set_open start />
                    </div>
                }
            }
        >
            <button class="pill scanning" on:click=cancel>
                <Glyph icon=Icon::Scan />
                <span class="swap">
                    <span class="doing">{t!("scan.running")}</span>
                    // Said to a pointer and to a keyboard alike: focus swaps it too,
                    // or the only way to know what the button does would be to hover,
                    // which is not something a keyboard can do.
                    <span class="undo">{t!("scan.cancel")}</span>
                </span>
            </button>
        </Show>
    }
}

/// What kind of scan, in the menu the pill opens.
#[component]
fn Menu(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    start: impl Fn(bool) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {

        <Show when=move || open.get()>
            <div class="veil" on:click=move |_| set_open.set(false)></div>
            <div class="menu">
                // The note is inside the button rather than under it. Beside it, it
                // looked like something to click that did nothing when clicked, and
                // left a gap in the middle of the menu that highlighted neither entry.
                <button class="menu-item explained" on:click=move |_| start(false)>
                    <span>{t!("scan.quick")}</span>
                    <span class="menu-note">{t!("scan.quick_note")}</span>
                </button>
                <button class="menu-item explained" on:click=move |_| start(true)>
                    <span>{t!("scan.start_full")}</span>
                    <span class="menu-note">{t!("scan.full_note")}</span>
                </button>
            </div>
        </Show>
    }
}

/// When the last scan ended, or what happened to it. Reads on the end of a
/// sentence, which is where every caller puts it.
fn when(scan: ReadSignal<Option<Status>>) -> String {
    match scan.get() {
        None => t!("common.loading").to_string(),
        Some(status) if status.scanning => t!("scan.running").to_string(),
        Some(status) if status.finished_at.is_none() => t!("scan.never").to_string(),
        Some(status) => ago(&status),
    }
}

/// A share, to one decimal. More would be a figure that changes every two seconds
/// in digits nobody is reading, and none at all would sit at zero through
/// everything a lightly loaded server does.
///
/// The space before the sign is a narrow one that does not break, so the number and
/// its unit stay on the same line.
fn percentage(share: f64) -> String {
    format!("{share:.1}\u{202f}%")
}

/// How long ago it ended, said the way somebody would say it.
///
/// Abbreviated on purpose: "3 min" needs no plural, and rust-i18n has no
/// pluralisation to lean on even if it did.
fn ago(status: &Status) -> String {
    let Some(finished) = status.finished_at.as_deref() else {
        return t!("scan.never").to_string();
    };

    let ago = super::since(finished);

    if status.cancelled {
        format!("{ago} · {}", t!("scan.cancelled"))
    } else {
        ago
    }
}

/// Hours and minutes. A collection is measured in days of music, not in seconds.
fn length(seconds: i64) -> String {
    format!("{}:{:02}", seconds / 3600, (seconds % 3600) / 60)
}
