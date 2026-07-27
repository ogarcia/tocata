// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Where you land.
//!
//! Four figures large enough to read from across the room, and below them the
//! rest grouped by what it talks about rather than by how it is measured: the
//! collection on one side, the machine on the other.
//!
//! Nothing appears twice. The four at the top are the four at the top, and the
//! boxes below say what those do not.

use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Stats, Status};

#[component]
pub fn Home(
    identity: tocata::types::Identity,
    scan: ReadSignal<Option<Status>>,
    admin: bool,
    on_expired: Callback<()>,
) -> impl IntoView {
    let figures = LocalResource::new(api::stats);
    let name = identity.username.clone();

    view! {
        <h1 class="greeting">{t!("home.greeting", name = name)}</h1>
        <p class="quiet lead">{t!("home.lead")}</p>

        <Suspense fallback=|| view! { <p class="quiet">{t!("common.loading")}</p> }>
            {move || Suspend::new(async move {
                match figures.await {
                    Ok(stats) => view! { <Boxes stats scan admin /> }.into_any(),
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

#[component]
fn Boxes(stats: Stats, scan: ReadSignal<Option<Status>>, admin: bool) -> impl IntoView {
    view! {
        <div class="tiles">
            <Tile
                figure=stats.tracks
                label=t!("home.songs").to_string()
                icon=Icon::Songs
                shade="songs"
            />
            <Tile
                figure=stats.albums
                label=t!("home.albums").to_string()
                icon=Icon::Albums
                shade="albums"
            />
            <Tile
                figure=stats.artists
                label=t!("home.artists").to_string()
                icon=Icon::Artists
                shade="artists"
            />
            <Tile
                figure=stats.genres
                label=t!("home.genres").to_string()
                icon=Icon::Genres
                shade="genres"
            />
        </div>

        <div class="panes">
            <section class="pane">
                <h2>{t!("home.collection")}</h2>
                <dl>
                    <dt>{t!("home.size")}</dt>
                    <dd>{bytes(stats.total_size)}</dd>
                    <dt>{t!("home.duration")}</dt>
                    <dd>{length(stats.total_duration)}</dd>
                    <dt>{t!("home.playlists")}</dt>
                    <dd>{stats.playlists}</dd>
                    <dt>{t!("home.missing")}</dt>
                    <dd>{stats.missing}</dd>
                </dl>
            </section>

            <section class="pane">
                <h2>{t!("home.server")}</h2>
                <dl>
                    <dt>{t!("home.version")}</dt>
                    <dd>{stats.version}</dd>
                    <dt>{t!("home.last_scan")}</dt>
                    <dd>
                        <LastScan scan />
                    </dd>
                    <dt>{t!("home.libraries")}</dt>
                    <dd>{stats.libraries}</dd>
                    <dt>{t!("home.accounts")}</dt>
                    <dd>{stats.users}</dd>
                    <dt>{t!("home.database")}</dt>
                    <dd>{bytes(stats.database_size)}</dd>
                </dl>
            </section>
        </div>

        // Last, so starting a scan does not shove the figures down the page every
        // time, and gone entirely when nothing is running.
        <Running scan admin />
    }
}

/// When the last scan ended, with the rest of what it did behind it.
///
/// A `details` rather than anything of ours: it opens, it closes, it works with a
/// keyboard, and the browser wrote all of that already.
#[component]
fn LastScan(scan: ReadSignal<Option<Status>>) -> impl IntoView {
    view! {
        {move || match scan.get() {
            None => view! { <span class="quiet">{t!("common.loading")}</span> }.into_any(),
            Some(status) if status.scanning => {
                view! { <span class="working-text">{t!("scan.running")}</span> }.into_any()
            }
            Some(status) if status.finished_at.is_none() => {
                view! { <span class="quiet">{t!("scan.never")}</span> }.into_any()
            }
            Some(status) => {
                view! {
                    <details class="last-scan">
                        <summary>{ago(&status)}</summary>
                        <dl class="inner">
                            <dt>{t!("scan.folders")}</dt>
                            <dd>{thousands(status.folders as i64)}</dd>
                            <dt>{t!("scan.unchanged")}</dt>
                            <dd>{thousands(status.unchanged as i64)}</dd>
                            <dt>{t!("scan.failed")}</dt>
                            <dd>{thousands(status.failed as i64)}</dd>
                            <dt>{t!("scan.gone")}</dt>
                            <dd>{thousands(status.gone as i64)}</dd>
                        </dl>
                    </details>
                }
                    .into_any()
            }
        }}
    }
}

/// The scan while it runs: where it has got to, how much it has seen, and the way
/// to stop it.
///
/// Cancelling is here rather than in the header because stopping something should
/// mean having looked at what is being stopped, and this is the only place that
/// shows it.
#[component]
fn Running(scan: ReadSignal<Option<Status>>, admin: bool) -> impl IntoView {
    let (asking, set_asking) = signal(false);

    let cancel = move |_| {
        set_asking.set(true);
        spawn_local(async move {
            let _ = api::cancel_scan().await;
            set_asking.set(false);
        });
    };

    view! {
        <Show when=move || scan.get().is_some_and(|status| status.scanning)>
            <section class="pane running">
                <h2 class="working">
                    <Glyph icon=Icon::Scan />
                    {t!("scan.running")}
                </h2>

                {move || {
                    scan.get()
                        .map(|status| {
                            view! {
                                <p class="counts">
                                    {t!("scan.seen", tracks = thousands(status.tracks as i64),
                                        folders = thousands(status.folders as i64))}
                                </p>
                                // The one field that says something is happening
                                // rather than how much has happened.
                                {status
                                    .path
                                    .or(status.library)
                                    .map(|where_| view! { <p class="path quiet">{where_}</p> })}
                            }
                        })
                }}

                <Show when=move || admin>
                    <p class="row">
                        <button class="second" disabled=asking on:click=cancel>
                            {t!("scan.cancel")}
                        </button>
                    </p>
                </Show>
            </section>
        </Show>
    }
}

/// One figure, its name, and a mark to tell it from the others at a glance.
///
/// The colour is the icon's alone: the figure stays the colour of text, because
/// four differently coloured numbers would be four things competing rather than
/// four things counted.
#[component]
fn Tile(figure: i64, label: String, icon: Icon, shade: &'static str) -> impl IntoView {
    view! {
        <div class="tile">
            <span class=format!("mark {shade}")>
                <Glyph icon />
            </span>
            <span class="figure">{thousands(figure)}</span>
            <span class="quiet">{label}</span>
        </div>
    }
}

/// How long ago it ended, said the way somebody would say it.
///
/// Abbreviated on purpose: "3 min" needs no plural, and rust-i18n has no
/// pluralisation to lean on even if it did.
fn ago(status: &Status) -> String {
    let Some(finished) = status.finished_at.as_deref() else {
        return t!("scan.never").to_string();
    };

    let then = js_sys::Date::parse(finished);
    if then.is_nan() {
        return finished.to_string();
    }

    let seconds = ((js_sys::Date::now() - then) / 1000.0).max(0.0);
    let ago = if seconds < 60.0 {
        t!("home.moments").to_string()
    } else if seconds < 3600.0 {
        t!("home.minutes", count = (seconds / 60.0).floor()).to_string()
    } else if seconds < 86_400.0 {
        t!("home.hours", count = (seconds / 3600.0).floor()).to_string()
    } else {
        t!("home.days", count = (seconds / 86_400.0).floor()).to_string()
    };

    if status.cancelled {
        format!("{ago} · {}", t!("scan.cancelled"))
    } else {
        ago
    }
}

/// Grouped with a space, which every language this speaks agrees on and no
/// language mistakes for a decimal point.
fn thousands(count: i64) -> String {
    let digits = count.abs().to_string();
    let mut out = String::new();

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }

    if count < 0 { format!("-{out}") } else { out }
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
