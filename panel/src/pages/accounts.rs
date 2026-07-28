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

use super::said;
use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};
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
            <button class="pill solid" on:click=move |_| set_adding.set(true)>
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
            <form on:submit=submit>
                <div class="sheet-body">
                    <h2>{t!("accounts.add")}</h2>
                    <p class="sheet-lead">{t!("accounts.add_lead")}</p>

                    <div class="sheet-content">
                        <label>
                            <span>{t!("accounts.username")}</span>
                            <input
                                autocomplete="off"
                                autofocus
                                required
                                prop:value=username
                                on:input:target=move |e| set_username.set(e.target().value())
                            />
                        </label>

                        <label>
                            <span>{t!("accounts.password")}</span>
                            <input
                                type="password"
                                autocomplete="new-password"
                                required
                                prop:value=password
                                on:input:target=move |e| set_password.set(e.target().value())
                            />
                        </label>

                        <label>
                            <span>{t!("accounts.email")}</span>
                            <input
                                type="email"
                                autocomplete="off"
                                prop:value=email
                                on:input:target=move |e| set_email.set(e.target().value())
                            />
                        </label>

                        <label class="checkbox">
                            <input
                                type="checkbox"
                                prop:checked=admin
                                on:change:target=move |e| set_admin.set(e.target().checked())
                            />
                            {t!("accounts.is_admin")}
                        </label>
                    </div>
                </div>

                <div class="sheet-foot">
                    <button
                        type="button"
                        class="away"
                        disabled=waiting
                        on:click=move |_| set_adding.set(false)
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

/// One account under administration.
///
/// Somebody else's, always. Opening your own from the list lands on your own
/// account instead, which is not the same screen and should not be: what is
/// administration about somebody else — what they may reach, cutting them off,
/// deleting them — is either meaningless or forbidden about yourself.
#[component]
pub fn Detail(who: Identity, on_expired: Callback<()>) -> impl IntoView {
    let params = use_params_map();
    let navigate = use_navigate();

    let (account, set_account) = signal(Option::<Account>::None);
    let (libraries, set_libraries) = signal(Vec::<Library>::new());
    let (failure, set_failure) = signal(Option::<String>::None);
    let (note, set_note) = signal(Option::<String>::None);

    let named = move || params.read().get("username").unwrap_or_default();

    // Sent to your own account rather than shown a version of this one with half of
    // it missing. It is a redirect and not a branch because the URL should end up
    // saying where you are.
    {
        let me = who.username.clone();
        Effect::new(move |_| {
            if named() == me {
                navigate(crate::layout::MINE_PATH, Default::default());
            }
        });
    }

    let reload = {
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
    Effect::new(move |_| {
        let _ = params.read();
        reload();
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
        Callback::new(move |what: Cut| {
            let looking = named();
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

    view! {
        <p class="back">
            <A href="/accounts">{t!("accounts.all")}</A>
        </p>

        {move || {
            match account.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(account) => {
                    let username = account.username.clone();
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
                            <Who account=account.clone() admin save />
                            <Access account admin cut libraries restrict />
                        </div>

                        // Named from the account that was read rather than from the
                        // URL, so going from one account to another cannot leave
                        // this pointing at the one before: the router reuses this
                        // screen instead of building it again, and a name taken once
                        // when it was built would stay taken.
                        //
                        // Never your own. The server refuses that, and what it is
                        // protecting is a server that still has an administrator.
                        <Danger username on_expired />
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
///
/// Somebody else's, always — your own profile is its own screen, with its own rules
/// about what has to be proved. Which is why nothing here asks for a password: an
/// administrator does not have this one, and the server does not ask them for it.
#[component]
fn Who(account: Account, admin: bool, save: Callback<AccountChanges>) -> impl IntoView {
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
            // Only an administrator may set this at all, so anybody else sends
            // nothing rather than sending what it already is.
            admin: admin.then_some(is_admin.get()),
            scrobbling: Some(scrobbling.get()),
            // Nothing to prove: the server asks for the current password only when
            // the account being changed is the one asking, and this screen is never
            // that.
            current_password: None,
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

                    // Offered only to an administrator. Never on their own account
                    // either, which this screen guarantees by never being their own.
                    <Show when=move || admin>
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
                    <button type="submit" class="pill solid">{t!("common.save")}</button>
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
                    <span class="hint quiet">{t!("accounts.sessions_note")}</span>
                </span>
                <span class="acts">
                    <Show when=move || { sessions > 0 }>
                        <button class="pill risky" on:click=move |_| cut.run(Cut::Sessions)>
                            {t!("accounts.close_all")}
                        </button>
                    </Show>
                </span>

                <span>
                    {t!("accounts.keys", count = keys)}
                    <span class="hint quiet">{t!("accounts.keys_note")}</span>
                </span>
                <span class="acts">
                    <Show when=move || { keys > 0 }>
                        <button class="pill risky" on:click=move |_| cut.run(Cut::Keys)>
                            {t!("accounts.revoke_all")}
                        </button>
                    </Show>
                </span>
            </div>
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
                                class="pill risky"
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
                        <button class="pill solid undoing" disabled=busy on:click=remove>
                            {t!("accounts.remove")}
                        </button>
                        <button
                            class="link"
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
