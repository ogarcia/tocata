// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The directories the music is read from.
//!
//! One row each, on three columns: what it is called and where it is, how much is
//! in it and when that was last looked at, and what can be done about it. These
//! were cards on a raised background, on the grounds that a path is too wide for a
//! column — and a path is too wide for a column, which is why the one that holds it
//! is the one that takes whatever width is going and wraps mid-path when it has to.
//! The other two are fixed, so the figures and the actions of every row line up.
//!
//! Adding and editing share one dialogue. They ask for the same two things, and
//! the only difference is whether the fields start empty.
//!
//! Everything that changes a library answers with the library as it now is, so
//! the row is redrawn from what the server said rather than from what we hoped
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

/// Which library is being removed, while it is being asked about.
#[derive(Clone, PartialEq, Eq)]
struct Removing {
    id: i64,
    name: String,
}

#[component]
pub fn Libraries(on_expired: Callback<()>) -> impl IntoView {
    // The one place the list lives. Everything below changes this rather than
    // asking the server again, since every call that changes a library hands the
    // new one back.
    let (libraries, set_libraries) = signal(Option::<Vec<Library>>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (editing, set_editing) = signal(Option::<Editing>::None);
    let (removing, set_removing) = signal(Option::<Removing>::None);

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
    let ask = Callback::new(move |which: Removing| set_removing.set(Some(which)));

    // A row that failed says so here rather than inside itself. What goes wrong is
    // a sentence, a row is three narrow columns, and a sentence appearing inside one
    // of them pushes the whole list down by however many lines it takes.
    let failed = Callback::new(move |why: String| set_failure.set(Some(why)));

    view! {
        <header class="titled">
            <div>
                <h1>{t!("libraries.heading")}</h1>
                <p class="quiet lead">{t!("libraries.lead")}</p>
            </div>
            <button class="pill solid" on:click=move |_| set_editing.set(Some(Editing::New))>
                <Glyph icon=Icon::Add />
                {t!("libraries.add")}
            </button>
        </header>

        {move || {
            failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        {move || match libraries.get() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(list) if list.is_empty() => {
                view! { <p class="nothing">{t!("libraries.none")}</p> }.into_any()
            }
            Some(list) => {
                view! {
                    // A list, because it is one: the rows are siblings of equal
                    // weight and nothing about them is a heading over the next.
                    <ul class="shelf">
                        {list
                            .into_iter()
                            .map(|library| {
                                view! {
                                    <Held
                                        library
                                        on_changed=settled
                                        on_edit=edit
                                        on_remove=ask
                                        on_failed=failed
                                        on_expired
                                    />
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }
        }}

        <Details editing set_editing on_settled=settled on_expired />
        <Confirm removing set_removing on_removed=forget on_expired />
    }
}

/// Asks before removing one, in a dialogue rather than in the row.
///
/// In the row it was three words where two had been, which wrapped inside a column
/// 120 pixels wide and made every row below it jump down a line — a list that moves
/// while you are aiming at it. A `dialog` floats over the screen, so nothing behind
/// it moves at all, and it has room for the whole question and for the name of what
/// is about to go, which a column that narrow never had.
///
/// One for the screen rather than one per row: what it needs to know is which
/// library, and that fits in the signal that opens it.
#[component]
fn Confirm(
    removing: ReadSignal<Option<Removing>>,
    set_removing: WriteSignal<Option<Removing>>,
    on_removed: Callback<i64>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    let (failure, set_failure) = signal(Option::<String>::None);
    let (waiting, set_waiting) = signal(false);

    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        match removing.get() {
            Some(_) => {
                set_failure.set(None);
                let _ = element.show_modal();
            }
            None => element.close(),
        }
    });

    let remove = move |_| {
        let Some(which) = removing.get() else { return };

        set_waiting.set(true);
        set_failure.set(None);

        spawn_local(async move {
            match api::remove_library(which.id).await {
                Ok(()) => {
                    on_removed.run(which.id);
                    set_removing.set(None);
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => {
                    // `conflict` here is the server saying the library is still
                    // enabled, not that the path is taken, and the generic wording
                    // would say the wrong thing. It can only be reached by a race —
                    // another tab switching it back on — but a wrong message about a
                    // destructive action is worse than a vague one.
                    set_failure.set(Some(match &why {
                        Failure::Refused(code) if code == "conflict" => {
                            t!("libraries.disable_first").to_string()
                        }
                        other => said(other),
                    }));
                }
            }
            set_waiting.set(false);
        });
    };

    view! {
        // Narrow, because it is a sentence and two answers. The width of a form
        // around one line of text is a box mostly full of nothing.
        <dialog
            node_ref=dialog
            class="sheet narrow"
            on:close=move |_| set_removing.set(None)
        >
            <div class="sheet-body">
                // Named in the title, because "remove it" in the middle of the
                // screen no longer has the row beside it to say which one.
                <h2>
                    {move || {
                        removing
                            .get()
                            .map(|which| t!("libraries.remove_this", name = which.name).to_string())
                    }}
                </h2>
                <p class="sheet-lead">{t!("libraries.remove_note")}</p>
            </div>

            <div class="sheet-foot">
                // "Keep it", not "Cancel". What the safe answer does is worth saying
                // when the other one cannot be undone.
                <button
                    type="button"
                    class="away"
                    disabled=waiting
                    on:click=move |_| set_removing.set(None)
                >
                    {t!("common.keep")}
                </button>
                <button type="button" class="pill solid undoing" disabled=waiting on:click=remove>
                    {move || {
                        if waiting.get() { t!("login.working") } else { t!("libraries.remove_yes") }
                    }}
                </button>
            </div>

            {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}
        </dialog>
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
            // One form around the whole thing, footer included, so the primary
            // action is a submit and Enter in either field does what the button
            // does.
            <form on:submit=submit>
                <div class="sheet-body">
                    <h2>
                        {move || {
                            match editing.get() {
                                Some(Editing::Existing { .. }) => t!("libraries.edit"),
                                _ => t!("libraries.add"),
                            }
                        }}
                    </h2>
                    <p class="sheet-lead">{t!("libraries.lead_sheet")}</p>

                    <div class="sheet-content">
                        // What it is called comes first: somebody adding a library
                        // has decided to add "the vinyl rips" and then goes looking
                        // for where they are, not the other way round.
                        <label>
                            <span>{t!("libraries.name")}</span>
                            <input
                                placeholder=t!("libraries.name_default")
                                autofocus
                                prop:value=name
                                on:input:target=move |e| set_name.set(e.target().value())
                            />
                        </label>

                        <label>
                            <span>{t!("libraries.path")}</span>
                            // Monospaced: a path is text where every character
                            // counts and none should be guessed at.
                            <input
                                class="exact"
                                placeholder="/srv/music"
                                required
                                prop:value=path
                                on:input:target=move |e| set_path.set(e.target().value())
                            />
                            // The server is the only one who can say whether the
                            // directory is there, so this says what it looks for.
                            <span class="hint">{t!("libraries.path_note")}</span>
                        </label>

                        // Only when moving one, because only then is there anything
                        // under an old path to reconcile.
                        <Show when=move || matches!(editing.get(), Some(Editing::Existing { .. }))>
                            <p class="hint">{t!("libraries.move_note")}</p>
                        </Show>
                    </div>
                </div>

                <div class="sheet-foot">
                    <button
                        type="button"
                        class="away"
                        disabled=waiting
                        on:click=move |_| set_editing.set(None)
                    >
                        {t!("common.cancel")}
                    </button>
                    <button type="submit" class="pill solid" disabled=waiting>
                        {move || {
                            if waiting.get() { t!("login.working") } else { t!("common.save") }
                        }}
                    </button>
                </div>

                {move || {
                    failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
                }}
            </form>
        </dialog>
    }
}

/// One library, with what it holds and what can be done to it.
#[component]
fn Held(
    library: Library,
    on_changed: Callback<Library>,
    on_edit: Callback<Editing>,
    on_remove: Callback<Removing>,
    on_failed: Callback<String>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let id = library.id;
    let enabled = library.enabled;
    let name = library.name.clone();
    let path = library.path.clone();

    let (busy, set_busy) = signal(false);

    let switch = move |_| {
        set_busy.set(true);

        spawn_local(async move {
            let changes = LibraryChanges {
                enabled: Some(!enabled),
                ..LibraryChanges::default()
            };

            match api::change_library(id, changes).await {
                Ok(library) => on_changed.run(library),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => on_failed.run(said(&why)),
            }
            set_busy.set(false);
        });
    };

    let editing = Editing::Existing {
        id,
        name: name.clone(),
        path: path.clone(),
    };

    // Held rather than cloned into the closure below: what asks for the removal is
    // inside a `Show`, whose children are called again every time its condition
    // changes, so a closure that consumed the name would only work the first time.
    let named = StoredValue::new(name.clone());

    let scanned = library.last_scanned_at.clone();
    let missing = library.missing;

    view! {
        <li class:off=move || !enabled>
            <div class="what">
                <h2>{name}</h2>
                <p class="path quiet">{path}</p>
            </div>

            <div class="much">
                // Unclassed: the column it is in is already right aligned and
                // tabular, and this is its own first line.
                <span>{thousands(library.tracks)}</span>
                // Both on one line, and short enough to stay on it: the count of
                // what is missing only when something is, and how long ago rather
                // than the date and time it happened.
                <span class="quiet">
                    {if missing > 0 {
                        format!("{} · ", t!("libraries.missing", count = missing))
                    } else {
                        String::new()
                    }}
                    {scanned
                        .as_deref()
                        .map(super::since)
                        .unwrap_or_else(|| t!("scan.never").to_string())}
                </span>
            </div>

            <div class="doing">
                <span class=if enabled { "state on" } else { "state" }>
                    {if enabled { t!("libraries.on") } else { t!("libraries.off") }}
                </span>

                <span class="actions">
                    // Always, switched on or off. Nothing about a library being set
                    // aside makes its name or its path any less editable, and a row
                    // that offers to turn it on but not to correct the path it is
                    // turned on to makes somebody switch it on to fix it.
                    <button
                        class="link"
                        disabled=busy
                        on:click=move |_| on_edit.run(editing.clone())
                    >
                        {t!("libraries.edit")}
                    </button>

                    <button class="link" disabled=busy on:click=switch>
                        {if enabled { t!("libraries.disable") } else { t!("libraries.enable") }}
                    </button>

                    // Only once it is switched off, which is the server's rule and
                    // not ours: disabling costs nothing and is undone by asking
                    // again, so requiring it first means no single misdirected click
                    // can take a collection's history with it. Offered rather than
                    // explained, because a row has no space for the explanation and
                    // the way to it is the button beside this one.
                    <Show when=move || !enabled>
                        <button
                            class="link risky"
                            disabled=busy
                            on:click=move |_| {
                                on_remove.run(Removing { id, name: named.get_value() })
                            }
                        >
                            {t!("libraries.remove")}
                        </button>
                    </Show>
                </span>
            </div>
        </li>
    }
}

/// Grouped with a space, the same way every other figure in the panel is.
fn thousands(count: i64) -> String {
    let digits = count.abs().to_string();
    let mut out = String::new();

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }

    out
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
