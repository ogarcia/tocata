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
                                    <th>{t!("accounts.reach")}</th>
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
            <td class="quiet">
                {if account.libraries.is_empty() {
                    t!("accounts.all_libraries").to_string()
                } else {
                    t!("accounts.some", count = account.libraries.len()).to_string()
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
                <label for="username">{t!("accounts.username")}</label>
                <input
                    id="username"
                    autocomplete="off"
                    required
                    prop:value=username
                    on:input:target=move |e| set_username.set(e.target().value())
                />

                <label for="new-password">{t!("accounts.password")}</label>
                <input
                    id="new-password"
                    type="password"
                    autocomplete="new-password"
                    required
                    prop:value=password
                    on:input:target=move |e| set_password.set(e.target().value())
                />

                <label for="email">{t!("accounts.email")}</label>
                <input
                    id="email"
                    type="email"
                    autocomplete="off"
                    prop:value=email
                    on:input:target=move |e| set_email.set(e.target().value())
                />

                <label class="checkbox">
                    <input
                        type="checkbox"
                        prop:checked=admin
                        on:change:target=move |e| set_admin.set(e.target().checked())
                    />
                    {t!("accounts.is_admin")}
                </label>

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
                    // Held rather than cloned into each closure: three of the
                    // sections below capture by move, and a String cannot be
                    // moved three times.
                    let name = StoredValue::new(account.username.clone());
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
                            <Access account=account.clone() mine cut />
                        </div>

                        <Show when=move || admin>
                            <Reach account=account.clone() libraries restrict />
                        </Show>

                        // The full management of somebody's own credentials, and
                        // only their own: an administrator gets the two blunt
                        // instruments above and no view of this.
                        <Show when=move || mine>
                            <MyKeys username=name.get_value() on_expired />
                            <MySessions username=name.get_value() on_expired />
                        </Show>

                        // Deleting is administration, and never your own account:
                        // the server refuses that, and what it is protecting is a
                        // server that still has an administrator.
                        <Show when=move || admin && !mine>
                            <Danger username=name.get_value() on_expired />
                        </Show>
                    }
                        .into_any()
                }
            }
        }}

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
                <label for="name">{t!("accounts.username")}</label>
                <input
                    id="name"
                    required
                    prop:value=username
                    on:input:target=move |e| set_username.set(e.target().value())
                />

                <label for="mail">{t!("accounts.email")}</label>
                <input
                    id="mail"
                    type="email"
                    prop:value=email
                    on:input:target=move |e| set_email.set(e.target().value())
                />

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

                <label class="checkbox">
                    <input
                        type="checkbox"
                        prop:checked=scrobbling
                        on:change:target=move |e| set_scrobbling.set(e.target().checked())
                    />
                    {t!("accounts.scrobbling")}
                </label>

                // Offered only to an administrator, and never on their own
                // account: the server refuses that, and the reason it refuses is
                // that it is what keeps a server administrable.
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
                <Show when=move || admin && mine>
                    <span class="hint quiet">{t!("accounts.not_yourself")}</span>
                </Show>

                <p class="row ends">
                    <button type="submit">{t!("common.save")}</button>
                </p>
            </form>
        </section>
    }
}

/// What is open, and how to close it.
#[component]
fn Access(account: Account, mine: bool, cut: Callback<Cut>) -> impl IntoView {
    let sessions = account.sessions;
    let keys = account.keys;

    view! {
        <section class="pane">
            <h2>{t!("accounts.access")}</h2>

            <div class="lines">
                <div class="line">
                    <span>{t!("accounts.sessions", count = sessions)}</span>
                    <Show when=move || { sessions > 0 }>
                        <button class="second small" on:click=move |_| cut.run(Cut::Sessions)>
                            {if mine { t!("accounts.close_mine") } else { t!("accounts.close_all") }}
                        </button>
                    </Show>
                </div>

                <div class="line">
                    <span>{t!("accounts.keys", count = keys)}</span>
                    <Show when=move || { keys > 0 }>
                        <button class="second small" on:click=move |_| cut.run(Cut::Keys)>
                            {t!("accounts.revoke_all")}
                        </button>
                    </Show>
                </div>
            </div>

            <p class="hint quiet">
                {if mine { t!("accounts.mine_note") } else { t!("accounts.cut_note") }}
            </p>
        </section>
    }
}

/// Which libraries the account may see.
///
/// Nothing ticked means no restriction, which is not the same as seeing nothing:
/// an account with no restriction sees every library that is switched on. Saying
/// that in words matters, because an empty set of boxes looks like "none".
#[component]
fn Reach(
    account: Account,
    libraries: ReadSignal<Vec<Library>>,
    restrict: Callback<Vec<i64>>,
) -> impl IntoView {
    let allowed = RwSignal::new(account.libraries.clone());

    view! {
        <section class="pane wide">
            <h2>{t!("accounts.reach")}</h2>
            <p class="hint quiet">
                {move || {
                    if allowed.get().is_empty() {
                        t!("accounts.reach_all")
                    } else {
                        t!("accounts.reach_some")
                    }
                }}
            </p>

            <div class="lines">
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
                                            // Ticking from "all" means naming the
                                            // rest: an empty list is not a subset
                                            // to add to, it is the absence of a
                                            // restriction.
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
                                            // Every one ticked is the same as no
                                            // restriction, and saying it that way
                                            // keeps it true as libraries are added.
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
        </section>
    }
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

/// Your own keys, in full: making them, giving them a date, taking them away.
///
/// A key is readable once, when it is made. What the database keeps is a hash, so
/// this is the only moment it can be shown and the screen says so rather than
/// letting somebody close it and come back for it.
#[component]
fn MyKeys(username: String, on_expired: Callback<()>) -> impl IntoView {
    let (keys, set_keys) = signal(Option::<Vec<tocata::types::Key>>::None);
    let (issued, set_issued) = signal(Option::<tocata::types::IssuedKey>::None);
    let (label, set_label) = signal(String::new());
    let (expires, set_expires) = signal(String::new());
    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);

    let who = StoredValue::new(username);

    let reload = move || {
        spawn_local(async move {
            match api::keys(&who.get_value()).await {
                Ok(list) => set_keys.set(Some(list)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });
    };

    reload();

    let issue = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        set_busy.set(true);
        set_failure.set(None);
        set_issued.set(None);

        let asked = tocata::types::NewKey {
            label: {
                let written = label.get().trim().to_string();
                (!written.is_empty()).then_some(written)
            },
            // A date, or nothing at all. The input gives a day; the server wants a
            // moment, so it is the end of that day.
            expires_at: {
                let day = expires.get();
                (!day.is_empty()).then(|| format!("{day}T23:59:59Z"))
            },
        };

        spawn_local(async move {
            match api::issue_key(&who.get_value(), asked).await {
                Ok(key) => {
                    set_issued.set(Some(key));
                    set_label.set(String::new());
                    set_expires.set(String::new());
                    reload();
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    };

    let revoke = move |id: i64| {
        set_busy.set(true);
        spawn_local(async move {
            match api::revoke_key(&who.get_value(), id).await {
                Ok(()) => reload(),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    };

    view! {
        <section class="pane wide">
            <h2>{t!("keys.heading")}</h2>
            <p class="hint quiet">{t!("keys.lead")}</p>

            {move || match keys.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(list) if list.is_empty() => {
                    view! { <p class="quiet">{t!("keys.none")}</p> }.into_any()
                }
                Some(list) => {
                    view! {
                        <div class="lines">
                            {list
                                .into_iter()
                                .map(|key| {
                                    let id = key.id;
                                    view! {
                                        <div class="line" class:off=key.expired>
                                            <span>
                                                <strong>{key.label}</strong>
                                                <span class="quiet detail">
                                                    {key
                                                        .expires_at
                                                        .as_deref()
                                                        .map(|at| {
                                                            if key.expired {
                                                                t!("keys.expired", when = when(at))
                                                                    .to_string()
                                                            } else {
                                                                t!("keys.expires", when = when(at))
                                                                    .to_string()
                                                            }
                                                        })
                                                        .unwrap_or_else(|| {
                                                            t!("keys.forever").to_string()
                                                        })}
                                                    {key
                                                        .last_used_at
                                                        .as_deref()
                                                        .map(|at| {
                                                            format!(
                                                                " · {}",
                                                                t!("keys.used", when = when(at)),
                                                            )
                                                        })
                                                        .unwrap_or_else(|| {
                                                            format!(" · {}", t!("keys.unused"))
                                                        })}
                                                </span>
                                            </span>
                                            <button
                                                class="second small danger"
                                                disabled=busy
                                                on:click=move |_| revoke(id)
                                            >
                                                {t!("keys.revoke")}
                                            </button>
                                        </div>
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                        .into_any()
                }
            }}

            // Shown once and never again, which is the whole of what there is to
            // say about it.
            {move || {
                issued
                    .get()
                    .map(|key| {
                        view! {
                            <div class="issued">
                                <p>{t!("keys.once")}</p>
                                <code>{key.key}</code>
                            </div>
                        }
                    })
            }}

            <form class="stacked" on:submit=issue>
                <label for="key-label">{t!("keys.label")}</label>
                <input
                    id="key-label"
                    placeholder=t!("keys.label_default")
                    prop:value=label
                    on:input:target=move |e| set_label.set(e.target().value())
                />

                <label for="key-expires">{t!("keys.until")}</label>
                <input
                    id="key-expires"
                    type="date"
                    prop:value=expires
                    on:input:target=move |e| set_expires.set(e.target().value())
                />
                <span class="hint quiet">{t!("keys.until_note")}</span>

                <p class="row ends">
                    <button type="submit" disabled=busy>
                        <Glyph icon=Icon::Add />
                        {t!("keys.issue")}
                    </button>
                </p>
            </form>

            {move || {
                failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
            }}
        </section>
    }
}

/// Your own panel sessions, and closing them.
#[component]
fn MySessions(username: String, on_expired: Callback<()>) -> impl IntoView {
    let (sessions, set_sessions) = signal(Option::<Vec<tocata::types::Login>>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);

    let who = StoredValue::new(username);

    let reload = move || {
        spawn_local(async move {
            match api::sessions(&who.get_value()).await {
                Ok(list) => set_sessions.set(Some(list)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });
    };

    reload();

    let close = move |id: i64| {
        set_busy.set(true);
        spawn_local(async move {
            match api::close_session(&who.get_value(), id).await {
                Ok(()) => reload(),
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
                        <div class="lines">
                            {list
                                .into_iter()
                                .map(|login| {
                                    let id = login.id;
                                    let current = login.current;
                                    view! {
                                        <div class="line">
                                            <span>
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
                                                <span class="quiet detail">
                                                    {t!("sessions.until", when = when(&login.expires_at))}
                                                </span>
                                            </span>
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
                                        </div>
                                    }
                                })
                                .collect_view()}
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
