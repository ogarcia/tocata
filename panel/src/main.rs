// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Tocata's administration panel.
//!
//! A client of `/api/v1` and nothing else. It holds no state the server does not
//! already hold, which is what keeps a reload from being a way to lose anything.
//!
//! The shapes it exchanges come from the server's own crate, with everything that
//! needs a database or a socket switched off by a feature. Rename a field there
//! and this stops compiling, which is the whole reason the panel is in Rust.

mod api;
mod events;
mod icon;
mod layout;
mod locale;
mod login;
mod pages;

// Compiles the translations in. `fallback` is what a key missing from a
// translation falls back to, so a half translated language shows English rather
// than the name of the key.
rust_i18n::i18n!("locales", fallback = "en");

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;
use rust_i18n::t;
use tocata::types::Identity;

/// Who is logged in, or nobody, or not asked yet.
///
/// The third state matters: without it the form flashes up for a moment on every
/// reload before the answer to "who am I" arrives, which looks like being logged
/// out and is not.
#[derive(Clone, PartialEq, Eq)]
enum Who {
    Asking,
    Nobody,
    Somebody(Identity),
}

#[component]
fn Panel() -> impl IntoView {
    let (who, set_who) = signal(Who::Asking);

    // One question on load. A live cookie lands straight on the panel; anything
    // else puts the form up.
    spawn_local(async move {
        set_who.set(match api::whoami().await {
            Ok(identity) => Who::Somebody(identity),
            Err(_) => Who::Nobody,
        });
    });

    let forget = Callback::new(move |()| set_who.set(Who::Nobody));

    view! {
        {move || match who.get() {
            Who::Asking => {
                view! {
                    <main class="entry">
                        <p class="quiet">{t!("common.loading")}</p>
                    </main>
                }
                    .into_any()
            }
            Who::Nobody => {
                view! {
                    <login::LogIn on_in=Callback::new(move |identity| {
                        set_who.set(Who::Somebody(identity))
                    }) />
                }
                    .into_any()
            }
            Who::Somebody(identity) => view! { <Inside identity forget /> }.into_any(),
        }}
    }
}

/// The panel proper, once there is somebody to show it to.
#[component]
fn Inside(identity: Identity, forget: Callback<()>) -> impl IntoView {
    let admin = identity.admin;

    // One stream for the whole panel, opened here and read in two places: the
    // header, which says a scan is running, and the screen that shows it in
    // full. Two connections would be two of everything for the same news.
    let scan = events::scan_status();

    view! {
        <Router>
            <layout::Shell identity on_out=forget scan>
                <Routes fallback=move || {
                    view! { <pages::Unbuilt heading=t!("nav.overview").to_string() /> }
                }>
                    <Route
                        path=path!("/")
                        view=move || view! { <pages::overview::Overview on_expired=forget /> }
                    />
                    <Route
                        path=path!("/scan")
                        view=move || view! { <pages::scan::Scan status=scan admin /> }
                    />
                    <Route
                        path=path!("/account")
                        view=move || {
                            view! { <pages::Unbuilt heading=t!("nav.account").to_string() /> }
                        }
                    />

                    // The administration sections. The menu does not offer these
                    // to anybody else and the server refuses them regardless, so
                    // what is left to handle here is a URL typed by hand.
                    <Route
                        path=path!("/libraries")
                        view=move || {
                            view! { <Restricted admin heading=t!("nav.libraries").to_string() /> }
                        }
                    />
                    <Route
                        path=path!("/accounts")
                        view=move || {
                            view! { <Restricted admin heading=t!("nav.accounts").to_string() /> }
                        }
                    />
                    <Route
                        path=path!("/settings")
                        view=move || {
                            view! { <Restricted admin heading=t!("nav.settings").to_string() /> }
                        }
                    />
                    <Route
                        path=path!("/maintenance")
                        view=move || {
                            view! { <Restricted admin heading=t!("nav.maintenance").to_string() /> }
                        }
                    />
                </Routes>
            </layout::Shell>
        </Router>
    }
}

/// Keeps the rights check in one place rather than repeated at every route.
#[component]
fn Restricted(admin: bool, heading: String) -> impl IntoView {
    if admin {
        view! { <pages::Unbuilt heading /> }.into_any()
    } else {
        view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
    }
}

fn main() {
    console_error_panic_hook::set_once();
    locale::settle();
    leptos::mount::mount_to_body(Panel);
}
