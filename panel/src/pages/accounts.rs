// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The accounts, and what administering one means.
//!
//! A list of who there is, and a screen of its own for each. The detail screen has
//! a URL because an account is worth linking to and coming back to.
//!
//! What is not here: making keys and reading sessions one at a time. Those are
//! somebody's own credentials and belong where their owner manages them. What an
//! administrator gets is the two blunt instruments — close every session, revoke
//! every key — because cutting somebody off is administration and knowing that
//! their third key is called "phone" is not.
//!
//! Two rules the server enforces and this screen therefore honours: nobody may
//! delete their own account, and nobody may take away their own administrator
//! rights. Between them they guarantee there is always somebody left who can
//! administer the server.

use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use rust_i18n::t;
use tocata::types::{Account, AccountChanges, Identity, Library, NewAccount};
use wasm_bindgen::JsCast;

#[component]
pub fn Accounts(on_expired: Callback<()>) -> impl IntoView {
    let (accounts, set_accounts) = signal(Option::<Vec<Account>>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (adding, set_adding) = signal(false);

    spawn_local(async move {
        match api::accounts().await {
            Ok(list) => set_accounts.set(Some(list)),
            Err(Failure::Unauthenticated) => on_expired.run(()),
            Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
        }
    });

    let added = Callback::new(move |account: Account| {
        set_accounts.update(|list| {
            if let Some(list) = list {
                list.push(account);
            }
        });
    });

    view! {
        <div class="titled">
            <div>
                <h1>{t!("accounts.heading")}</h1>
                <p class="quiet lead">{t!("accounts.lead")}</p>
            </div>
            <button on:click=move |_| set_adding.set(true)>
                <Glyph icon=Icon::Add />
                {t!("accounts.add")}
            </button>
        </div>

        {move || {
            failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        {move || match accounts.get() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(list) => {
                view! {
                    // Scrolls inside its own box rather than pushing the page
                    // sideways, which is what a table does on a narrow screen if
                    // nobody stops it.
                    <div class="scrolls">
                        <table class="listing">
                            <thead>
                                <tr>
                                    <th>{t!("accounts.username")}</th>
                                    <th>{t!("accounts.admin_short")}</th>
                                    <th>{t!("accounts.email")}</th>
                                    <th class="figure">{t!("accounts.sessions_short")}</th>
                                    <th class="figure">{t!("accounts.keys_short")}</th>
                                    <th class="figure">{t!("accounts.reach")}</th>
                                </tr>
                            </thead>
                            <tbody>
                                {list
                                    .into_iter()
                                    .map(|account| view! { <Row account /> })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                    .into_any()
            }
        }}

        <Adding adding set_adding on_added=added on_expired />
    }
}

/// One account as a row.
///
/// The name is the link rather than the whole row: a `tr` cannot be an anchor, and
/// the alternatives are a click handler that a keyboard cannot reach or an anchor
/// wrapping every cell. One link, in the cell that names the thing.
#[component]
fn Row(account: Account) -> impl IntoView {
    view! {
        <tr>
            <td>
                <A href=format!("/accounts/{}", account.username) attr:class="named">
                    <Glyph icon=Icon::Account />
                    {account.username.clone()}
                </A>
            </td>
            // A column of its own rather than a badge: a word repeated down a
            // column is read once, and a badge as wide as the name it sits beside
            // is read every time.
            <td>{if account.admin { t!("common.yes") } else { t!("common.no") }}</td>
            <td class="quiet ellipsis">
                {account.email.clone().unwrap_or_default()}
            </td>
            <td class="figure">{account.sessions}</td>
            <td class="figure">{account.keys}</td>
            // The word when there is no restriction, and the bare count when
            // there is: "3 libraries" in a column headed Libraries says libraries
            // twice.
            <td class="figure quiet">
                {if account.libraries.is_empty() {
                    t!("accounts.all_libraries").to_string()
                } else {
                    account.libraries.len().to_string()
                }}
            </td>
        </tr>
    }
}

/// Creating one.
#[component]
fn Adding(
    adding: ReadSignal<bool>,
    set_adding: WriteSignal<bool>,
    on_added: Callback<Account>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    let (username, set_username) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (email, set_email) = signal(String::new());
    let (admin, set_admin) = signal(false);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (waiting, set_waiting) = signal(false);

    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        if adding.get() {
            set_username.set(String::new());
            set_password.set(String::new());
            set_email.set(String::new());
            set_admin.set(false);
            set_failure.set(None);
            let _ = element.show_modal();
        } else {
            element.close();
        }
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        set_waiting.set(true);
        set_failure.set(None);

        let asked = NewAccount {
            username: username.get().trim().to_string(),
            password: password.get(),
            email: {
                let written = email.get().trim().to_string();
                (!written.is_empty()).then_some(written)
            },
            admin: admin.get(),
        };

        spawn_local(async move {
            match api::add_account(asked).await {
                Ok(account) => {
                    on_added.run(account);
                    set_adding.set(false);
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_waiting.set(false);
        });
    };

    view! {
        <dialog node_ref=dialog class="sheet" on:close=move |_| set_adding.set(false)>
            <header class="sheet-head">
                <h2>{t!("accounts.add")}</h2>
                <button
                    type="button"
                    class="close"
                    title=t!("common.close")
                    on:click=move |_| set_adding.set(false)
                >
                    <Glyph icon=Icon::Close />
                </button>
            </header>

            <form on:submit=submit>
                <div class="field">
                    <label for="username">{t!("accounts.username")}</label>
                    <input
                        id="username"
                        autocomplete="off"
                        required
                        prop:value=username
                        on:input:target=move |e| set_username.set(e.target().value())
                    />
                </div>

                <div class="field">
                    <label for="new-password">{t!("accounts.password")}</label>
                    <input
                        id="new-password"
                        type="password"
                        autocomplete="new-password"
                        required
                        prop:value=password
                        on:input:target=move |e| set_password.set(e.target().value())
                    />
                </div>

                <div class="field">
                    <label for="email">{t!("accounts.email")}</label>
                    <input
                        id="email"
                        type="email"
                        autocomplete="off"
                        prop:value=email
                        on:input:target=move |e| set_email.set(e.target().value())
                    />
                </div>

                <div class="checks">
                    <label class="checkbox">
                        <input
                            type="checkbox"
                            prop:checked=admin
                            on:change:target=move |e| set_admin.set(e.target().checked())
                        />
                        {t!("accounts.is_admin")}
                    </label>
                </div>

                <p class="row ends">
                    <button
                        type="button"
                        class="second"
                        disabled=waiting
                        on:click=move |_| set_adding.set(false)
                    >
                        {t!("common.cancel")}
                    </button>
                    <button type="submit" disabled=waiting>
                        {move || {
                            if waiting.get() { t!("login.working") } else { t!("common.save") }
                        }}
                    </button>
                </p>

                {move || {
                    failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
                }}
            </form>
        </dialog>
    }
}

/// One account, in full.
///
/// The same screen serves an administrator opening somebody else and a person
/// opening their own, because the API draws that line already: every call here is
/// "yours, or anybody's if you administer the server". What changes is what is
/// offered, not who is trusted.
#[component]
pub fn Detail(who: Identity, on_expired: Callback<()>) -> impl IntoView {
    let params = use_params_map();

    let (account, set_account) = signal(Option::<Account>::None);
    let (libraries, set_libraries) = signal(Vec::<Library>::new());
    let (failure, set_failure) = signal(Option::<String>::None);
    let (note, set_note) = signal(Option::<String>::None);

    // Whoever the URL names, or yourself when it names nobody, which is what
    // makes this screen serve /account as well.
    let named = {
        let mine = who.username.clone();
        move || {
            let asked = params.read().get("username").unwrap_or_default();
            if asked.is_empty() {
                mine.clone()
            } else {
                asked
            }
        }
    };

    let reload = {
        let named = named.clone();
        move || {
            let looking = named();
            spawn_local(async move {
                match api::account(&looking).await {
                    Ok(found) => set_account.set(Some(found)),
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
                }
            });
        }
    };

    // Handed to the sections that change what the counts count, so the figures
    // come from the server rather than from arithmetic here.
    let reloading = Callback::new({
        let reload = reload.clone();
        move |()| reload()
    });

    // Follows the URL, so going from one account to another redraws rather than
    // showing whoever was open before.
    Effect::new({
        let reload = reload.clone();
        move |_| {
            let _ = params.read();
            reload();
        }
    });

    // Only an administrator may list them, and only an administrator is offered
    // the restriction, so asking otherwise would be a call answered with 403.
    if who.admin {
        spawn_local(async move {
            if let Ok(list) = api::libraries().await {
                set_libraries.set(list);
            }
        });
    }

    let cut = {
        let named = named.clone();
        let reload = reload.clone();
        Callback::new(move |what: Cut| {
            let looking = named();
            let reload = reload.clone();
            set_failure.set(None);
            set_note.set(None);

            spawn_local(async move {
                let outcome = match what {
                    Cut::Sessions => api::close_sessions(&looking)
                        .await
                        .map(|done| t!("accounts.closed", count = done.closed).to_string()),
                    Cut::Keys => api::revoke_keys(&looking)
                        .await
                        .map(|done| t!("accounts.revoked", count = done.revoked).to_string()),
                };

                match outcome {
                    // Counted by the server, so the figures are read back rather
                    // than adjusted here.
                    Ok(said) => {
                        set_note.set(Some(said));
                        reload();
                    }
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                }
            });
        })
    };

    let save = {
        let named = named.clone();
        Callback::new(move |changes: AccountChanges| {
            let looking = named();
            set_failure.set(None);
            set_note.set(None);

            spawn_local(async move {
                match api::change_account(&looking, changes).await {
                    Ok(fresh) => {
                        set_note.set(Some(t!("common.saved").to_string()));
                        set_account.set(Some(fresh));
                    }
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                }
            });
        })
    };

    let restrict = {
        let named = named.clone();
        Callback::new(move |chosen: Vec<i64>| {
            let looking = named();
            set_failure.set(None);

            spawn_local(async move {
                match api::restrict(&looking, chosen).await {
                    Ok(fresh) => set_account.set(Some(fresh)),
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                }
            });
        })
    };

    let admin = who.admin;
    let me = who.username.clone();

    // Known without the account: the URL says who is being looked at and the
    // session says who is looking. Held rather than cloned into each closure,
    // since several of them read it.
    let looking_at = StoredValue::new({
        let named = named.clone();
        Callback::new(move |()| named())
    });
    let me_held = StoredValue::new(me.clone());
    let mine_now = move || looking_at.get_value().run(()) == me_held.get_value();

    view! {
        <Show when=move || admin>
            <p class="back">
                <A href="/accounts">{t!("accounts.all")}</A>
            </p>
        </Show>

        {move || {
            let me = me.clone();
            match account.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(account) => {
                    let mine = account.username == me;
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

                        <div class="panes">
                            <Who account=account.clone() mine admin save />
                            <Access account mine admin cut libraries restrict />
                        </div>
                    }
                        .into_any()
                }
            }
        }}

        // Outside the block that watches the account, and deliberately: these
        // need a username and nothing else, and inside it they were rebuilt every
        // time the account was read again — which, since they ask for the account
        // to be read again when they change something, was a loop. It also took
        // the dialogue holding a new key down with it.
        <Show when=move || mine_now()>
            <MyKeys
                username=looking_at.get_value().run(())
                on_changed=reloading
                on_expired
            />
            <MySessions
                username=looking_at.get_value().run(())
                on_changed=reloading
                on_expired
            />
        </Show>

        // Deleting is administration, and never your own account: the server
        // refuses that, and what it is protecting is a server that still has an
        // administrator.
        <Show when=move || admin && !mine_now()>
            <Danger username=looking_at.get_value().run(()) on_expired />
        </Show>

        {move || note.get().map(|said| view! { <p class="note">{said}</p> })}
        {move || {
            failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}
    }
}

/// Which of the two blunt instruments.
#[derive(Clone, Copy)]
enum Cut {
    Sessions,
    Keys,
}

/// Who they are: what can be changed about the account itself.
#[component]
fn Who(account: Account, mine: bool, admin: bool, save: Callback<AccountChanges>) -> impl IntoView {
    let (username, set_username) = signal(account.username.clone());
    let (email, set_email) = signal(account.email.clone().unwrap_or_default());
    let (password, set_password) = signal(String::new());
    let (is_admin, set_is_admin) = signal(account.admin);
    let (scrobbling, set_scrobbling) = signal(account.scrobbling);

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();

        let password = password.get();

        save.run(AccountChanges {
            username: Some(username.get().trim().to_string()),
            email: Some(email.get().trim().to_string()),
            password: (!password.is_empty()).then_some(password),
            // Only an administrator may set this at all, and never on themselves,
            // so anybody else sends nothing rather than sending what it already is.
            admin: (admin && !mine).then_some(is_admin.get()),
            scrobbling: Some(scrobbling.get()),
        });

        set_password.set(String::new());
    };

    view! {
        <section class="pane">
            <h2>{t!("accounts.who")}</h2>

            <form class="stacked" on:submit=submit>
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
                    <span class="hint quiet">{t!("accounts.password_note")}</span>
                </div>

                // Together, and closer to each other than to the fields above:
                // between two switches there is nothing to explain, so the space
                // that separates a label from its own note only separates these
                // from each other for no reason.
                <div class="checks">
                    <label class="checkbox">
                        <input
                            type="checkbox"
                            prop:checked=scrobbling
                            on:change:target=move |e| set_scrobbling.set(e.target().checked())
                        />
                        {t!("accounts.scrobbling")}
                    </label>

                    // Offered only to an administrator, and never on their own
                    // account: the server refuses that, and the reason it refuses
                    // is that it is what keeps a server administrable.
                    <Show when=move || admin && !mine>
                        <label class="checkbox">
                            <input
                                type="checkbox"
                                prop:checked=is_admin
                                on:change:target=move |e| set_is_admin.set(e.target().checked())
                            />
                            {t!("accounts.is_admin")}
                        </label>
                    </Show>
                </div>
                <p class="row ends">
                    <button type="submit">{t!("common.save")}</button>
                </p>
            </form>
        </section>
    }
}

/// What this account can reach, and what it is reaching with.
///
/// One pane, because they are the same question asked twice: which libraries it
/// may see, and which credentials are currently open on it. Two headings for that
/// was two names for one idea.
#[component]
fn Access(
    account: Account,
    mine: bool,
    admin: bool,
    cut: Callback<Cut>,
    libraries: ReadSignal<Vec<Library>>,
    restrict: Callback<Vec<i64>>,
) -> impl IntoView {
    let sessions = account.sessions;
    let keys = account.keys;
    let allowed = RwSignal::new(account.libraries.clone());

    view! {
        <section class="pane">
            <h2>{t!("accounts.access")}</h2>

            // Which libraries, only for an administrator: it is the one thing here
            // that is a setting rather than a fact, and only they may change it.
            <Show when=move || admin>
                <h3>{t!("accounts.reach")}</h3>
                <p class="hint quiet">
                    {move || {
                        if allowed.get().is_empty() {
                            t!("accounts.reach_all")
                        } else {
                            t!("accounts.reach_some")
                        }
                    }}
                </p>

                <div class="checks">
                    {move || {
                        libraries
                            .get()
                            .into_iter()
                            .map(|library| {
                                let id = library.id;
                                let ticked = move || {
                                    let allowed = allowed.get();
                                    allowed.is_empty() || allowed.contains(&id)
                                };

                                view! {
                                    <label class="checkbox">
                                        <input
                                            type="checkbox"
                                            prop:checked=ticked
                                            on:change:target=move |event| {
                                                // Ticking from "all" means naming
                                                // the rest: an empty list is not a
                                                // subset to add to, it is the
                                                // absence of a restriction.
                                                let mut chosen = allowed.get();
                                                if chosen.is_empty() {
                                                    chosen = libraries
                                                        .get()
                                                        .iter()
                                                        .map(|held| held.id)
                                                        .collect();
                                                }
                                                if event.target().checked() {
                                                    if !chosen.contains(&id) {
                                                        chosen.push(id);
                                                    }
                                                } else {
                                                    chosen.retain(|held| *held != id);
                                                }
                                                // Every one ticked is the same as
                                                // no restriction, and saying it
                                                // that way keeps it true as
                                                // libraries are added.
                                                if chosen.len() == libraries.get().len() {
                                                    chosen.clear();
                                                }
                                                allowed.set(chosen.clone());
                                                restrict.run(chosen);
                                            }
                                        />
                                        {library.name}
                                    </label>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </Show>

            <h3>{t!("accounts.open_now")}</h3>

            // Two columns shared by both rows rather than two rows each with its
            // own, so the buttons come out the same width without anybody naming
            // one. Whichever label is longest decides, in any language.
            //
            // Each row carries its own note. One note under both of them was read
            // as belonging to the second, which is where it sat, while it talked
            // about the first.
            <div class="paired">
                <span>
                    {t!("accounts.sessions", count = sessions)}
                    <span class="hint quiet">
                        {if mine {
                            t!("accounts.sessions_mine")
                        } else {
                            t!("accounts.sessions_note")
                        }}
                    </span>
                </span>
                <span class="acts">
                    <Show when=move || { sessions > 0 }>
                        <button class="second small" on:click=move |_| cut.run(Cut::Sessions)>
                            {if mine { t!("accounts.close_mine") } else { t!("accounts.close_all") }}
                        </button>
                    </Show>
                </span>

                <span>
                    {t!("accounts.keys", count = keys)}
                    <span class="hint quiet">{t!("accounts.keys_note")}</span>
                </span>
                <span class="acts">
                    <Show when=move || { keys > 0 }>
                        <button class="second small" on:click=move |_| cut.run(Cut::Keys)>
                            {t!("accounts.revoke_all")}
                        </button>
                    </Show>
                </span>
            </div>
        </section>
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
fn MyKeys(username: String, on_changed: Callback<()>, on_expired: Callback<()>) -> impl IntoView {
    let (keys, set_keys) = signal(Option::<Vec<tocata::types::Key>>::None);
    let (asking, set_asking) = signal(false);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);

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

    // Reads the list again and tells whoever shows the count, because the count
    // belongs to the account rather than to this section. Only for what changed
    // something: the first read has nothing to announce.
    let reload = move || {
        load();
        on_changed.run(());
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
                        reload();
                    }
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                },
                KeyAction::Revoke => match api::revoke_key(&who.get_value(), id).await {
                    Ok(()) => reload(),
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                },
            }
            set_busy.set(false);
        });
    });

    view! {
        <section class="pane wide">
            <div class="pane-head">
                <div>
                    <h2>{t!("keys.heading")}</h2>
                    <p class="hint quiet">{t!("keys.lead")}</p>
                </div>
                <button on:click=move |_| set_asking.set(true)>
                    <Glyph icon=Icon::Add />
                    {t!("keys.issue")}
                </button>
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
                on_made=Callback::new(move |()| reload())
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
fn MySessions(
    username: String,
    on_changed: Callback<()>,
    on_expired: Callback<()>,
) -> impl IntoView {
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

    let close = move |id: i64| {
        set_busy.set(true);
        spawn_local(async move {
            match api::close_session(&who.get_value(), id).await {
                Ok(()) => {
                    load();
                    // The count in the access summary is the server's, so it is
                    // asked again rather than adjusted here.
                    on_changed.run(());
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    };

    view! {
        <section class="pane wide">
            <h2>{t!("sessions.heading")}</h2>
            <p class="hint quiet">{t!("sessions.lead")}</p>

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
                                                            on:click=move |_| close(id)
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

/// Deleting an account, asked for twice.
#[component]
fn Danger(username: String, on_expired: Callback<()>) -> impl IntoView {
    let (confirming, set_confirming) = signal(false);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);

    let who = StoredValue::new(username);

    let remove = move |_| {
        set_busy.set(true);
        set_failure.set(None);

        spawn_local(async move {
            match api::remove_account(&who.get_value()).await {
                // Nothing left to show here, so it goes back to the list.
                Ok(()) => {
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href("/accounts");
                    }
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => {
                    set_failure.set(Some(said(&why)));
                    set_confirming.set(false);
                }
            }
            set_busy.set(false);
        });
    };

    view! {
        <section class="pane wide">
            <h2>{t!("accounts.remove")}</h2>
            <p class="hint quiet">{t!("accounts.remove_note")}</p>

            <p class="row">
                <Show
                    when=move || confirming.get()
                    fallback=move || {
                        view! {
                            <button
                                class="second danger"
                                disabled=busy
                                on:click=move |_| set_confirming.set(true)
                            >
                                <Glyph icon=Icon::Remove />
                                {t!("accounts.remove")}
                            </button>
                        }
                    }
                >
                    <span class="confirm">
                        <span>{t!("accounts.remove_sure")}</span>
                        <button class="danger" disabled=busy on:click=remove>
                            {t!("accounts.remove")}
                        </button>
                        <button
                            class="second"
                            disabled=busy
                            on:click=move |_| set_confirming.set(false)
                        >
                            {t!("common.cancel")}
                        </button>
                    </span>
                </Show>
            </p>

            {move || {
                failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
            }}
        </section>
    }
}

/// A timestamp the way the reader's own machine writes one.
fn when(iso: &str) -> String {
    let moment = js_sys::Date::new(&iso.into());

    if moment.get_time().is_nan() {
        return iso.to_string();
    }

    moment
        .to_locale_string(&crate::locale::current(), &js_sys::Object::new())
        .into()
}

/// What to say about a refusal. The codes are the server's own and stable, so this
/// can branch on them and say something useful in the reader's language.
fn said(why: &Failure) -> String {
    match why {
        Failure::Unreachable => t!("login.unreachable").to_string(),
        Failure::Refused(code) => match code.as_str() {
            "conflict" => t!("accounts.taken").to_string(),
            "invalidRequest" => t!("accounts.invalid").to_string(),
            "notAuthorized" => t!("accounts.not_allowed").to_string(),
            _ => t!("common.refused").to_string(),
        },
        Failure::Unauthenticated => t!("common.refused").to_string(),
    }
}
