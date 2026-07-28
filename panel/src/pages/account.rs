// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Your own account: who you are, what opens it, and how the panel looks to you.
//!
//! Three screens rather than one, and the sections down the left become them while
//! you are in here. An account is not a form: it holds a profile that changes twice
//! a year, credentials that want a table of their own, and preferences that used to
//! be two buttons in the header of every screen in the panel.
//!
//! Separate from the accounts an administrator manages, even where the calls
//! underneath are the same. Administering somebody is deciding what they may reach
//! and cutting them off; looking after your own account is neither of those things,
//! and one screen trying to be both was a screen where half of what was offered was
//! offered to the wrong person.
//!
//! An administrator opening their own account from the list lands here too, because
//! there is no version of this that is administration.

use super::{said, when};
use crate::accent::{self, Accent};
use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use crate::locale;
use crate::theme::{self, Theme};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Account, AccountChanges, Identity, PreferenceChanges};
use wasm_bindgen::JsCast;

/// Who you are, in two forms rather than one.
///
/// The line between them is what a mistake costs. Your name, your address and your
/// password are what you log in with and how a lost password would ever come back,
/// so changing any of them asks for the password as it is now — not because the
/// session is in doubt but because the person in front of it is: a session proved
/// itself when it was opened, and an open browser is exactly the case that proves
/// nothing about who is sitting there now.
///
/// Everything else is a preference. Nothing is lost by getting one wrong, so nothing
/// is asked for, and asking anyway would be the surest way to teach somebody to type
/// their password without reading why.
///
/// The administrator tick appears in neither: nobody may take it off themselves, and
/// a disabled box explaining that is a box that exists to be refused.
#[component]
pub fn Profile(who: Identity, on_expired: Callback<()>) -> impl IntoView {
    let (account, set_account) = signal(Option::<Account>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (note, set_note) = signal(Option::<String>::None);

    let me = StoredValue::new(who.username.clone());

    let load = move || {
        spawn_local(async move {
            match api::account(&me.get_value()).await {
                Ok(found) => set_account.set(Some(found)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });
    };

    load();

    // Changing your own name changes what every call here asks about, so the held
    // name follows it. Without that, saving twice would ask about somebody who no
    // longer exists under that name.
    let save = Callback::new(move |changes: AccountChanges| {
        set_failure.set(None);
        set_note.set(None);

        spawn_local(async move {
            match api::change_account(&me.get_value(), changes).await {
                Ok(fresh) => {
                    me.set_value(fresh.username.clone());
                    set_note.set(Some(t!("common.saved").to_string()));
                    set_account.set(Some(fresh));
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
        });
    });

    view! {
        {move || match account.get() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(account) => {
                view! {
                    <div class="titled">
                        <div>
                            <h1>{account.username.clone()}</h1>
                            <p class="quiet lead">
                                {account
                                    .email
                                    .clone()
                                    .unwrap_or_else(|| t!("accounts.no_email").to_string())}
                            </p>
                        </div>
                    </div>

                    <Credentials account=account.clone() save />
                    <Listening account save />
                }
                    .into_any()
            }
        }}

        {move || note.get().map(|said| view! { <p class="note">{said}</p> })}
        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}
    }
}

/// The three that are worth being careful about, and the password that guards them.
///
/// A new password is asked for twice. Nothing can read one back — what is stored is
/// a hash — so a typo in the only copy is not a mistake anybody could find later: it
/// is an account whose password nobody knows, including its owner.
#[component]
fn Credentials(account: Account, save: Callback<AccountChanges>) -> impl IntoView {
    let (username, set_username) = signal(account.username.clone());
    let (email, set_email) = signal(account.email.clone().unwrap_or_default());
    let (password, set_password) = signal(String::new());
    let (again, set_again) = signal(String::new());
    let (current, set_current) = signal(String::new());

    // Only once both have something in them. Complaining while somebody is still
    // typing the second one is complaining about a word half written.
    let mismatched = move || {
        !again.get().is_empty() && !password.get().is_empty() && again.get() != password.get()
    };

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();

        let password = password.get();

        if password != again.get() {
            return;
        }

        save.run(AccountChanges {
            username: Some(username.get().trim().to_string()),
            email: Some(email.get().trim().to_string()),
            password: (!password.is_empty()).then_some(password),
            current_password: Some(current.get()),
            ..Default::default()
        });

        // Cleared whatever the server says. On success there is nothing left to
        // send; on failure it is a password that was mistyped, and offering the
        // mistyping back to be corrected is offering somebody a guess at what they
        // meant.
        set_password.set(String::new());
        set_again.set(String::new());
        set_current.set(String::new());
    };

    view! {
        <section class="pane wide">
            <h2>{t!("accounts.who")}</h2>

            <form class="stacked" on:submit=submit>
                // Who you are on one side and what you get in with on the other.
                // Two columns rather than a single file of five boxes, which is a
                // form as long as the screen for what is two short lists. They
                // collapse into one when there is no room for two.
                <div class="columns">
                    <div class="column">
                        <div class="field">
                            <label for="name">{t!("accounts.username")}</label>
                            <input
                                id="name"
                                required
                                prop:value=username
                                on:input:target=move |e| set_username.set(e.target().value())
                            />
                        </div>

                        <div class="field">
                            <label for="mail">{t!("accounts.email")}</label>
                            <input
                                id="mail"
                                type="email"
                                prop:value=email
                                on:input:target=move |e| set_email.set(e.target().value())
                            />
                        </div>
                    </div>

                    <div class="column">
                        <div class="field">
                            <label for="pass">{t!("accounts.new_password")}</label>
                            <input
                                id="pass"
                                type="password"
                                autocomplete="new-password"
                                placeholder=t!("accounts.unchanged")
                                prop:value=password
                                on:input:target=move |e| set_password.set(e.target().value())
                            />
                        </div>

                        <div class="field">
                            <label for="pass-again">{t!("accounts.repeat_password")}</label>
                            <input
                                id="pass-again"
                                type="password"
                                autocomplete="new-password"
                                class:wrong=mismatched
                                prop:value=again
                                on:input:target=move |e| set_again.set(e.target().value())
                            />
                            // Under the second box rather than the first, so the
                            // note about the pair does not push its other half down
                            // the column and out of line with the address beside it.
                            <Show
                                when=mismatched
                                fallback=move || {
                                    view! {
                                        <span class="hint quiet">
                                            {t!("accounts.password_note_mine")}
                                        </span>
                                    }
                                }
                            >
                                <span class="hint alarm">{t!("accounts.mismatch")}</span>
                            </Show>
                        </div>
                    </div>
                </div>

                // Last, and set apart: everything above is what is being asked for,
                // and this is the asking answered. The label says what it is for, so
                // there is nothing left for a note to explain.
                <div class="field guarded">
                    <label for="current">{t!("accounts.confirm_with_password")}</label>
                    <input
                        id="current"
                        type="password"
                        autocomplete="current-password"
                        prop:value=current
                        on:input:target=move |e| set_current.set(e.target().value())
                    />
                </div>

                <p class="row ends">
                    // Nothing here can be saved without it, so the button says so by
                    // being unavailable rather than by letting somebody press it and
                    // reading a refusal.
                    <button type="submit" disabled=move || current.get().is_empty() || mismatched()>
                        {t!("common.save")}
                    </button>
                </p>
            </form>
        </section>
    }
}

/// What the server does with what you play.
///
/// Its own pane because nothing in it is worth guarding, and because it is where
/// scrobbling to a service will go when there is one: the switch and the service it
/// hands plays to belong together, and neither is a credential.
#[component]
fn Listening(account: Account, save: Callback<AccountChanges>) -> impl IntoView {
    let (scrobbling, set_scrobbling) = signal(account.scrobbling);

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();

        save.run(AccountChanges {
            scrobbling: Some(scrobbling.get()),
            ..Default::default()
        });
    };

    view! {
        <section class="pane wide">
            <h2>{t!("accounts.listening")}</h2>
            <p class="hint quiet">{t!("accounts.listening_note")}</p>

            <form class="stacked" on:submit=submit>
                <div class="checks">
                    <label class="checkbox">
                        <input
                            type="checkbox"
                            prop:checked=scrobbling
                            on:change:target=move |e| set_scrobbling.set(e.target().checked())
                        />
                        {t!("accounts.scrobbling")}
                    </label>
                </div>

                <p class="row ends">
                    <button type="submit">{t!("common.save")}</button>
                </p>
            </form>
        </section>
    }
}

/// What opens your account: the keys clients hold, and the logins you have here.
///
/// Both are yours to look at one at a time, which is the difference from what an
/// administrator sees: they get to cut you off, and cutting somebody off does not
/// need to know that your third key is called "phone".
#[component]
pub fn Access(who: Identity, on_expired: Callback<()>) -> impl IntoView {
    view! {
        <div class="titled">
            <div>
                <h1>{t!("nav.access")}</h1>
                <p class="quiet lead">{t!("access.lead")}</p>
            </div>
        </div>

        <MyKeys username=who.username.clone() on_expired />
        <MySessions username=who.username on_expired />
    }
}

/// How the panel looks and speaks to you.
///
/// Every one of these takes effect as it is pressed and is written to the account at
/// the same time, so there is no save button: the panel changing colour under the
/// hand that chose the colour is the confirmation, and a button would only be a way
/// to make it not happen yet.
///
/// They live with the account rather than in this browser, so logging in on the
/// phone brings them along.
#[component]
pub fn Preferences(on_expired: Callback<()>) -> impl IntoView {
    let theme = expect_context::<RwSignal<Theme>>();
    let Accent(accent) = expect_context::<Accent>();

    let (failure, set_failure) = signal(Option::<String>::None);

    // Applied first and stored after, because the change is already visible and
    // waiting for the server to agree would be a delay with nothing behind it. What
    // the server says is only whether it will still be true tomorrow, which is what
    // the failure below is for.
    let store = move |changes: PreferenceChanges| {
        set_failure.set(None);

        spawn_local(async move {
            match api::set_preferences(changes).await {
                Ok(_) => {}
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("prefs.unsaved").to_string())),
            }
        });
    };

    // Following the machine is the absence of a theme rather than a theme of its
    // own, so choosing it clears the field instead of writing "auto" into it.
    let pick_theme = move |chosen: Theme| {
        theme::choose(theme, chosen);
        store(PreferenceChanges {
            theme: Some((chosen != Theme::Auto).then(|| chosen.name().to_string())),
            ..Default::default()
        });
    };

    // Same again for the colour the panel already ships with: choosing it is
    // choosing nothing, and storing its name would be storing a preference that
    // says what the default says.
    let pick_accent = move |chosen: &'static str| {
        accent::choose(&accent, chosen);
        store(PreferenceChanges {
            accent: Some((chosen != accent::DEFAULT).then(|| chosen.to_string())),
            ..Default::default()
        });
    };

    // The language is the one that cannot take effect where it stands: rust-i18n
    // keeps it in a global that nothing reactive watches, so what applies it is a
    // reload. Stored first, since the reload is what reads it back.
    let pick_locale = move |chosen: Option<String>| {
        spawn_local(async move {
            match api::set_preferences(PreferenceChanges {
                locale: Some(chosen.clone()),
                ..Default::default()
            })
            .await
            {
                Ok(_) => locale::choose(chosen.as_deref()),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("prefs.unsaved").to_string())),
            }
        });
    };

    view! {
        <div class="titled">
            <div>
                <h1>{t!("nav.preferences")}</h1>
                <p class="quiet lead">{t!("prefs.lead")}</p>
            </div>
        </div>

        <section class="pane wide">
            <h2>{t!("looks.heading")}</h2>

            <div class="field">
                <span class="label">{t!("prefs.theme")}</span>
                <div class="choices">
                    {theme::AVAILABLE
                        .iter()
                        .map(|choice| {
                            let choice = *choice;
                            view! {
                                <button
                                    class="second"
                                    class:chosen=move || theme.get() == choice
                                    aria-pressed=move || (theme.get() == choice).to_string()
                                    on:click=move |_| pick_theme(choice)
                                >
                                    {looks_label(choice)}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
                <span class="hint quiet">{t!("prefs.theme_note")}</span>
            </div>

            <div class="field">
                <span class="label">{t!("prefs.accent")}</span>
                // The colour of each swatch is the same rule that colours the panel,
                // applied to the button: the attribute redefines the variable for
                // whatever carries it, so plum is plum here whatever is in force.
                <div class="swatches">
                    {accent::AVAILABLE
                        .iter()
                        .map(|choice| {
                            let choice = *choice;
                            view! {
                                <button
                                    class="swatch"
                                    class:chosen=move || accent.get() == choice
                                    attr:data-accent=choice
                                    title=accent_label(choice)
                                    aria-label=accent_label(choice)
                                    aria-pressed=move || (accent.get() == choice).to_string()
                                    on:click=move |_| pick_accent(choice)
                                ></button>
                            }
                        })
                        .collect_view()}
                </div>
            </div>
        </section>

        <section class="pane wide">
            <h2>{t!("prefs.language")}</h2>

            <div class="field">
                <label for="locale">{t!("prefs.language")}</label>
                <select
                    id="locale"
                    on:change:target=move |event| {
                        let chosen = event.target().value();
                        pick_locale((!chosen.is_empty()).then_some(chosen));
                    }
                >
                    // Empty rather than absent, because "whatever the browser asks
                    // for" is a choice somebody can come back to.
                    <option value="" selected=move || !locale::chosen()>
                        {t!("prefs.language_auto")}
                    </option>
                    {locale::AVAILABLE
                        .iter()
                        .map(|(code, name)| {
                            view! {
                                <option
                                    value=*code
                                    selected=locale::chosen() && *code == locale::current()
                                >
                                    {*name}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
                <span class="hint quiet">{t!("prefs.language_note")}</span>
            </div>
        </section>

        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}
    }
}

fn looks_label(theme: Theme) -> String {
    match theme {
        Theme::Auto => t!("looks.auto").to_string(),
        Theme::Light => t!("looks.light").to_string(),
        Theme::Dark => t!("looks.dark").to_string(),
    }
}

/// The colours have names so that a swatch is not a coloured square with nothing to
/// read: it is what a screen reader says and what a tooltip shows.
fn accent_label(accent: &str) -> String {
    match accent {
        "teal" => t!("accent.teal").to_string(),
        "green" => t!("accent.green").to_string(),
        "amber" => t!("accent.amber").to_string(),
        "crimson" => t!("accent.crimson").to_string(),
        "plum" => t!("accent.plum").to_string(),
        _ => t!("accent.blue").to_string(),
    }
}

/// Your own keys: making them, rotating them, taking them away.
///
/// A key is readable once, when it is made and when it is rotated. What the
/// database keeps is a hash, so there is no second chance to look at it.
///
/// Which is why both of those happen in a dialogue that shows the secret and
/// forgets it on the way out. Shown in the page instead, it was a box that
/// appeared and disappeared — moving everything under it — and a piece of state
/// somebody had to remember to clear when the key it belonged to was revoked.
/// A dialogue closes, and closing it is the clearing.
#[component]
fn MyKeys(username: String, on_expired: Callback<()>) -> impl IntoView {
    let (keys, set_keys) = signal(Option::<Vec<tocata::types::Key>>::None);
    let (asking, set_asking) = signal(false);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);
    let (cutting, set_cutting) = signal(false);

    let who = StoredValue::new(username);

    let load = move || {
        spawn_local(async move {
            match api::keys(&who.get_value()).await {
                Ok(list) => set_keys.set(Some(list)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });
    };

    load();

    // Rotating shows a secret too, so it opens the same dialogue rather than
    // inventing a second way to show one.
    let (rotated, set_rotated) = signal(Option::<tocata::types::IssuedKey>::None);

    let act = Callback::new(move |(what, id): (KeyAction, i64)| {
        set_busy.set(true);
        set_failure.set(None);

        spawn_local(async move {
            match what {
                KeyAction::Rotate => match api::rotate_key(&who.get_value(), id).await {
                    Ok(key) => {
                        set_rotated.set(Some(key));
                        load();
                    }
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                },
                KeyAction::Revoke => match api::revoke_key(&who.get_value(), id).await {
                    Ok(()) => load(),
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                },
            }
            set_busy.set(false);
        });
    });

    // Asked for twice, because it is every client at once and the list it empties
    // is the only place their names were.
    let revoke_all = move |_| {
        set_busy.set(true);
        set_cutting.set(false);
        set_failure.set(None);

        spawn_local(async move {
            match api::revoke_keys(&who.get_value()).await {
                Ok(_) => load(),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    };

    // Only worth offering for more than one. With a single key it is the same
    // action as the one already in its row, under a name that sounds bigger.
    let several = move || keys.get().is_some_and(|list| list.len() > 1);

    view! {
        <section class="pane wide">
            <div class="pane-head">
                <div>
                    <h2>{t!("keys.heading")}</h2>
                    <p class="hint quiet">{t!("keys.lead")}</p>
                </div>
                <span class="acts">
                    <Show when=move || several() && !cutting.get()>
                        <button
                            class="second danger"
                            disabled=busy
                            on:click=move |_| set_cutting.set(true)
                        >
                            {t!("accounts.revoke_all")}
                        </button>
                    </Show>
                    <Show when=move || cutting.get()>
                        <span class="confirm">
                            <span>{t!("keys.revoke_sure")}</span>
                            <button class="danger" disabled=busy on:click=revoke_all>
                                {t!("keys.revoke")}
                            </button>
                            <button
                                class="second"
                                disabled=busy
                                on:click=move |_| set_cutting.set(false)
                            >
                                {t!("common.cancel")}
                            </button>
                        </span>
                    </Show>
                    <button on:click=move |_| set_asking.set(true)>
                        <Glyph icon=Icon::Add />
                        {t!("keys.issue")}
                    </button>
                </span>
            </div>

            {move || match keys.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(list) if list.is_empty() => {
                    view! { <p class="quiet">{t!("keys.none")}</p> }.into_any()
                }
                Some(list) => {
                    view! {
                        <div class="scrolls">
                            <table class="listing">
                                <thead>
                                    <tr>
                                        <th>{t!("keys.label")}</th>
                                        <th>{t!("keys.until")}</th>
                                        <th>{t!("keys.last_use")}</th>
                                        <th class="figure">{t!("keys.actions")}</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {list
                                        .into_iter()
                                        .map(|key| view! { <KeyRow key act busy /> })
                                        .collect_view()}
                                </tbody>
                            </table>
                        </div>
                    }
                        .into_any()
                }
            }}

            {move || {
                failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
            }}

            <NewKeySheet
                who=who.get_value()
                asking
                set_asking
                rotated
                set_rotated
                on_made=Callback::new(move |()| load())
                on_expired
            />
        </section>
    }
}

/// Asks for a key, makes it, shows it, and forgets it when it closes.
///
/// Also where a rotated key appears, since a rotated key is the same thing: a
/// secret with one moment to be read.
#[component]
fn NewKeySheet(
    who: String,
    asking: ReadSignal<bool>,
    set_asking: WriteSignal<bool>,
    rotated: ReadSignal<Option<tocata::types::IssuedKey>>,
    set_rotated: WriteSignal<Option<tocata::types::IssuedKey>>,
    on_made: Callback<()>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    let (label, set_label) = signal(String::new());
    let (expires, set_expires) = signal(String::new());
    let (made, set_made) = signal(Option::<tocata::types::IssuedKey>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (waiting, set_waiting) = signal(false);

    let who = StoredValue::new(who);

    // Open for either reason: somebody asked to make one, or one was rotated and
    // its new secret has nowhere else to be shown.
    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        if asking.get() {
            set_label.set(String::new());
            set_expires.set(String::new());
            set_made.set(None);
            set_failure.set(None);
            let _ = element.show_modal();
        } else if rotated.get().is_some() {
            set_failure.set(None);
            let _ = element.show_modal();
        } else {
            element.close();
        }
    });

    // Whichever of the two has a secret to show. Nothing outlives the dialogue:
    // closing it clears both.
    let secret = move || made.get().or_else(|| rotated.get());

    let shut = move || {
        set_asking.set(false);
        set_rotated.set(None);
        set_made.set(None);
    };

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        set_waiting.set(true);
        set_failure.set(None);

        let asked = tocata::types::NewKey {
            label: {
                let written = label.get().trim().to_string();
                (!written.is_empty()).then_some(written)
            },
            // The input gives a day; the server wants a moment, so it is the end
            // of that day.
            expires_at: {
                let day = expires.get();
                (!day.is_empty()).then(|| format!("{day}T23:59:59Z"))
            },
        };

        spawn_local(async move {
            match api::issue_key(&who.get_value(), asked).await {
                Ok(key) => {
                    set_made.set(Some(key));
                    on_made.run(());
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_waiting.set(false);
        });
    };

    view! {
        <dialog node_ref=dialog class="sheet" on:close=move |_| shut()>
            <header class="sheet-head">
                <h2>
                    {move || {
                        if secret().is_some() { t!("keys.made") } else { t!("keys.issue") }
                    }}
                </h2>
                <button
                    type="button"
                    class="close"
                    title=t!("common.close")
                    on:click=move |_| shut()
                >
                    <Glyph icon=Icon::Close />
                </button>
            </header>

            <Show
                when=move || secret().is_some()
                fallback=move || {
                    view! {
                        <form on:submit=submit>
                            <div class="field">
                                <label for="key-label">{t!("keys.label")}</label>
                                <input
                                    id="key-label"
                                    placeholder=t!("keys.label_default")
                                    prop:value=label
                                    on:input:target=move |e| set_label.set(e.target().value())
                                />
                            </div>

                            <div class="field">
                                <label for="key-expires">{t!("keys.until")}</label>
                                // Dimmed while empty, because a date field shows
                                // dd/mm/yyyy whether or not anything has been typed,
                                // and in the ink of real text it reads as a value
                                // already chosen.
                                <input
                                    id="key-expires"
                                    type="date"
                                    class:empty=move || expires.get().is_empty()
                                    prop:value=expires
                                    on:input:target=move |e| set_expires.set(e.target().value())
                                />
                                <span class="hint quiet">{t!("keys.until_note")}</span>
                            </div>

                            <p class="row ends">
                                <button
                                    type="button"
                                    class="second"
                                    disabled=waiting
                                    on:click=move |_| shut()
                                >
                                    {t!("common.cancel")}
                                </button>
                                <button type="submit" disabled=waiting>
                                    {move || {
                                        if waiting.get() {
                                            t!("login.working")
                                        } else {
                                            t!("keys.issue")
                                        }
                                    }}
                                </button>
                            </p>

                            {move || {
                                failure
                                    .get()
                                    .map(|why| view! { <p class="failure" role="alert">{why}</p> })
                            }}
                        </form>
                    }
                }
            >
                <p>{t!("keys.once")}</p>
                <div class="issued">
                    <code>{move || secret().map(|key| key.key).unwrap_or_default()}</code>
                </div>
                <p class="row ends">
                    <button on:click=move |_| shut()>{t!("common.done")}</button>
                </p>
            </Show>
        </dialog>
    }
}

/// What the menu at the end of a key's row offers.
#[derive(Clone, Copy)]
enum KeyAction {
    Rotate,
    Revoke,
}

/// One key as a row, with its actions behind the dots.
#[component]
fn KeyRow(
    key: tocata::types::Key,
    act: Callback<(KeyAction, i64)>,
    busy: ReadSignal<bool>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    // Where the menu goes, in viewport coordinates.
    let (at, set_at) = signal((0.0, 0.0));

    let id = key.id;
    let expired = key.expired;

    // Fixed to the viewport rather than positioned inside the cell, because the
    // box around the table scrolls sideways and anything that overflows a
    // scrolling box is clipped by it. Fixed escapes that; the price is working out
    // where to put it, which is one rectangle.
    let toggle = move |event: web_sys::MouseEvent| {
        if let Some(button) = event
            .current_target()
            .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let rect = button.get_bounding_client_rect();
            set_at.set((rect.bottom() + 4.0, rect.right()));
        }

        set_open.update(|shown| *shown = !*shown);
    };

    view! {
        <tr class:off=move || expired>
            <td>{key.label}</td>
            <td class="quiet">
                {key
                    .expires_at
                    .as_deref()
                    .map(|at| {
                        if expired {
                            t!("keys.expired", when = when(at)).to_string()
                        } else {
                            when(at)
                        }
                    })
                    .unwrap_or_else(|| t!("keys.forever").to_string())}
            </td>
            <td class="quiet">
                {key
                    .last_used_at
                    .as_deref()
                    .map(when)
                    .unwrap_or_else(|| t!("keys.unused").to_string())}
            </td>
            <td class="figure">
                <button
                    class="dots"
                    title=t!("keys.actions")
                    disabled=busy
                    aria-expanded=move || open.get().to_string()
                    on:click=toggle
                >
                    <Glyph icon=Icon::More />
                </button>

                <Show when=move || open.get()>
                    <div class="veil" on:click=move |_| set_open.set(false)></div>
                    <div
                        class="menu afloat"
                        style=move || {
                            let (top, right) = at.get();
                            format!("top: {top}px; right: calc(100vw - {right}px)")
                        }
                    >
                        <button
                            class="menu-item"
                            on:click=move |_| {
                                set_open.set(false);
                                act.run((KeyAction::Rotate, id));
                            }
                        >
                            <Glyph icon=Icon::Rotate />
                            {t!("keys.rotate")}
                        </button>
                        <button
                            class="menu-item"
                            on:click=move |_| {
                                set_open.set(false);
                                act.run((KeyAction::Revoke, id));
                            }
                        >
                            <Glyph icon=Icon::Remove />
                            {t!("keys.revoke")}
                        </button>
                    </div>
                </Show>
            </td>
        </tr>
    }
}

/// Your own panel sessions, and closing them.
///
/// The same table as the keys above, because the values are the same shape: two
/// dates and one thing you can do about them.
#[component]
fn MySessions(username: String, on_expired: Callback<()>) -> impl IntoView {
    let (sessions, set_sessions) = signal(Option::<Vec<tocata::types::Login>>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);

    let who = StoredValue::new(username);

    let load = move || {
        spawn_local(async move {
            match api::sessions(&who.get_value()).await {
                Ok(list) => set_sessions.set(Some(list)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });
    };

    load();

    // Closing this one is logging out, so it ends the same way: back at the form.
    // Reading the list again would ask with a session that no longer exists.
    let close = move |(id, current): (i64, bool)| {
        set_busy.set(true);
        spawn_local(async move {
            match api::close_session(&who.get_value(), id).await {
                Ok(()) if current => on_expired.run(()),
                Ok(()) => load(),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    };

    // Every one of them, this one included, which is why it needs no confirming
    // beyond what it says: it is the log out button with a wider reach, and the
    // worst it can do is make somebody log in again.
    let close_all = move |_| {
        set_busy.set(true);
        set_failure.set(None);

        spawn_local(async move {
            match api::close_sessions(&who.get_value()).await {
                Ok(_) => on_expired.run(()),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => {
                    set_failure.set(Some(said(&why)));
                    set_busy.set(false);
                }
            }
        });
    };

    let several = move || sessions.get().is_some_and(|list| list.len() > 1);

    view! {
        <section class="pane wide">
            <div class="pane-head">
                <div>
                    <h2>{t!("sessions.heading")}</h2>
                    <p class="hint quiet">{t!("sessions.lead")}</p>
                </div>
                <Show when=several>
                    <button class="second" disabled=busy on:click=close_all>
                        {t!("accounts.close_mine")}
                    </button>
                </Show>
            </div>

            {move || match sessions.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(list) => {
                    view! {
                        <div class="scrolls">
                            <table class="listing">
                                <thead>
                                    <tr>
                                        <th>{t!("sessions.last_seen")}</th>
                                        <th>{t!("sessions.expires")}</th>
                                        <th class="figure">{t!("keys.actions")}</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {list
                                        .into_iter()
                                        .map(|login| {
                                            let id = login.id;
                                            let current = login.current;
                                            view! {
                                                <tr>
                                                    <td>
                                                        {when(&login.last_seen_at)}
                                                        {if current {
                                                            view! {
                                                                <span class="badge">
                                                                    {t!("sessions.this_one")}
                                                                </span>
                                                            }
                                                                .into_any()
                                                        } else {
                                                            ().into_any()
                                                        }}
                                                    </td>
                                                    <td class="quiet">{when(&login.expires_at)}</td>
                                                    <td class="figure">
                                                        <button
                                                            class="second small"
                                                            disabled=busy
                                                            on:click=move |_| close((
                                                                id,
                                                                current,
                                                            ))
                                                        >
                                                            {if current {
                                                                t!("sessions.log_out")
                                                            } else {
                                                                t!("sessions.close")
                                                            }}
                                                        </button>
                                                    </td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                        </div>
                    }
                        .into_any()
                }
            }}

            {move || {
                failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
            }}
        </section>
    }
}
