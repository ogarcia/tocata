// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The directories the music is read from.
//!
//! Cards rather than a table, because the widest thing on the screen is an
//! absolute path and a column that has to hold one either wraps badly or pushes
//! everything else off the side.
//!
//! Adding and editing share one dialogue. They ask for the same two things, and
//! the only difference is whether the fields start empty.
//!
//! Everything that changes a library answers with the library as it now is, so
//! the card is redrawn from what the server said rather than from what we hoped
//! it would say.

use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Library, LibraryChanges};

/// What the dialogue is being opened for.
#[derive(Clone, PartialEq, Eq)]
enum Editing {
    New,
    /// Which one, with the values it has now.
    Existing {
        id: i64,
        name: String,
        path: String,
    },
}

#[component]
pub fn Libraries(on_expired: Callback<()>) -> impl IntoView {
    // The one place the list lives. Everything below changes this rather than
    // asking the server again, since every call that changes a library hands the
    // new one back.
    let (libraries, set_libraries) = signal(Option::<Vec<Library>>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (editing, set_editing) = signal(Option::<Editing>::None);

    spawn_local(async move {
        match api::libraries().await {
            Ok(list) => set_libraries.set(Some(list)),
            Err(Failure::Unauthenticated) => on_expired.run(()),
            Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
        }
    });

    let settled = Callback::new(move |library: Library| {
        set_libraries.update(|list| {
            let Some(list) = list else { return };

            match list.iter_mut().find(|held| held.id == library.id) {
                Some(slot) => *slot = library,
                None => list.push(library),
            }
        });
    });

    let forget = Callback::new(move |id: i64| {
        set_libraries.update(|list| {
            if let Some(list) = list {
                list.retain(|held| held.id != id);
            }
        });
    });

    let edit = Callback::new(move |which: Editing| set_editing.set(Some(which)));

    view! {
        <div class="titled">
            <div>
                <h1>{t!("libraries.heading")}</h1>
                <p class="quiet lead">{t!("libraries.lead")}</p>
            </div>
            <button on:click=move |_| set_editing.set(Some(Editing::New))>
                <Glyph icon=Icon::Add />
                {t!("libraries.add")}
            </button>
        </div>

        {move || {
            failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        {move || match libraries.get() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(list) if list.is_empty() => {
                view! { <p class="quiet">{t!("libraries.none")}</p> }.into_any()
            }
            Some(list) => {
                view! {
                    <div class="cards">
                        {list
                            .into_iter()
                            .map(|library| {
                                view! {
                                    <Card
                                        library
                                        on_changed=settled
                                        on_removed=forget
                                        on_edit=edit
                                        on_expired
                                    />
                                }
                            })
                            .collect_view()}
                    </div>
                }
                    .into_any()
            }
        }}

        <Details editing set_editing on_settled=settled on_expired />
    }
}

/// The dialogue that asks for a path and a name, for a new library or an existing
/// one.
///
/// A real `dialog`, opened with `showModal`: it closes on Escape, keeps the focus
/// inside itself and dims what is behind it, none of which is written here.
#[component]
fn Details(
    editing: ReadSignal<Option<Editing>>,
    set_editing: WriteSignal<Option<Editing>>,
    on_settled: Callback<Library>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    let (path, set_path) = signal(String::new());
    let (name, set_name) = signal(String::new());
    let (failure, set_failure) = signal(Option::<String>::None);
    let (waiting, set_waiting) = signal(false);

    // Opening and closing is the browser's job, so it is done to the element
    // rather than by drawing something different.
    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        match editing.get() {
            Some(which) => {
                let (starting_name, starting_path) = match &which {
                    Editing::New => (String::new(), String::new()),
                    Editing::Existing { name, path, .. } => (name.clone(), path.clone()),
                };
                set_name.set(starting_name);
                set_path.set(starting_path);
                set_failure.set(None);
                let _ = element.show_modal();
            }
            None => element.close(),
        }
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();

        let Some(which) = editing.get() else { return };
        let asked = path.get().trim().to_string();
        let called = name.get().trim().to_string();

        set_waiting.set(true);
        set_failure.set(None);

        spawn_local(async move {
            let outcome = match which {
                Editing::New => {
                    api::add_library(asked, (!called.is_empty()).then_some(called)).await
                }
                // Both fields go every time. The server takes what changed and
                // ignores the rest, and working out which is which here would be
                // a second opinion about state the server already holds.
                Editing::Existing { id, .. } => {
                    api::change_library(
                        id,
                        LibraryChanges {
                            name: Some(called),
                            path: Some(asked),
                            enabled: None,
                        },
                    )
                    .await
                }
            };

            match outcome {
                Ok(library) => {
                    on_settled.run(library);
                    set_editing.set(None);
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_waiting.set(false);
        });
    };

    view! {
        <dialog
            node_ref=dialog
            class="sheet"
            // Escape closes it without going through the button, so the signal has
            // to hear about it or reopening would do nothing.
            on:close=move |_| set_editing.set(None)
        >
            <form on:submit=submit>
                <h2>
                    {move || {
                        match editing.get() {
                            Some(Editing::Existing { .. }) => t!("libraries.edit"),
                            _ => t!("libraries.add"),
                        }
                    }}
                </h2>

                <label for="path">{t!("libraries.path")}</label>
                <input
                    id="path"
                    placeholder="/srv/music"
                    required
                    prop:value=path
                    on:input:target=move |e| set_path.set(e.target().value())
                />
                // The server is the only one who can say whether the directory is
                // there, so this says what it will be looking for.
                <span class="hint quiet">{t!("libraries.path_note")}</span>

                <label for="name">{t!("libraries.name")}</label>
                <input
                    id="name"
                    placeholder=t!("libraries.name_default")
                    prop:value=name
                    on:input:target=move |e| set_name.set(e.target().value())
                />

                // Only when moving one, because only then is there anything under
                // an old path to reconcile.
                <Show when=move || matches!(editing.get(), Some(Editing::Existing { .. }))>
                    <p class="hint quiet">{t!("libraries.move_note")}</p>
                </Show>

                <p class="row">
                    <button type="submit" disabled=waiting>
                        {move || {
                            if waiting.get() { t!("login.working") } else { t!("common.save") }
                        }}
                    </button>
                    <button
                        type="button"
                        class="second"
                        disabled=waiting
                        on:click=move |_| set_editing.set(None)
                    >
                        {t!("common.cancel")}
                    </button>
                </p>

                {move || {
                    failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
                }}
            </form>
        </dialog>
    }
}

/// One library, with what it holds and what can be done to it.
#[component]
fn Card(
    library: Library,
    on_changed: Callback<Library>,
    on_removed: Callback<i64>,
    on_edit: Callback<Editing>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let id = library.id;
    let enabled = library.enabled;
    let name = library.name.clone();
    let path = library.path.clone();

    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);
    let (confirming, set_confirming) = signal(false);

    let switch = move |_| {
        set_busy.set(true);
        set_failure.set(None);

        spawn_local(async move {
            let changes = LibraryChanges {
                enabled: Some(!enabled),
                ..LibraryChanges::default()
            };

            match api::change_library(id, changes).await {
                Ok(library) => on_changed.run(library),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    };

    let remove = move |_| {
        set_busy.set(true);
        set_failure.set(None);

        spawn_local(async move {
            match api::remove_library(id).await {
                Ok(()) => on_removed.run(id),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => {
                    // `conflict` here is the server saying the library is still
                    // enabled, not that the path is taken, and the generic wording
                    // would say the wrong thing. It can only be reached by a race
                    // — another tab switching it back on — but a wrong message
                    // about a destructive action is worse than a vague one.
                    set_failure.set(Some(match &why {
                        Failure::Refused(code) if code == "conflict" => {
                            t!("libraries.disable_first").to_string()
                        }
                        other => said(other),
                    }));
                    set_confirming.set(false);
                }
            }
            set_busy.set(false);
        });
    };

    let editing = Editing::Existing {
        id,
        name: name.clone(),
        path: path.clone(),
    };

    view! {
        <section class="card" class:off=move || !enabled>
            <header>
                <h2>
                    <Glyph icon=Icon::Folder />
                    {name}
                </h2>
                {if enabled {
                    ().into_any()
                } else {
                    view! { <span class="badge muted">{t!("libraries.disabled")}</span> }.into_any()
                }}
            </header>

            <p class="path quiet">{path}</p>

            <p class="counts quiet">
                {t!("libraries.holds", tracks = library.tracks, missing = library.missing)}
                {library.last_scanned_at.as_deref().map(|at| format!(" · {}", when(at)))}
            </p>

            <p class="row wrap">
                <button
                    class="second small"
                    disabled=busy
                    on:click=move |_| on_edit.run(editing.clone())
                >
                    <Glyph icon=Icon::Rename />
                    {t!("libraries.edit")}
                </button>

                <button class="second small" disabled=busy on:click=switch>
                    <Glyph icon=if enabled { Icon::Off } else { Icon::On } />
                    {if enabled { t!("libraries.disable") } else { t!("libraries.enable") }}
                </button>

                // Only once it is switched off, which is the server's rule and not
                // ours: disabling costs nothing and is undone by asking again, so
                // requiring it first means no single misdirected click can take a
                // collection's history with it.
                <Show
                    when=move || !enabled
                    fallback=|| {
                        view! { <span class="hint quiet">{t!("libraries.disable_first")}</span> }
                    }
                >
                    <Show
                        when=move || confirming.get()
                        fallback=move || {
                            view! {
                                <button
                                    class="second small danger"
                                    disabled=busy
                                    on:click=move |_| set_confirming.set(true)
                                >
                                    <Glyph icon=Icon::Remove />
                                    {t!("libraries.remove")}
                                </button>
                            }
                        }
                    >
                        // Asked in place rather than in a dialogue: what is about
                        // to be destroyed is on this card, and a box in the middle
                        // of the screen would cover it.
                        <span class="confirm">
                            <span>{t!("libraries.remove_sure")}</span>
                            <button class="danger small" disabled=busy on:click=remove>
                                {t!("libraries.remove_yes")}
                            </button>
                            <button
                                class="second small"
                                disabled=busy
                                on:click=move |_| set_confirming.set(false)
                            >
                                {t!("common.cancel")}
                            </button>
                        </span>
                    </Show>
                </Show>
            </p>

            {move || {
                failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
            }}
        </section>
    }
}

/// A timestamp the way the reader's own machine writes one.
///
/// What we store is unambiguous and no way to show somebody a date. What comes
/// back here is whatever their locale says, which is the only correct answer and
/// not one we could work out ourselves.
fn when(iso: &str) -> String {
    let moment = js_sys::Date::new(&iso.into());

    if moment.get_time().is_nan() {
        return iso.to_string();
    }

    moment
        .to_locale_string(&crate::locale::current(), &js_sys::Object::new())
        .into()
}

/// What to say about a refusal. The codes are the server's own and stable, so
/// this can branch on them and say something useful in the reader's language.
fn said(why: &Failure) -> String {
    match why {
        Failure::Unreachable => t!("login.unreachable").to_string(),
        Failure::Refused(code) => match code.as_str() {
            "invalidRequest" => t!("libraries.bad_path").to_string(),
            "conflict" => t!("libraries.already").to_string(),
            _ => t!("common.refused").to_string(),
        },
        Failure::Unauthenticated => t!("common.refused").to_string(),
    }
}
