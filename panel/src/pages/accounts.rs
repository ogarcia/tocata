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

use super::{Dots, said, since};
use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};
use rust_i18n::t;
use std::cmp::Ordering;
use tocata::types::{Account, AccountChanges, Identity, Library, NewAccount};

/// Who there is, one row each.
///
/// Two kinds of row, and what tells them apart is whose account it is. Your own
/// name goes to your own account — the profile and the access screens, where
/// somebody looks after themselves — and every other name to the screen where an
/// account is administered. Neither is a version of the other: what administration
/// means about somebody else is either meaningless or forbidden about yourself.
///
/// Yours first and the rest by name. A list in the order the server happened to
/// return is a list where the row somebody knows is somewhere new every time, and
/// the row they know best is their own.
#[component]
pub fn Accounts(who: Identity, on_expired: Callback<()>) -> impl IntoView {
    let (accounts, set_accounts) = signal(Option::<Vec<Account>>::None);
    let (libraries, set_libraries) = signal(Option::<usize>::None);
    let (looking, set_looking) = signal(String::new());
    let (adding, set_adding) = signal(false);
    let (setting, set_setting) = signal(Option::<Account>::None);
    let (removing, set_removing) = signal(Option::<Account>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (note, set_note) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);

    let me = StoredValue::new(who.username);

    let load = move || {
        spawn_local(async move {
            match api::accounts().await {
                Ok(list) => set_accounts.set(Some(list)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });
    };

    load();

    // Only so that a restricted account can say "two of three" rather than "two".
    // What it takes from the list is its length.
    spawn_local(async move {
        if let Ok(list) = api::libraries().await {
            set_libraries.set(Some(list.len()));
        }
    });

    // One handler for everything a row's menu asks for, because they are all the
    // same shape: one call about one account, then the list read back rather than
    // figures adjusted here. Which also means the menu closes on a list that is
    // being rebuilt, so nothing has to remember to close it.
    let act = Callback::new(move |(deed, name): (Deed, String)| {
        set_busy.set(true);
        set_failure.set(None);
        set_note.set(None);

        spawn_local(async move {
            let shown = name.clone();

            let outcome = match deed {
                Deed::Role(admin) => api::change_account(
                    &name,
                    AccountChanges {
                        admin: Some(admin),
                        ..Default::default()
                    },
                )
                .await
                .map(|fresh| {
                    if fresh.admin {
                        t!("accounts.now_admin", name = shown).to_string()
                    } else {
                        t!("accounts.now_listener", name = shown).to_string()
                    }
                }),
                Deed::Password(password) => api::change_account(
                    &name,
                    AccountChanges {
                        password: Some(password),
                        ..Default::default()
                    },
                )
                .await
                .map(|_| t!("accounts.password_done", name = shown).to_string()),
                Deed::CloseSessions => api::close_sessions(&name)
                    .await
                    .map(|done| t!("accounts.closed", count = done.closed).to_string()),
                Deed::RevokeKeys => api::revoke_keys(&name)
                    .await
                    .map(|done| t!("accounts.revoked", count = done.revoked).to_string()),
                Deed::Delete => api::remove_account(&name)
                    .await
                    .map(|()| t!("accounts.removed", name = shown).to_string()),
            };

            match outcome {
                Ok(said) => {
                    set_note.set(Some(said));
                    load();
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }

            set_busy.set(false);
        });
    });

    // Which rows are on screen and in what order, in one closure: both answer the
    // same question, and the answer changes as somebody types.
    let shown = move || {
        let mut list = accounts.get()?;
        let needle = looking.get().trim().to_lowercase();

        if !needle.is_empty() {
            list.retain(|account| {
                account.username.to_lowercase().contains(&needle)
                    || account
                        .email
                        .as_deref()
                        .is_some_and(|mail| mail.to_lowercase().contains(&needle))
            });
        }

        let mine = me.get_value();

        list.sort_by(|one, other| {
            (one.username != mine)
                .cmp(&(other.username != mine))
                .then_with(|| by_name(&one.username, &other.username))
        });

        Some(list)
    };

    // Counted off the whole list rather than off what the search left, so it stays
    // still while somebody types and says what the server holds.
    //
    // Both halves are written out twice because rust-i18n interpolates and does not
    // pluralise, so the singular is a key of its own — and a fresh server has exactly
    // one account, which makes "1 cuentas" the first thing anybody would read. Every
    // name is a literal here on purpose: a key handed to a helper as a string is a key
    // nothing checks.
    let tally = move || {
        let list = accounts.get()?;
        let administer = list.iter().filter(|account| account.admin).count();

        let held = if list.len() == 1 {
            t!("accounts.one_account").to_string()
        } else {
            t!("accounts.many_accounts", count = list.len()).to_string()
        };

        let admins = if administer == 1 {
            t!("accounts.one_admin").to_string()
        } else {
            t!("accounts.many_admins", count = administer).to_string()
        };

        Some(format!("{held} · {admins}"))
    };

    view! {
        <header class="titled">
            <div>
                <h1>{t!("accounts.heading")}</h1>
                <p class="quiet lead">{t!("accounts.lead")}</p>
            </div>

            // The search and the one action, together at the other end of the line.
            // Both are about the list as a whole, which is what the title is too.
            <div class="finding">
                <label class="search">
                    <Glyph icon=Icon::Search />
                    <input
                        type="search"
                        placeholder=t!("accounts.search")
                        prop:value=looking
                        on:input:target=move |e| set_looking.set(e.target().value())
                    />
                </label>
                <button class="pill solid" on:click=move |_| set_adding.set(true)>
                    <Glyph icon=Icon::Add />
                    {t!("accounts.add")}
                </button>
            </div>
        </header>

        {move || note.get().map(|said| view! { <p class="note">{said}</p> })}
        {move || {
            failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}

        {move || match shown() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(list) if list.is_empty() => {
                view! { <p class="nothing">{t!("accounts.none_found")}</p> }.into_any()
            }
            Some(list) => {
                let mine = me.get_value();
                view! {
                    // Scrolls inside its own box rather than pushing the page
                    // sideways. The bleed that lets a row's tint run past the text
                    // goes on the scrolling box itself: inside it, the left twelve
                    // pixels would be unreachable and the right twelve would widen
                    // the content and ask for a scrollbar of their own.
                    <div class="scrolls bled">
                        <div class="roster-head">
                            <span>{t!("accounts.who")}</span>
                            <span>{t!("accounts.role")}</span>
                            <span class="figure">{t!("accounts.sessions_keys")}</span>
                            <span class="figure">{t!("accounts.last_seen")}</span>
                            <span></span>
                        </div>

                        <ul class="roster">
                            {list
                                .into_iter()
                                .map(|account| {
                                    let mine = account.username == mine;
                                    view! {
                                        <Row
                                            account
                                            mine
                                            libraries
                                            act
                                            busy
                                            on_password=Callback::new(move |whose| {
                                                set_setting.set(Some(whose))
                                            })
                                            on_remove=Callback::new(move |whose| {
                                                set_removing.set(Some(whose))
                                            })
                                        />
                                    }
                                })
                                .collect_view()}
                        </ul>
                    </div>

                    <p class="tally">
                        {move || tally().unwrap_or_default()} ". " {t!("accounts.name_opens")}
                    </p>
                }
                    .into_any()
            }
        }}

        <Adding adding set_adding on_added=Callback::new(move |_| load()) on_expired />
        <NewPassword account=setting set_account=set_setting act />
        <Removing account=removing set_account=set_removing act />
    }
}

/// What a row's menu can ask for. One account each, and the list read back after.
#[derive(Clone)]
enum Deed {
    /// Made an administrator, or made a listener again.
    Role(bool),
    /// A password they are not told. Telling them is the administrator's job.
    Password(String),
    CloseSessions,
    RevokeKeys,
    Delete,
}

/// Names in the order the reader's own machine would put them, which is not the
/// order their bytes come in: sorted by codepoint, "Álvaro" lands after "Zoe" and
/// "ñ" after "z".
fn by_name(one: &str, other: &str) -> Ordering {
    let locales = js_sys::Array::of1(&crate::locale::current().into());

    js_sys::JsString::from(one)
        .locale_compare(other, &locales, &js_sys::Object::new())
        .cmp(&0)
}

/// One account as a row: who they are, what they are, what is open on the account,
/// and a menu of what can be done about it.
///
/// The name is the link rather than the whole row, and where it goes is the one
/// difference between your row and everybody else's. A row cannot be an anchor
/// without either wrapping every cell in one or handing a keyboard nothing to land
/// on, and the cell that names the thing is the honest place for the link anyway.
#[component]
fn Row(
    account: Account,
    /// Whether this is the reader's own, which decides where the name goes and what
    /// the menu offers.
    mine: bool,
    /// How many libraries there are, once that is known.
    libraries: ReadSignal<Option<usize>>,
    act: Callback<(Deed, String)>,
    busy: ReadSignal<bool>,
    on_password: Callback<Account>,
    on_remove: Callback<Account>,
) -> impl IntoView {
    let name = StoredValue::new(account.username.clone());
    let admin = account.admin;
    let sessions = account.sessions;
    let keys = account.keys;
    let restricted = account.libraries.len();
    let last_seen = account.last_seen_at.clone();

    let initial = account
        .username
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_default();

    let mail = StoredValue::new(
        account
            .email
            .clone()
            .unwrap_or_else(|| t!("accounts.no_email").to_string()),
    );

    // The address and what the account may reach, on one line under the name. The
    // second half waits for the count of libraries rather than saying "2" and
    // becoming "2 of 3" a moment later.
    let under = move || match (restricted, libraries.get()) {
        (0, _) => format!("{} · {}", mail.get_value(), t!("accounts.every_library")),
        (some, Some(all)) => format!(
            "{} · {}",
            mail.get_value(),
            t!("accounts.some_libraries", some = some, all = all)
        ),
        (_, None) => mail.get_value(),
    };

    let whose = StoredValue::new(account);

    view! {
        <li>
            <span class="account">
                // Tinted with the accent for an administrator and with plain ink for
                // a listener, the same way the account block in the sidebar says
                // which you are.
                <span class="avatar" class:plain=!admin>
                    {initial}
                </span>

                <span class="who">
                    // On the name's own line and not at the end of the row, the way
                    // this browser is marked among the sessions: it says something
                    // about the name, and set against the row it read as a column
                    // nobody had given a heading to.
                    <span class="whose">
                        <A
                            href=if mine {
                                crate::layout::MINE_PATH.to_string()
                            } else {
                                format!("/accounts/{}", name.get_value())
                            }
                            attr:class="named"
                        >
                            {name.get_value()}
                        </A>
                        {mine.then(|| view! { <span class="badge">{t!("accounts.you")}</span> })}
                    </span>

                    <span class="quiet">{under}</span>
                </span>
            </span>

            <span class="rank" class:admin=admin>
                {if admin { t!("accounts.administrator") } else { t!("accounts.listener") }}
            </span>

            <span class="figure quiet">{format!("{sessions} · {keys}")}</span>

            // Relative, because the question is whether anybody is still using the
            // account and not on which afternoon they last did. Never used says so:
            // an account nobody has signed into is what an administrator is looking
            // for down this column.
            <span class="figure quiet">
                {match last_seen {
                    Some(at) => since(&at),
                    None => t!("accounts.never_seen").to_string(),
                }}
            </span>

            <Dots title=t!("accounts.more", name = name.get_value()).to_string() disabled=busy>
                <p class="menu-who">{name.get_value()}</p>

                <A href=if mine {
                    crate::layout::MINE_PATH.to_string()
                } else {
                    format!("/accounts/{}", name.get_value())
                }>{t!("accounts.open")}</A>

                // Your own row offers the two screens that are yours and nothing else.
                // What an administrator does to an account is not a thing anybody may
                // do to their own, and a menu that stopped to explain its own length
                // would be explaining what its length already says.
                {mine.then(|| view! { <A href="/account/access">{t!("accounts.my_access")}</A> })}

                {(!mine)
                    .then(|| {
                        view! {
                            <button
                                class="menu-item"
                                on:click=move |_| on_password.run(whose.get_value())
                            >
                                {t!("accounts.set_password")}
                            </button>

                            <button
                                class="menu-item"
                                on:click=move |_| act.run((Deed::Role(!admin), name.get_value()))
                            >
                                {if admin {
                                    t!("accounts.make_listener")
                                } else {
                                    t!("accounts.make_admin")
                                }}
                            </button>

                            // Only what there is to cut off. An item that would close
                            // no sessions is an item that does nothing, and the figure
                            // beside it says how much it would do.
                            {(sessions > 0 || keys > 0)
                                .then(|| {
                                    view! {
                                        <hr />

                                        {(sessions > 0)
                                            .then(|| {
                                                view! {
                                                    <button
                                                        class="menu-item"
                                                        on:click=move |_| {
                                                            act.run((
                                                                Deed::CloseSessions,
                                                                name.get_value(),
                                                            ))
                                                        }
                                                    >
                                                        {t!("accounts.close_theirs")}
                                                        <span class="figure">{sessions}</span>
                                                    </button>
                                                }
                                            })}

                                        {(keys > 0)
                                            .then(|| {
                                                view! {
                                                    <button
                                                        class="menu-item"
                                                        on:click=move |_| {
                                                            act.run((
                                                                Deed::RevokeKeys,
                                                                name.get_value(),
                                                            ))
                                                        }
                                                    >
                                                        {t!("accounts.revoke_theirs")}
                                                        <span class="figure">{keys}</span>
                                                    </button>
                                                }
                                            })}
                                    }
                                })}

                            <hr />

                            <button
                                class="menu-item risky"
                                on:click=move |_| on_remove.run(whose.get_value())
                            >
                                {t!("accounts.remove_menu")}
                            </button>
                        }
                    })}
            </Dots>
        </li>
    }
}

/// A new password for somebody else, set from the list.
///
/// One field, because that is the whole of it. An administrator does not confirm
/// with their own password — the server does not ask them for it on an account that
/// is not theirs — and the person whose password it is will not be told, which is
/// the sentence under the field rather than a step in it.
#[component]
fn NewPassword(
    account: ReadSignal<Option<Account>>,
    set_account: WriteSignal<Option<Account>>,
    act: Callback<(Deed, String)>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    let (password, set_password) = signal(String::new());

    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        if account.get().is_some() {
            set_password.set(String::new());
            let _ = element.show_modal();
        } else {
            element.close();
        }
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();

        let Some(whose) = account.get() else { return };

        act.run((Deed::Password(password.get()), whose.username));
        set_account.set(None);
    };

    view! {
        <dialog node_ref=dialog class="sheet" on:close=move |_| set_account.set(None)>
            // Somebody else's password, so nothing of the reader's belongs in it.
            <form autocomplete="off" on:submit=submit>
                <div class="sheet-body">
                    <h2>{t!("accounts.new_password")}</h2>
                    <p class="sheet-lead">
                        {move || {
                            account
                                .get()
                                .map(|whose| {
                                    t!("accounts.password_lead", name = whose.username).to_string()
                                })
                        }}
                    </p>

                    <div class="sheet-content">
                        <label>
                            <span>{t!("accounts.password")}</span>
                            <input
                                type="password"
                                autocomplete="new-password"
                                autofocus
                                required
                                prop:value=password
                                on:input:target=move |e| set_password.set(e.target().value())
                            />
                        </label>
                    </div>
                </div>

                <div class="sheet-foot">
                    <button
                        type="button"
                        class="away"
                        on:click=move |_| set_account.set(None)
                    >
                        {t!("common.cancel")}
                    </button>
                    <button
                        type="submit"
                        class="pill solid"
                        disabled=move || password.get().is_empty()
                    >
                        {t!("common.save")}
                    </button>
                </div>
            </form>
        </dialog>
    }
}

/// Deleting somebody's account, confirmed by writing their name.
///
/// The two figures are the part nobody remembers: a confirmation that only asks
/// whether you are sure is asking somebody to agree to something they were not told.
/// And the name has to be typed, because this is the one action here that cannot be
/// undone — a mis-aimed click on a row is exactly what it is guarding against.
#[component]
fn Removing(
    account: ReadSignal<Option<Account>>,
    set_account: WriteSignal<Option<Account>>,
    act: Callback<(Deed, String)>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    let (typed, set_typed) = signal(String::new());

    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        if account.get().is_some() {
            set_typed.set(String::new());
            let _ = element.show_modal();
        } else {
            element.close();
        }
    });

    let matches = move || {
        account
            .get()
            .is_some_and(|whose| typed.get().trim() == whose.username)
    };

    let remove = move |_| {
        let Some(whose) = account.get() else { return };

        act.run((Deed::Delete, whose.username));
        set_account.set(None);
    };

    view! {
        <dialog node_ref=dialog class="sheet" on:close=move |_| set_account.set(None)>
            <div class="sheet-body">
                <h2>
                    {move || {
                        account
                            .get()
                            .map(|whose| t!("accounts.remove_this", name = whose.username)
                                .to_string())
                    }}
                </h2>
                <p class="sheet-lead">{t!("accounts.remove_note")}</p>

                <dl class="facts">
                    <div>
                        <dt>{t!("accounts.sessions_short")}</dt>
                        <dd>{move || account.get().map(|whose| whose.sessions)}</dd>
                    </div>
                    <div>
                        <dt>{t!("accounts.keys_short")}</dt>
                        <dd>{move || account.get().map(|whose| whose.keys)}</dd>
                    </div>
                </dl>

                <div class="sheet-content">
                    <label>
                        <span>
                            {move || {
                                account
                                    .get()
                                    .map(|whose| {
                                        t!("accounts.type_name", name = whose.username).to_string()
                                    })
                            }}
                        </span>
                        <input
                            class="exact"
                            autocomplete="off"
                            prop:value=typed
                            on:input:target=move |e| set_typed.set(e.target().value())
                        />
                    </label>
                </div>
            </div>

            <div class="sheet-foot">
                <button type="button" class="away" on:click=move |_| set_account.set(None)>
                    {t!("common.keep")}
                </button>
                <button
                    type="button"
                    class="pill solid undoing"
                    disabled=move || !matches()
                    on:click=remove
                >
                    {t!("accounts.remove_yes")}
                </button>
            </div>
        </dialog>
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
            // Nothing in here is the reader's own. The whole form says so, and the
            // password field says it in the one word browsers actually act on: a
            // password to be made is not a password to be recalled, and a form with
            // no `current-password` in it is not a form to be logged in with.
            <form autocomplete="off" on:submit=submit>
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

            <form class="stacked" autocomplete="off" on:submit=submit>
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
