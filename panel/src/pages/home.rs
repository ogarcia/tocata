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
use rust_i18n::t;
use tocata::types::{Resources, Stats, Status};

#[component]
pub fn Home(
    identity: tocata::types::Identity,
    scan: ReadSignal<Option<Status>>,
    resources: ReadSignal<Option<Resources>>,
    admin: bool,
    on_expired: Callback<()>,
) -> impl IntoView {
    let figures = LocalResource::new(api::stats);
    let name = identity.username.clone();

    view! {
        <header class="titled">
            <div>
                // Your name and not the section's. The section is named in the
                // column on the left, where it is also marked as the one you are
                // on, and a screen whose title repeats its own menu entry spends
                // its largest line saying where you already know you are.
                <h1>{t!("home.greeting", name = name)}</h1>
                <p class="quiet lead">
                    <Lead scan />
                </p>
            </div>

            // Starting one, and only while there is none to start. What a running
            // scan is doing, and the way to stop it, are in the column on the left:
            // one place for it rather than a button here and a panel there.
            <Show when=move || admin && !running(scan)>
                <StartScan />
            </Show>
        </header>

        <Suspense fallback=|| view! { <p class="quiet">{t!("common.loading")}</p> }>
            {move || Suspend::new(async move {
                match figures.await {
                    Ok(stats) => view! { <Figures stats scan resources admin /> }.into_any(),
                    Err(Failure::Unauthenticated) => {
                        on_expired.run(());
                        ().into_any()
                    }
                    Err(_) => view! {
                        <p class="failure" role="alert">{t!("login.unreachable")}</p>
                    }
                        .into_any(),
                }
            })}
        </Suspense>
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
                    <Row label=t!("home.size").to_string() value=bytes(stats.total_size) />
                    <Row label=t!("home.duration").to_string() value=length(stats.total_duration) />
                    <Row label=t!("home.playlists").to_string() value=super::thousands(stats.playlists) />
                    <Row label=t!("home.missing").to_string() value=super::thousands(stats.missing) />
                    <Row label=t!("home.libraries").to_string() value=super::thousands(stats.libraries) />
                </dl>
            </section>

            <section class="pane">
                <h2>{t!("home.server")}</h2>
                <dl class="facts">
                    <Row label=t!("home.version").to_string() value=stats.version />
                    <Row label=t!("home.database").to_string() value=bytes(stats.database_size) />
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
#[component]
fn Row(label: String, value: String) -> impl IntoView {
    view! {
        <div>
            <dt>{label}</dt>
            <dd>{value}</dd>
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
            .map_or_else(|| MISSING.to_string(), |read| percentage(read.cpu))
    });
    let cpu_bar = Signal::derive(move || resources.get().map(|read| read.cpu / 100.0));
    let cores = Signal::derive(move || {
        resources
            .get()
            .map(|read| t!("home.cores", count = read.cores).to_string())
            .unwrap_or_default()
    });

    let memory = Signal::derive(move || {
        resources
            .get()
            .map_or_else(|| MISSING.to_string(), |read| bytes(read.memory))
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
            .map(|total| t!("home.of_total", total = bytes(total)).to_string())
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
                    />
                    <Figure
                        label=t!("scan.gone").to_string()
                        figure=Signal::derive(move || counted(done(), |status| status.gone))
                    />
                </div>
            </section>
        </Show>
    }
}

fn counted(status: Option<Status>, of: fn(&Status) -> u64) -> String {
    super::thousands(status.as_ref().map(of).unwrap_or_default() as i64)
}

/// One figure over its name.
#[component]
fn Figure(label: String, figure: Signal<String>) -> impl IntoView {
    view! {
        <div>
            <span class="figure">{move || figure.get()}</span>
            <span class="quiet">{label}</span>
        </div>
    }
}

/// Starting a scan, from the screen that says what the last one did.
///
/// Two kinds, so it is a menu rather than a button: one of them reads every file
/// again and takes as long as the collection is big, which is not something to set
/// off by aiming badly. The chevron is what says so — a pill with a word on it and
/// nothing else would promise one thing and do two.
#[component]
fn StartScan() -> impl IntoView {
    let (open, set_open) = signal(false);

    let start = move |full: bool| {
        set_open.set(false);
        spawn_local(async move {
            let _ = api::start_scan(full).await;
        });
    };

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

            <Show when=move || open.get()>
                <div class="veil" on:click=move |_| set_open.set(false)></div>
                <div class="menu">
                    // The note is inside the button rather than under it. Beside
                    // it, it looked like something to click that did nothing when
                    // clicked, and left a gap in the middle of the menu that
                    // highlighted neither entry.
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
        </div>
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

/// Stands in for a figure nothing has reported yet. An em dash rather than a zero,
/// which would be a measurement.
const MISSING: &str = "—";

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

/// Powers of two, and the unit spelled the way the standard spells it. Not
/// translated: a unit symbol is a symbol.
fn bytes(count: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut size = count as f64;
    let mut unit = 0;

    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{count} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Hours and minutes. A collection is measured in days of music, not in seconds.
fn length(seconds: i64) -> String {
    format!("{}:{:02}", seconds / 3600, (seconds % 3600) / 60)
}
