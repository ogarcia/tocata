// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The frame every screen sits in: sections down the left, and along the top the
//! button that opens what belongs to you rather than to the server.
//!
//! The header and the sections share one background, and the screen sits on
//! another. Nothing is divided by a line because the change of colour already
//! divides it, and a border between two panels of the same shade only draws
//! attention to a seam that need not exist.
//!
//! On a narrow screen the sections fold away behind a button. That was a
//! `checkbox` and a CSS rule, on the grounds that the browser already knows how
//! to remember whether something is open — but it does not know that you have
//! navigated, so choosing a section left the sections sitting over the screen
//! they had just taken you to. Whatever holds this has to hear about the one
//! thing CSS cannot see, so it is a signal.

use crate::api;
use crate::locale;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use rust_i18n::t;
use tocata::types::Identity;

/// A place to go, and what to call it.
///
/// Held as a function rather than a string so the label is fetched at render
/// time: it has to come out in whatever language is in force.
struct Section {
    path: &'static str,
    label: fn() -> String,
    /// Whether it is only worth drawing for somebody who administers the server.
    administration: bool,
}

/// What the collection is made of. Anything that belongs to the person rather
/// than to the server lives in the menu behind the button instead.
const SECTIONS: [Section; 5] = [
    Section {
        path: "/",
        label: || t!("nav.overview").to_string(),
        administration: false,
    },
    Section {
        path: "/libraries",
        label: || t!("nav.libraries").to_string(),
        administration: true,
    },
    Section {
        path: "/accounts",
        label: || t!("nav.accounts").to_string(),
        administration: true,
    },
    Section {
        path: "/settings",
        label: || t!("nav.settings").to_string(),
        administration: true,
    },
    Section {
        path: "/maintenance",
        label: || t!("nav.maintenance").to_string(),
        administration: true,
    },
];

#[component]
pub fn Shell(identity: Identity, on_out: Callback<()>, children: Children) -> impl IntoView {
    let admin = identity.admin;
    let (folded_out, fold) = signal(false);

    view! {
        <div class="shell">
            <aside class="side" class:out=move || folded_out.get()>
                <div class="brand">{t!("app.name")}</div>
                <nav>
                    {SECTIONS
                        .iter()
                        .filter(|section| admin || !section.administration)
                        .map(|section| {
                            view! {
                                // Going somewhere folds them away again. On a
                                // wide screen there is nothing to fold and this
                                // changes nothing.
                                <A
                                    href=section.path
                                    exact=section.path == "/"
                                    on:click=move |_| fold.set(false)
                                >
                                    {(section.label)()}
                                </A>
                            }
                        })
                        .collect_view()}
                </nav>
            </aside>

            // Over the screen while they are out, so a touch anywhere else folds
            // them rather than landing on whatever is underneath.
            <Show when=move || folded_out.get()>
                <div class="menu-shade" on:click=move |_| fold.set(false)></div>
            </Show>

            <header class="head">
                <button
                    class="menu-button"
                    title=t!("nav.menu")
                    aria-expanded=move || folded_out.get().to_string()
                    on:click=move |_| fold.update(|out| *out = !*out)
                >
                    "☰"
                </button>
                <You identity on_out />
            </header>

            <main class="body">{children()}</main>
        </div>
    }
}

/// The round button, and what it opens.
///
/// It closes when the focus leaves it, which is what a click anywhere else does.
/// No listener on the document, and no way for it to be left open behind a
/// screen that has already changed.
#[component]
fn You(identity: Identity, on_out: Callback<()>) -> impl IntoView {
    let (open, set_open) = signal(false);

    // The first letter, and only ever one: a name is text in a language we do
    // not know, so a character is taken rather than a byte sliced off.
    let initial = identity
        .username
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_default();

    let name = identity.username.clone();
    let admin = identity.admin;

    view! {
        <div class="you" on:focusout=move |_| set_open.set(false)>
            <button
                class="avatar"
                title=name.clone()
                aria-expanded=move || open.get().to_string()
                on:click=move |_| set_open.update(|shown| *shown = !*shown)
            >
                {initial}
            </button>

            <Show when=move || open.get()>
                <div class="menu">
                    <div class="menu-who">
                        <span class="quiet">{t!("header.you")}</span>
                        <strong>{name.clone()}</strong>
                        {if admin {
                            view! { <span class="badge">{t!("header.administrator")}</span> }
                                .into_any()
                        } else {
                            ().into_any()
                        }}
                    </div>

                    <A href="/account">{t!("nav.account")}</A>

                    <label class="menu-field">
                        <span class="quiet">{t!("header.language")}</span>
                        <Languages />
                    </label>

                    <button
                        class="menu-item"
                        on:click=move |_| {
                            spawn_local(async move {
                                api::log_out().await;
                                on_out.run(());
                            });
                        }
                    >
                        {t!("header.log_out")}
                    </button>
                </div>
            </Show>
        </div>
    }
}

/// Picking a language reloads the page, for the reason spelled out in `locale`.
#[component]
fn Languages() -> impl IntoView {
    let current = locale::current();

    view! {
        <select on:change:target=move |event| locale::choose(&event.target().value())>
            {locale::AVAILABLE
                .iter()
                .map(|(code, name)| {
                    view! {
                        <option value=*code selected=*code == current>
                            {*name}
                        </option>
                    }
                })
                .collect_view()}
        </select>
    }
}
