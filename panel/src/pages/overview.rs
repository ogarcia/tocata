// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What there is, in figures.
//!
//! The one screen with something real in it, which is what makes it the one
//! worth having in a skeleton: it proves the cookie travels, the JSON parses and
//! the shared types line up.

use crate::api::{self, Failure};
use leptos::prelude::*;
use rust_i18n::t;
use tocata::types::Stats;

#[component]
pub fn Overview(on_expired: Callback<()>) -> impl IntoView {
    let figures = LocalResource::new(api::stats);

    view! {
        <h1>{t!("overview.heading")}</h1>

        <Suspense fallback=|| view! { <p class="quiet">{t!("common.loading")}</p> }>
            {move || Suspend::new(async move {
                match figures.await {
                    Ok(stats) => view! { <Figures stats /> }.into_any(),
                    // A session that ran out while this screen was open is not
                    // this screen's problem to report: the whole panel goes back
                    // to the form.
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
fn Figures(stats: Stats) -> impl IntoView {
    view! {
        <table class="figures">
            <tbody>
                <tr><th>{t!("overview.version")}</th><td>{stats.version}</td></tr>
                <tr><th>{t!("overview.artists")}</th><td>{stats.artists}</td></tr>
                <tr><th>{t!("overview.albums")}</th><td>{stats.albums}</td></tr>
                <tr><th>{t!("overview.tracks")}</th><td>{stats.tracks}</td></tr>
                <tr><th>{t!("overview.missing")}</th><td>{stats.missing}</td></tr>
                <tr><th>{t!("overview.genres")}</th><td>{stats.genres}</td></tr>
                <tr><th>{t!("overview.playlists")}</th><td>{stats.playlists}</td></tr>
                <tr><th>{t!("overview.accounts")}</th><td>{stats.users}</td></tr>
                <tr><th>{t!("overview.libraries")}</th><td>{stats.libraries}</td></tr>
                <tr><th>{t!("overview.size")}</th><td>{bytes(stats.total_size)}</td></tr>
                <tr><th>{t!("overview.duration")}</th><td>{hours(stats.total_duration)}</td></tr>
                <tr><th>{t!("overview.database")}</th><td>{bytes(stats.database_size)}</td></tr>
            </tbody>
        </table>
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
        format!("{count} {}", UNITS[0])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Hours and minutes. A collection is measured in days of music, not in seconds.
fn hours(seconds: i64) -> String {
    format!("{}:{:02}", seconds / 3600, (seconds % 3600) / 60)
}
