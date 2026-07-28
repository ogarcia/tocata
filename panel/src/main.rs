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

mod accent;
mod api;
mod events;
mod icon;
mod layout;
mod locale;
mod login;
mod pages;
mod theme;

// Compiles the translations in. `fallback` is what a key missing from a
// translation falls back to, so a half translated language shows English rather
// than the name of the key.
//
// It reads the files without telling cargo, which is what build.rs is there to
// fix: without it, editing a translation rebuilds nothing.
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
    //
    // The answer carries how the panel should look and speak, so it is applied
    // before the panel is built rather than after: the language especially, since
    // rust-i18n reads it as each string is rendered and nothing already on screen
    // would be rendered again.
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
    let who = identity.clone();

    // What the account chose, over what this browser had cached from last time.
    // Both were already applied before the first paint; this is where they are
    // corrected, and where somebody logging in on a borrowed machine stops seeing
    // its owner's colours.
    let theme = theme::settle();
    let accent = accent::settle();

    theme::adopt(theme, identity.preferences.theme.as_deref());
    accent::adopt(&accent, identity.preferences.accent.as_deref());
    locale::adopt(identity.preferences.locale.as_deref());

    // Reached by the one screen that offers them, rather than threaded through
    // every route on the way there.
    provide_context(theme);
    provide_context(accent::Accent(accent));

    // One stream for the whole panel, opened here and read in two places: the
    // header, which says a scan is running, and the screen that shows it in
    // full. Two connections would be two of everything for the same news.
    let scan = events::scan_status();

    view! {
        <Router>
            <layout::Shell identity on_out=forget scan>
                <Routes fallback=move || {
                    view! { <pages::Unbuilt heading=t!("nav.home").to_string() /> }
                }>
                    <Route
                        path=path!("/")
                        view={
                            let who = who.clone();
                            move || {
                                view! {
                                    <pages::home::Home
                                        identity=who.clone()
                                        scan
                                        admin
                                        on_expired=forget
                                    />
                                }
                            }
                        }
                    />
                    // Your own account, in three: who you are, what opens it, and
                    // how the panel looks to you. Not the screen an administrator
                    // sees on somebody else, because none of this is administration.
                    <Route
                        path=path!("/account")
                        view={
                            let who = who.clone();
                            move || {
                                view! {
                                    <pages::account::Profile who=who.clone() on_expired=forget />
                                }
                            }
                        }
                    />
                    <Route
                        path=path!("/account/access")
                        view={
                            let who = who.clone();
                            move || {
                                view! {
                                    <pages::account::Access who=who.clone() on_expired=forget />
                                }
                            }
                        }
                    />
                    <Route
                        path=path!("/account/preferences")
                        view=move || {
                            view! { <pages::account::Preferences on_expired=forget /> }
                        }
                    />

                    // The administration sections. The menu does not offer these
                    // to anybody else and the server refuses them regardless, so
                    // what is left to handle here is a URL typed by hand.
                    <Route
                        path=path!("/libraries")
                        view=move || {
                            if admin {
                                view! { <pages::libraries::Libraries on_expired=forget /> }
                                    .into_any()
                            } else {
                                view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
                            }
                        }
                    />
                    <Route
                        path=path!("/accounts")
                        view=move || {
                            if admin {
                                view! { <pages::accounts::Accounts on_expired=forget /> }.into_any()
                            } else {
                                view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
                            }
                        }
                    />
                    <Route
                        path=path!("/accounts/:username")
                        view={
                            let who = who.clone();
                            move || {
                                if admin {
                                    view! {
                                        <pages::accounts::Detail
                                            who=who.clone()
                                            on_expired=forget
                                        />
                                    }
                                        .into_any()
                                } else {
                                    view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
                                }
                            }
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

#[cfg(test)]
mod tests {
    use rust_i18n::t;

    /// Every key the code asks for, in both languages, resolving to something
    /// other than its own name.
    ///
    /// The check that exists because of `no`: YAML reads it as a boolean, so an
    /// unquoted `no:` becomes the key `false` and `t!("common.no")` quietly
    /// returns nothing useful. Comparing the answer against the key is what
    /// catches that, and it would have caught it before anybody saw "false" in a
    /// table.
    #[test]
    fn every_key_resolves_in_every_language() {
        let keys = collect_keys();
        assert!(
            keys.len() > 100,
            "the keys are read from the source, so a parse that finds none would pass everything"
        );

        for locale in ["en", "es"] {
            rust_i18n::set_locale(locale);

            for key in &keys {
                let key = key.as_str();
                let said = t!(key);
                assert_ne!(
                    said, key,
                    "{key} does not resolve in {locale}: rust-i18n hands back the key when it \
                     cannot find one"
                );
                assert!(!said.is_empty(), "{key} resolves to nothing in {locale}");

                // The check that `no` needed and the one above missed. YAML reads
                // yes, no, on and off as booleans in values too, so an unquoted
                // `no: No` resolves to the string "false" — which is not the key,
                // so comparing against the key said nothing was wrong, and what
                // ended up on screen was the word "false".
                assert!(
                    !matches!(said.as_ref(), "true" | "false"),
                    "{key} resolves to {said:?} in {locale}: YAML read the value as a boolean, \
                     which is what unquoted yes, no, on and off do"
                );
            }
        }
    }

    /// Read from the source rather than listed here, so a key added tomorrow is
    /// checked without anybody remembering to add it.
    fn collect_keys() -> Vec<String> {
        let mut keys = Vec::new();

        for source in [
            include_str!("main.rs"),
            include_str!("layout.rs"),
            include_str!("login.rs"),
            include_str!("pages/mod.rs"),
            include_str!("pages/home.rs"),
            include_str!("pages/libraries.rs"),
            include_str!("pages/accounts.rs"),
            include_str!("pages/account.rs"),
        ] {
            let mut rest = source;

            while let Some(at) = rest.find("t!(") {
                rest = &rest[at + 3..];
                let trimmed = rest.trim_start();

                if let Some(key) = trimmed
                    .strip_prefix('"')
                    .and_then(|quoted| quoted.split('"').next())
                    // Only what looks like one of ours, so the pattern in this
                    // very function does not count itself.
                    .filter(|key| key.contains('.') && !key.contains(' '))
                {
                    keys.push(key.to_string());
                }
            }
        }

        keys.sort();
        keys.dedup();
        keys
    }
}
