// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The frame every screen sits in.
//!
//! Down the left, where you can go. Along the top, what you can do from anywhere:
//! start a scan, and what belongs to you.
//!
//! The sections down the left are not one list but two, and which one is showing
//! depends on where you are. Inside your own account they are the parts of your
//! account; everywhere else they are the server's. It is the same panel either way,
//! and the point of swapping them is that neither list has to carry the other's
//! entries: the theme and the language used to be buttons in the header, on every
//! screen, for the two minutes in a year somebody wants them.
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
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use rust_i18n::t;
use tocata::types::{Identity, Status};

/// A place to go, and what to call it.
///
/// The label is a function rather than a string so it is fetched at render time:
/// it has to come out in whatever language is in force.
struct Section {
    path: &'static str,
    label: fn() -> String,
    icon: Icon,
    /// Whether the mark belongs to this entry only when the path is exactly this.
    /// Anything with sections under it needs this, or it lights up while somebody
    /// is inside one of its children and two entries claim to be where you are.
    exact: bool,
}

/// What anybody with a session can reach.
const EVERYONE: [Section; 1] = [Section {
    path: "/",
    label: || t!("nav.home").to_string(),
    icon: Icon::Home,
    exact: true,
}];

/// What your own account is made of.
///
/// Shown instead of everything else while you are in it, which is what lets the
/// first entry be the profile rather than a way back: the logo goes home, and so
/// does the entry that says so.
const MINE: [Section; 3] = [
    Section {
        path: "/account",
        label: || t!("nav.profile").to_string(),
        icon: Icon::Account,
        // The other two live under this path, so without this it would be lit
        // while somebody is on either of them.
        exact: true,
    },
    Section {
        path: "/account/access",
        label: || t!("nav.access").to_string(),
        icon: Icon::Key,
        exact: false,
    },
    Section {
        path: "/account/preferences",
        label: || t!("nav.preferences").to_string(),
        icon: Icon::Preferences,
        exact: false,
    },
];

/// Where your own account starts. Everything under it is yours; nothing under it
/// is administration, including for an administrator.
pub const MINE_PATH: &str = "/account";

/// What only an administrator can reach, gathered under one heading.
///
/// Called administration rather than settings because two of them are not
/// settings: a library is a place music comes from and an account is a person.
/// Grouping them under the wrong word would spend the word for nothing.
const ADMINISTRATION: [Section; 4] = [
    Section {
        path: "/libraries",
        label: || t!("nav.libraries").to_string(),
        icon: Icon::Libraries,
        exact: false,
    },
    Section {
        path: "/accounts",
        label: || t!("nav.accounts").to_string(),
        icon: Icon::Accounts,
        // One account of somebody else's lives under this, and the list is where
        // you came from rather than where you are.
        exact: false,
    },
    Section {
        path: "/settings",
        label: || t!("nav.settings").to_string(),
        icon: Icon::Settings,
        exact: false,
    },
    Section {
        path: "/maintenance",
        label: || t!("nav.maintenance").to_string(),
        icon: Icon::Maintenance,
        exact: false,
    },
];

#[component]
pub fn Shell(
    identity: Identity,
    on_out: Callback<()>,
    scan: ReadSignal<Option<Status>>,
    children: Children,
) -> impl IntoView {
    let admin = identity.admin;
    let (folded_out, fold) = signal(false);

    let location = use_location();
    let inside_mine = move || location.pathname.get().starts_with(MINE_PATH);

    view! {
        <div class="shell">
            <aside class="side" class:out=move || folded_out.get()>
                // The name goes home. It is the one thing on every screen that
                // everybody already tries to click.
                <A href="/" attr:class="brand" on:click=move |_| fold.set(false)>
                    <Glyph icon=Icon::Logo />
                    {t!("app.name")}
                </A>

                <nav>
                    <Show
                        when=move || inside_mine()
                        fallback=move || {
                            view! {
                                {EVERYONE
                                    .iter()
                                    .map(|section| view! { <Entry section fold /> })
                                    .collect_view()}
                                <Show when=move || admin>
                                    <Group fold />
                                </Show>
                            }
                        }
                    >
                        // Said out loud, because the sections having been swapped
                        // under somebody is not something to make them infer.
                        <p class="nav-title">{t!("nav.account")}</p>
                        {MINE.iter().map(|section| view! { <Entry section fold /> }).collect_view()}
                    </Show>
                </nav>
            </aside>

            // Over the screen while the sections are out, so a touch anywhere
            // else folds them rather than landing on what is underneath.
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
                    <Glyph icon=Icon::Menu />
                </button>

                // Grouped rather than pushed one by one: whatever is in here
                // sits at the right hand end, and adding another button later
                // does not depend on which of them happens to come first.
                <div class="tools">
                    <Scanning scan />
                    <Show when=move || admin>
                        <StartScan scan />
                    </Show>
                    <You identity on_out />
                </div>
            </header>

            <main class="body">{children()}</main>
        </div>
    }
}

/// One place to go. Going there folds the sections away again, which on a wide
/// screen changes nothing because there is nothing folded.
#[component]
fn Entry(section: &'static Section, fold: WriteSignal<bool>) -> impl IntoView {
    view! {
        <A href=section.path exact=section.exact on:click=move |_| fold.set(false)>
            <Glyph icon=section.icon />
            {(section.label)()}
        </A>
    }
}

/// The administration sections, behind a heading that folds.
///
/// Arriving inside one of them opens it, so the menu never disagrees with the
/// screen about where you are. Closing it again is allowed even then — it is a
/// fold, and one that refuses to fold is a decoration — and while it is closed
/// over the section you are in, the heading itself carries the mark. Something
/// has to say where you are.
#[component]
fn Group(fold: WriteSignal<bool>) -> impl IntoView {
    let location = use_location();
    let inside = move || {
        let path = location.pathname.get();
        ADMINISTRATION.iter().any(|section| section.path == path)
    };

    let (open, set_open) = signal(inside());

    // Opens on the way in, and only then. Landing inside from a typed URL or a
    // reload should show where that is; closing it afterwards is a choice this
    // does not undo, because nothing it watches has changed.
    Effect::new(move |_| {
        if inside() {
            set_open.set(true);
        }
    });

    view! {
        <button
            class="group"
            class:open=move || open.get()
            class:current=move || !open.get() && inside()
            aria-expanded=move || open.get().to_string()
            on:click=move |_| set_open.update(|shown| *shown = !*shown)
        >
            <Glyph icon=Icon::Settings />
            {t!("nav.administration")}
            <span class="chevron">
                <Glyph icon=Icon::Chevron />
            </span>
        </button>

        <Show when=move || open.get()>
            <div class="grouped">
                {ADMINISTRATION
                    .iter()
                    .map(|section| view! { <Entry section fold /> })
                    .collect_view()}
            </div>
        </Show>
    }
}

/// Says that a scan is running, from wherever in the panel you happen to be.
///
/// A scan takes minutes and people go and do something else while it runs. If the
/// only place that said so were its own screen, they would have to keep going
/// back to look; the stream is already open, so the header can say it for free.
///
/// Silent when nothing is running: a permanent badge saying "idle" is furniture.
#[component]
fn Scanning(scan: ReadSignal<Option<Status>>) -> impl IntoView {
    view! {
        <Show when=move || scan.get().is_some_and(|status| status.scanning)>
            <A href="/" attr:class="scanning" attr:title=t!("scan.running")>
                <Glyph icon=Icon::Scan />
                <span class="counted">
                    {move || scan.get().map(|status| status.tracks).unwrap_or_default()}
                </span>
            </A>
        </Show>
    }
}

/// Starting a scan without going anywhere to do it.
///
/// Two kinds, so it is a menu rather than a button: one of them reads every file
/// again and takes as long as the collection is big, which is not something to set
/// off by aiming badly.
///
/// The icon carries it alone. Two arrows going round is what every program on this
/// machine uses for "go and look again", and a word beside it would be a word
/// nobody needs to read twice.
///
/// Gone while a scan runs. Cancelling stays on the scan's own screen, because
/// stopping something should mean having looked at what is being stopped.
#[component]
fn StartScan(scan: ReadSignal<Option<Status>>) -> impl IntoView {
    let (open, set_open) = signal(false);

    let start = move |full: bool| {
        set_open.set(false);
        spawn_local(async move {
            let _ = api::start_scan(full).await;
        });
    };

    view! {
        <Show when=move || !scan.get().is_some_and(|status| status.scanning)>
            <div class="dropdown">
                <button
                    class="plain"
                    title=t!("scan.start")
                    aria-expanded=move || open.get().to_string()
                    on:click=move |_| set_open.update(|shown| *shown = !*shown)
                >
                    <Glyph icon=Icon::Scan />
                </button>

                <Show when=move || open.get()>
                    <div class="veil" on:click=move |_| set_open.set(false)></div>
                    <div class="menu">
                        // The note is inside the button rather than under it.
                        // Beside it, it looked like something to click that did
                        // nothing when clicked, and left a gap in the middle of
                        // the menu that highlighted neither entry.
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
        </Show>
    }
}

/// The round button, and what it opens.
///
/// Closing it was `focusout` on the container, which reads well and does not
/// work: the order is mousedown, then focusout, then click, so the menu went away
/// before the click could land on anything in it. Nothing inside was clickable.
///
/// It closes the way the folded sections do — a sheet behind the menu catches
/// anything aimed elsewhere — and every entry closes it on the way out, since
/// choosing one is finishing with it.
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
        <div class="dropdown you">
            <button
                class="avatar"
                title=name.clone()
                aria-expanded=move || open.get().to_string()
                on:click=move |_| set_open.update(|shown| *shown = !*shown)
            >
                {initial}
            </button>

            <Show when=move || open.get()>
                <div class="veil" on:click=move |_| set_open.set(false)></div>
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

                    <A href="/account" on:click=move |_| set_open.set(false)>
                        <Glyph icon=Icon::Account />
                        {t!("nav.account")}
                    </A>

                    <button
                        class="menu-item"
                        on:click=move |_| {
                            set_open.set(false);
                            spawn_local(async move {
                                api::log_out().await;
                                on_out.run(());
                            });
                        }
                    >
                        <Glyph icon=Icon::LogOut />
                        {t!("header.log_out")}
                    </button>
                </div>
            </Show>
        </div>
    }
}
