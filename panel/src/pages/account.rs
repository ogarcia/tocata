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
//! Which goes for the words as well, and that is the harder half to keep: these
//! screens speak out of `profile`, `access` and `prefs`, and never out of `accounts`.
//! The same field is a different sentence when it is somebody else's — "Papel: lo
//! decide otro administrador" is true of yours and false of theirs, because on their
//! screen the administrator deciding it is the person reading — so a key shared
//! between the two is a key that can only ever be right on one side of it. Where the
//! label really is the same word, it is written twice on purpose.
//!
//! An administrator opening their own account from the list lands here too, because
//! there is no version of this that is administration.

use super::{Dots, MISSING, Setting, lapse, on_day, said, since, thousands, when};
use crate::accent::{self, Accent};
use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use crate::locale;
use crate::theme::{self, Theme};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use rust_i18n::t;
use tocata::types::{Account, AccountChanges, Holdings, PreferenceChanges};

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
pub fn Profile(on_expired: Callback<()>) -> impl IntoView {
    let (account, set_account) = signal(Option::<Account>::None);
    let (held, set_held) = signal(Option::<Holdings>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (note, set_note) = signal(Option::<String>::None);

    // Read rather than copied, so that walking back onto this screen after renaming
    // yourself asks about the name you have now. It used to be taken from the identity
    // the panel was built with, which is the name you had when you logged in.
    let me = expect_context::<crate::layout::Me>();

    let load = move || {
        spawn_local(async move {
            match api::account(&me.username.get_untracked()).await {
                Ok(found) => set_account.set(Some(found)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });
    };

    load();

    // What is yours on this server, which the account itself does not carry: it is
    // counted where it is asked for, and this is the screen that asks.
    spawn_local(async move {
        if let Ok(counted) = api::holdings(&me.username.get_untracked()).await {
            set_held.set(Some(counted));
        }
    });

    // Changing your own name changes what every call about you asks the server about,
    // so who the panel thinks you are follows the answer. Without that, saving twice
    // would ask about somebody who no longer exists under that name — and so would
    // this screen the next time it was opened, and Access, and the roster.
    let save = Callback::new(move |changes: AccountChanges| {
        set_failure.set(None);
        set_note.set(None);

        spawn_local(async move {
            match api::change_account(&me.username.get_untracked(), changes).await {
                Ok(fresh) => {
                    // The greeting and the account menu say your name, and they read
                    // it from here: without this, changing it would appear to do
                    // nothing at all until a reload.
                    me.is_now(&fresh);

                    set_note.set(Some(t!("common.saved").to_string()));
                    set_account.set(Some(fresh));
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
        });
    });

    view! {
        // The screen is called what it is, and the name and the address are two of
        // the things on it. They used to be the title and the lead, which left the
        // fields below repeating them and nothing saying which screen this was.
        <header class="titled">
            <div>
                <h1>{t!("nav.profile")}</h1>
                <p class="quiet lead">{t!("profile.profile_lead")}</p>
            </div>
        </header>

        <Listened held />

        {move || match account.get() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(account) => {
                view! {
                    // Two, and neither is a column of a grid: on a wide screen the
                    // form alone left a band of nothing beside it, and the rail is
                    // what fills it with the thing somebody came here to check.
                    <div class="two">
                        // Two forms, each with its own save, so what holds them is a
                        // plain box: nesting one form in another is not a thing HTML
                        // does.
                        <div class="forms">
                            <Yourself account=account.clone() save />
                            <Listening account=account.clone() on_expired />
                        </div>

                        <Rail account on_expired />
                    </div>
                }
                    .into_any()
            }
        }}

        {move || note.get().map(|said| view! { <p class="note">{said}</p> })}
        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}
    }
}

/// What brings somebody to this screen, beside the form that changes it: where you
/// are signed in, and with what.
///
/// Informational, all of it. Nothing here is editable and nothing repeats what the
/// form already says — the counts and the dates are the parts of the account the form
/// has no field for, and the two things that can be acted on are behind one link to
/// the screen that owns them.
///
/// Three calls rather than one. The account carries the counts; the sessions say when
/// this one started; the keys say when a client last used one. None of the three is
/// worth a round trip on its own and all three are cheap, so they go out together and
/// each fills its own lines as it lands.
#[component]
fn Rail(account: Account, on_expired: Callback<()>) -> impl IntoView {
    let (started, set_started) = signal(Option::<String>::None);
    let (used, set_used) = signal(Option::<(String, String)>::None);
    let (all, set_all) = signal(Option::<usize>::None);

    let me = StoredValue::new(account.username.clone());

    spawn_local(async move {
        // Whichever of them is this one. The server marks it, because only the server
        // knows which token arrived.
        if let Ok(logins) = api::sessions(&me.get_value()).await {
            set_started.set(
                logins
                    .into_iter()
                    .find(|one| one.current)
                    .map(|one| one.created_at),
            );
        }
    });

    spawn_local(async move {
        match api::keys(&me.get_value()).await {
            // The most recent use across every key, and which key that was. A key
            // nobody has used yet has no date, so it cannot be the answer.
            Ok(keys) => {
                set_used.set(
                    keys.into_iter()
                        .filter_map(|key| key.last_used_at.map(|at| (key.label, at)))
                        .max_by(|(_, left), (_, right)| left.cmp(right)),
                );
            }
            Err(Failure::Unauthenticated) => on_expired.run(()),
            Err(_) => {}
        }
    });

    spawn_local(async move {
        // Only to say "two of three" rather than "two". Anybody with a session may
        // read the list, and what this takes from it is its length.
        if let Ok(libraries) = api::libraries().await {
            set_all.set(Some(libraries.len()));
        }
    });

    let reach = {
        let mine = account.libraries.len();

        move || match (mine, all.get()) {
            // No restriction at all is every library there is, whatever there is.
            (0, _) => t!("profile.reach_all").to_string(),
            (some, Some(all)) => t!("profile.reach_some", some = some, all = all).to_string(),
            (some, None) => some.to_string(),
        }
    };

    view! {
        <aside class="rail">
            <h2 class="part">{t!("profile.where_signed_in")}</h2>
            <dl class="facts">
                <Fact label=t!("profile.panel_sessions").to_string()>
                    {account.sessions.to_string()}
                </Fact>
                <Fact label=t!("profile.api_keys").to_string()>{account.keys.to_string()}</Fact>
                <Fact label=t!("profile.reach").to_string()>{move || reach()}</Fact>
                <Fact label=t!("profile.this_session").to_string()>
                    {move || {
                        started.get().map(|at| when(&at)).unwrap_or_else(|| MISSING.to_string())
                    }}
                </Fact>
            </dl>

            <p class="onward">
                <A href="/account/access">{t!("profile.manage_access")}</A>
            </p>

            // Two lines, not three. The third said when you signed in, which is the
            // row above it said twice — and the honest replacement, when you were
            // last here before now, is not a thing the server can answer: a session
            // is a row that expires and goes, and nothing writes down a login. It
            // would take a column on the account to say it, so until there is one
            // this heading covers what it can.
            <h2 class="part">{t!("profile.lately")}</h2>
            <ul class="facts">
                <Lately label=Signal::derive(move || {
                    match used.get() {
                        Some((label, _)) => t!("profile.key_used", name = label).to_string(),
                        None => t!("profile.key_never_used").to_string(),
                    }
                })>
                    {move || {
                        used.get().map(|(_, at)| when(&at)).unwrap_or_else(|| MISSING.to_string())
                    }}
                </Lately>
                <Lately label=t!("profile.password_changed").to_string()>
                    {when(&account.password_set_at)}
                </Lately>
            </ul>
        </aside>
    }
}

/// A name and a figure, on one line with a rule under it.
#[component]
fn Fact(label: String, children: Children) -> impl IntoView {
    view! {
        <div>
            <dt>{label}</dt>
            <dd>{children()}</dd>
        </div>
    }
}

/// Something that happened, and when. The label is a signal because one of the three
/// names the key it is talking about, and which key that is arrives late.
#[component]
fn Lately(#[prop(into)] label: Signal<String>, children: Children) -> impl IntoView {
    view! {
        <li>
            <span>{move || label.get()}</span>
            <span>{children()}</span>
        </li>
    }
}

/// Who you are, in one form.
///
/// One form and one save, not one per field: the current password confirms *any*
/// change in here — the name and the address as much as the password — so splitting
/// it into a save per row would be asking for the password once per row.
///
/// A new password is asked for twice. Nothing can read one back — what is stored is
/// a hash — so a typo in the only copy is not a mistake anybody could find later: it
/// is an account whose password nobody knows, including its owner.
#[component]
fn Yourself(account: Account, save: Callback<AccountChanges>) -> impl IntoView {
    let admin = account.admin;
    // Held rather than read from the signal: what a listener sees is the name they
    // have, and the signal exists for the field only an administrator gets.
    let held = StoredValue::new(account.username.clone());

    let (username, set_username) = signal(account.username.clone());
    let (shown_as, set_shown_as) = signal(account.display_name.clone().unwrap_or_default());
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
            // Left out entirely unless it is yours to change, rather than sent
            // unchanged: the server refuses a rename from anybody but an
            // administrator, and sending the name back on every save would leave that
            // refusal one typo away from being what somebody sees when they change
            // their address.
            username: admin.then(|| username.get().trim().to_string()),
            // Always sent, empty included: an empty one is the request to go back to
            // being called by the account's name, which is not the same as not
            // having asked for anything.
            display_name: Some(shown_as.get().trim().to_string()),
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
        <h2 class="part">{t!("profile.you")}</h2>

        <form on:submit=submit>
            <div class="settings">
                // A field for an administrator and plain text for everybody else,
                // which is the same choice the role below makes and for the same
                // reason: a disabled box is a control that says it could be used.
                //
                // Renaming is administration. Your name is how an administrator knows
                // who you are, and it is what every OpenSubsonic client logs in with —
                // so changing it yourself would stop every player you own, quietly,
                // and put the way back under a name somebody else might have taken by
                // then.
                <Show
                    when=move || admin
                    fallback=move || {
                        view! {
                            <Setting
                                label=t!("profile.username").to_string()
                                why=t!("profile.username_theirs").to_string()
                            >
                                <span class="flat">{held.get_value()}</span>
                            </Setting>
                        }
                    }
                >
                    <Setting label=t!("profile.username").to_string()>
                        <input
                            required
                            prop:value=username
                            on:input:target=move |e| set_username.set(e.target().value())
                        />
                    </Setting>
                </Show>

                // Under the name of the account and above everything else, because it
                // is the answer to the field above it: that one is what an
                // administrator files you under and this is what you are called.
                <Setting label=t!("profile.display_name").to_string()>
                    <input
                        prop:value=shown_as
                        on:input:target=move |e| set_shown_as.set(e.target().value())
                    />
                </Setting>

                <Setting label=t!("profile.email").to_string()>
                    <input
                        type="email"
                        prop:value=email
                        on:input:target=move |e| set_email.set(e.target().value())
                    />
                </Setting>

                // Text and not a field: whether you administer the server is not
                // yours to set, and a disabled box would be a control that says it
                // could be used.
                <Setting label=t!("profile.role").to_string()>
                    <span class="flat">
                        {if admin {
                            t!("header.administrator")
                        } else {
                            t!("header.listener")
                        }}
                    </span>
                </Setting>

                <Setting
                    label=t!("profile.new_password").to_string()
                    why=t!("profile.unchanged").to_string()
                >
                    <input
                        type="password"
                        autocomplete="new-password"
                        prop:value=password
                        on:input:target=move |e| set_password.set(e.target().value())
                    />
                </Setting>

                // Written out rather than built from `Setting`, because its second
                // line is the only one on this screen that changes: the warning goes
                // where an explanation would have been, so a pair that does not match
                // says so without the row changing height.
                <div class="setting">
                    <div>
                        <span>{t!("profile.repeat_password")}</span>
                        <Show when=mismatched>
                            <span class="why alarm">{t!("profile.mismatch")}</span>
                        </Show>
                    </div>
                    <div>
                        <input
                            type="password"
                            autocomplete="new-password"
                            class:wrong=mismatched
                            prop:value=again
                            on:input:target=move |e| set_again.set(e.target().value())
                        />
                    </div>
                </div>

                // Last, and the only row said in the accent: everything above is
                // what is being asked for, and this is the asking answered.
                <Setting label=t!("profile.confirm_with_password").to_string() asked=true>
                    <input
                        type="password"
                        autocomplete="current-password"
                        prop:value=current
                        on:input:target=move |e| set_current.set(e.target().value())
                    />
                </Setting>
            </div>

            <div class="saving">
                // Nothing here can be saved without that password, so the button
                // says so by being unavailable rather than by letting somebody press
                // it and reading a refusal.
                <button
                    type="submit"
                    class="pill solid"
                    disabled=move || current.get().is_empty() || mismatched()
                >
                    {t!("profile.save_changes")}
                </button>
            </div>
        </form>
    }
}

/// What the server does with what you play.
///
/// Its own pane because nothing in it is worth guarding: a token for a music website
/// is not a credential to this server, and getting one wrong costs a wrong
/// destination rather than an account.
///
/// The switch and the destinations are one block on purpose. The switch is the older
/// half — OpenSubsonic has always carried it — and on its own it was a promise
/// nothing kept: it now means "pass my listens on", and what is under it is where to.
///
/// Nothing in here is saved by a button. The destinations always took effect as they
/// were pressed, and the switch had a Save of its own beside them: two buttons of
/// apparently equal rank, one of which did nothing for the list next to it. The switch
/// saves itself now and says so under itself — which is also what separates this pane
/// properly from the one above, where the button is there because the current password
/// guards what it sends.
#[component]
fn Listening(account: Account, on_expired: Callback<()>) -> impl IntoView {
    let me = StoredValue::new(account.username.clone());
    // What the server has, which is what the destinations below are described against.
    let (passing, set_passing) = signal(account.scrobbling);
    // What the checkbox shows, which runs ahead of the answer.
    let (shown, set_shown) = signal(account.scrobbling);
    let (saved, set_saved) = signal(false);
    let (failure, set_failure) = signal(Option::<String>::None);

    // Saved where it is changed, and its own call rather than the one the form above
    // shares: that one reports at the foot of the whole screen, a column away from
    // the switch somebody just touched.
    let flip = move |wanted: bool| {
        set_shown.set(wanted);
        set_saved.set(false);
        set_failure.set(None);

        spawn_local(async move {
            let changes = AccountChanges {
                scrobbling: Some(wanted),
                ..Default::default()
            };

            match api::change_account(&me.get_value(), changes).await {
                Ok(fresh) => {
                    set_passing.set(fresh.scrobbling);
                    set_shown.set(fresh.scrobbling);
                    set_saved.set(true);
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => {
                    // Back to what the server still has. A box left ticked over a
                    // refusal is a setting somebody believes they made.
                    set_shown.set(passing.get_untracked());
                    set_failure.set(Some(said(&why)));
                }
            }
        });
    };

    view! {
        <h2 class="part">{t!("profile.listening")}</h2>

        <div class="settings">
            <Setting
                label=t!("profile.scrobbling").to_string()
                why=t!("profile.scrobbling_why").to_string()
            >
                <label class="checkbox">
                    <input
                        type="checkbox"
                        prop:checked=shown
                        on:change:target=move |e| flip(e.target().checked())
                    />
                    <span>{t!("profile.scrobbling_on")}</span>
                </label>

                {move || saved.get().then(|| view! { <p class="settled">{t!("common.saved")}</p> })}
                {move || {
                    failure
                        .get()
                        .map(|why| view! { <p class="settled alarm" role="alert">{why}</p> })
                }}
            </Setting>
        </div>

        // Told what the switch is, so it can say what that means for the queue and go
        // quiet when nothing is being sent. The switch as the server has it and not as
        // the box stands, because what the list says is true of what was saved.
        <Destinations passing=Signal::derive(move || passing.get()) on_expired />
    }
}

/// Where the listens are passed on to, and what is waiting to go.
///
/// The catalogue of services comes from the server rather than from here, so a
/// service added there appears here without this file knowing its name. What this
/// knows is the shape: a name, an address for the ones that run on your own machine,
/// and a token.
#[component]
fn Destinations(
    /// Whether plays are being passed on at all, as the server has it. What decides
    /// whether any of this is doing anything, and so how it reads.
    passing: Signal<bool>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let (sending, set_sending) = signal(Option::<tocata::types::Scrobbling>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (unchecked, set_unchecked) = signal(false);
    let (busy, set_busy) = signal(false);
    // What the sheet is configuring, or nothing when it is shut.
    let asking = RwSignal::new(Option::<Asked>::None);

    let load = move || {
        spawn_local(async move {
            match api::scrobblers().await {
                Ok(found) => set_sending.set(Some(found)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
        });
    };

    load();

    let act = Callback::new(move |(what, service): (Doing, String)| {
        set_busy.set(true);
        set_failure.set(None);
        set_unchecked.set(false);

        spawn_local(async move {
            let outcome = match what {
                Doing::Pause => api::switch_scrobbler(&service, false).await.map(|_| ()),
                Doing::Resume => api::switch_scrobbler(&service, true).await.map(|_| ()),
                Doing::Remove => api::remove_scrobbler(&service).await,
            };

            match outcome {
                Ok(()) => load(),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    });

    // What could still be added: everything not already set up. With all of them set
    // up there is nothing to add, and the button says so by not being there.
    let spare = move || {
        sending.get().map(|sending| {
            sending
                .offered
                .into_iter()
                .filter(|offer| {
                    !sending
                        .scrobblers
                        .iter()
                        .any(|one| one.service == offer.service)
                })
                .collect::<Vec<_>>()
        })
    };

    view! {
        <div class="parted whither">
            <h3 class="part">{t!("listens.destinations")}</h3>
            {move || {
                spare()
                    .filter(|spare| !spare.is_empty())
                    .map(|_| {
                        // Opens on none of them: the sheet holds the list, and which
                        // one it is going to be is the first thing it asks.
                        view! {
                            <button
                                class="offer"
                                on:click=move |_| asking.set(Some(Asked::adding()))
                            >
                                <Glyph icon=Icon::Add />
                                {t!("listens.add")}
                            </button>
                        }
                    })
            }}
        </div>
        <p class="hint quiet">{t!("listens.lead")}</p>

        // A sibling of what has gone quiet and never a child of it. Dimmed along with
        // everything else, the one line explaining why the rest went quiet was the
        // least legible text on the screen.
        {move || {
            (!passing.get()).then(|| view! { <p class="because">{t!("listens.while_off")}</p> })
        }}

        <div class:stilled=move || !passing.get()>
            {move || match sending.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(sending) if sending.scrobblers.is_empty() => {
                    // What this state raises is what becomes of what you play
                    // meanwhile, and the answer is nothing: a listen is queued once per
                    // destination, so with none there is nothing to queue it against.
                    // Said only when the switch is on, because with it off the line
                    // above is already the answer.
                    view! {
                        <p class="nothing">{t!("listens.none")}</p>
                        {move || {
                            passing
                                .get()
                                .then(|| view! { <p class="hint quiet">{t!("listens.none_why")}</p> })
                        }}
                    }
                        .into_any()
                }
                Some(sending) => {
                    view! {
                        <ul class="ways tallied">
                            {sending
                                .scrobblers
                                .into_iter()
                                .map(|one| view! { <Destination one act busy asking /> })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
            }}
        </div>

        {move || {
            unchecked.get().then(|| view! { <p class="note">{t!("listens.unchecked")}</p> })
        }}
        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}

        <NewDestinationSheet
            asking
            spare=Signal::derive(move || spare().unwrap_or_default())
            catalogue=Signal::derive(move || {
                sending.get().map(|sending| sending.offered).unwrap_or_default()
            })
            on_saved=Callback::new(move |named: bool| {
                set_unchecked.set(!named);
                load();
            })
            on_expired
        />
    }
}

/// What can be done to a destination from its own row.
#[derive(Clone, Copy)]
enum Doing {
    Pause,
    Resume,
    Remove,
}

/// What the sheet was opened to do.
///
/// Two cases that look alike and are not: adding one, where the service is still to
/// be chosen from what is left, and changing the token of one that exists, where it
/// is settled and its address is already known. The same call underneath — the server
/// takes a destination and replaces whatever was there — and two different things to
/// have in front of you.
#[derive(Clone)]
struct Asked {
    service: String,
    /// The address to start with, which is only ever what a self hosted destination
    /// already had. Empty for anything being added.
    url: String,
    /// Whether this destination is already set up, which is what decides between
    /// choosing a service and being told which one.
    existing: bool,
}

impl Asked {
    /// With no service in it. Opening on the first spare one meant the sheet named a
    /// service nobody had chosen, and changed that name under the pointer as soon as
    /// they chose — so it opens asking which, and says nothing until it is told.
    fn adding() -> Self {
        Self {
            service: String::new(),
            url: String::new(),
            existing: false,
        }
    }

    fn changing(one: &tocata::types::Scrobbler) -> Self {
        Self {
            service: one.service.clone(),
            url: one.url.clone(),
            existing: true,
        }
    }
}

/// One destination: what it is, how it is going, and the three things that can be
/// done to it.
///
/// The same shape as a key's row, because it is the same kind of thing: a name with
/// one line under it and its actions behind the dots. No glyph, which is what keeps
/// this list from looking like that one.
#[component]
fn Destination(
    one: tocata::types::Scrobbler,
    act: Callback<(Doing, String)>,
    busy: ReadSignal<bool>,
    asking: RwSignal<Option<Asked>>,
) -> impl IntoView {
    let enabled = one.enabled;
    let service = StoredValue::new(one.service.clone());
    // Held rather than captured: the menu item's handler is called more than once,
    // and a closure that moved this out of scope would only be good for one press.
    let again = StoredValue::new(Asked::changing(&one));

    // How it is going, level with the name and out of the joined line below.
    //
    // It used to be the tail of that line, after the address — which put the one fact
    // that means something is wrong behind a URL that ellipsises away. It is the
    // figure somebody is looking for, so the address is what gives way.
    let queue = match one.waiting {
        0 => t!("listens.up_to_date").to_string(),
        waiting => t!("listens.waiting", count = waiting).to_string(),
    };

    // Only when something is actually stuck, which needs both halves: listens waiting
    // and somebody trying to send them. A destination that is merely behind catches up
    // on its own, and a paused one is a queue standing still on purpose — the reason
    // it gave the last time it tried would read as a problem in both.
    //
    // Being paused used to say the second half on its own: the error sat under the
    // word "paused", where it read as the reason somebody had paused it.
    let stuck = enabled && one.waiting > 0;
    let complaint = stuck.then_some(one.last_error).flatten();

    // The line under the name: which account it is over there, where it is, and how
    // far back the queue goes. Whichever of the three there is.
    let line = {
        let mut said = Vec::new();

        // Not a tag beside the name: `.tag` is the ink of something wrong, and being
        // signed in as somebody is the opposite of that.
        if let Some(name) = one.remote_name.as_deref() {
            said.push(t!("listens.as", name = name).to_string());
        }

        // Always, hosted or not. It is where somebody's listening is going, and the
        // rule that would hide it for the one service with a fixed address is a rule
        // that has to know which service that is.
        said.push(one.url.clone());

        // How long the oldest of them has been waiting, which the figure beside the
        // name cannot say. Nothing when there is no queue to date, and nothing when
        // the queue has no date — every queued listen carries the moment it was heard,
        // so this is the shape of a server that answered oddly rather than a case to
        // put a number in.
        if let Some(oldest) = one.oldest.as_deref().filter(|_| one.waiting > 0) {
            said.push(t!("listens.oldest", when = when(oldest)).to_string());
        }

        said.join(" · ")
    };

    view! {
        <li class:off=move || !enabled>
            <span class="what">
                {one.shown}
                // Quiet and not in the ink of something wrong: a revoked key wears the
                // same word shape and is a fault, and this is somebody's decision.
                {(!enabled)
                    .then(|| view! { <span class="tag deliberate">{t!("listens.paused")}</span> })}
            </span>

            <span class="standing" class:alarm=stuck>
                {queue}
            </span>

            <span class="doing">
                <Dots title=t!("listens.actions").to_string() disabled=busy>
                    <button class="menu-item" on:click=move |_| asking.set(Some(again.get_value()))>
                        <Glyph icon=Icon::Rotate />
                        {t!("listens.change_token")}
                    </button>
                    <button
                        class="menu-item"
                        on:click=move |_| {
                            act.run((
                                if enabled { Doing::Pause } else { Doing::Resume },
                                service.get_value(),
                            ))
                        }
                    >
                        <Glyph icon=if enabled { Icon::Pause } else { Icon::Play } />
                        {if enabled { t!("listens.pause") } else { t!("listens.resume") }}
                    </button>
                    <button
                        class="menu-item"
                        on:click=move |_| act.run((Doing::Remove, service.get_value()))
                    >
                        <Glyph icon=Icon::Remove />
                        {t!("listens.remove")}
                    </button>
                </Dots>
            </span>

            <span class="said">{line}</span>
            {complaint.map(|why| view! { <span class="said wrong">{why}</span> })}
        </li>
    }
}

/// Asks for an address and a token, and hands them over to be checked.
///
/// A sheet rather than a row of boxes in the column, for the reason issuing a key is
/// one: what is being typed is a secret, from somewhere else, and it wants the whole
/// width and the reader's whole attention for the one moment it is in front of them.
#[component]
fn NewDestinationSheet(
    asking: RwSignal<Option<Asked>>,
    spare: Signal<Vec<tocata::types::Offered>>,
    catalogue: Signal<Vec<tocata::types::Offered>>,
    on_saved: Callback<bool>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    // Which service this is about. Held apart from `asking` because it is the one
    // thing in here somebody can change while it is open.
    let chosen = RwSignal::new(String::new());
    let (address, set_address) = signal(String::new());
    let (token, set_token) = signal(String::new());
    let (failure, set_failure) = signal(Option::<String>::None);
    let (waiting, set_waiting) = signal(false);

    // Opening fills the boxes from whatever it was opened on, so changing a token
    // does not mean typing the address again — and closing empties them, because a
    // token left in a field is a token still on the screen.
    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        match asking.get() {
            Some(asked) => {
                set_failure.set(None);
                set_token.set(String::new());
                chosen.set(asked.service);
                set_address.set(asked.url);
                let _ = element.show_modal();
            }
            None => element.close(),
        }
    });

    let shut = move || {
        asking.set(None);
        set_token.set(String::new());
        set_address.set(String::new());
    };

    // What the catalogue says about the service in hand, which is where the answer to
    // "does this one have an address of its own" lives. Not in the row: a row carries
    // the address this instance is at, and the two look identical from here.
    let offer = move || {
        let service = chosen.get();

        catalogue
            .get()
            .into_iter()
            .find(|offer| offer.service == service)
    };

    // A service with an address everybody uses takes no address here: there is one
    // ListenBrainz, and a box that could point it elsewhere would make the name mean
    // nothing.
    let fixed = move || offer().is_some_and(|offer| offer.url.is_some());

    let named = move || offer().map(|offer| offer.shown).unwrap_or_default();

    // Only while adding. Once a destination exists, its service is what it is, and a
    // list to change it to would be a way of quietly setting up a different one.
    let choosing = move || asking.get().is_some_and(|asked| !asked.existing);

    // Said where the address is typed rather than refused: a scrobbler on your own
    // network is reached over plain HTTP as often as not, and this is the one place
    // somebody can weigh that up.
    let insecure = move || address.get().trim().starts_with("http://");

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();

        let service = chosen.get();
        if service.is_empty() {
            return;
        }

        set_waiting.set(true);
        set_failure.set(None);

        let asked = tocata::types::NewScrobbler {
            url: (!fixed()).then(|| address.get().trim().to_string()),
            token: token.get().trim().to_string(),
            enabled: Some(true),
        };

        spawn_local(async move {
            match api::set_scrobbler(&service, asked).await {
                // Whether the service vouched for the token decides what the screen
                // says next: a name means it was checked, and nothing means it could
                // not be asked.
                Ok(saved) => {
                    on_saved.run(saved.remote_name.is_some());
                    shut();
                }
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(match &why {
                    Failure::Refused(code) if code == "tokenRefused" => {
                        t!("listens.refused").to_string()
                    }
                    other => said(other),
                })),
            }
            set_waiting.set(false);
        });
    };

    view! {
        <dialog node_ref=dialog class="sheet" on:close=move |_| shut()>
            <form on:submit=submit>
                <div class="sheet-body">
                    // Neutral until there is a service to name. Named before that, it
                    // said one thing on opening and another the moment somebody chose,
                    // which is a heading changing under the hands of whoever reads it.
                    <h2>
                        {move || {
                            if chosen.get().is_empty() {
                                t!("listens.somewhere").to_string()
                            } else {
                                t!("listens.at", name = named()).to_string()
                            }
                        }}
                    </h2>
                    <p class="sheet-lead">{t!("listens.sheet_lead")}</p>

                    <div class="sheet-content">
                        // Only when adding, and only what is not set up already: the
                        // sheet opened on one of them, and this is how to pick
                        // another without going back for a second menu.
                        <Show when=choosing>
                            <label>
                                <span>{t!("listens.service")}</span>
                                <select
                                    prop:value=move || chosen.get()
                                    on:change:target=move |e| {
                                        chosen.set(e.target().value());
                                        // Each instance is at its own address, so what
                                        // was typed for one says nothing about the next.
                                        set_address.set(String::new());
                                    }
                                >
                                    // The state the sheet opens in, and a real option
                                    // rather than a blank line: a list whose first entry
                                    // is already selected is a choice somebody has made
                                    // without knowing it.
                                    <option value="">{t!("listens.choose")}</option>
                                    <For
                                        each=move || spare.get()
                                        key=|offer| offer.service.clone()
                                        let:offer
                                    >
                                        <option value=offer.service>{offer.shown}</option>
                                    </For>
                                </select>
                            </label>
                        </Show>

                        <Show when=move || !fixed()>
                            <label>
                                <span>{t!("listens.address")}</span>
                                <input
                                    placeholder="http://localhost:4110"
                                    prop:value=address
                                    on:input:target=move |e| set_address.set(e.target().value())
                                />
                                <span class="hint">
                                    {move || {
                                        if insecure() {
                                            t!("listens.insecure")
                                        } else {
                                            t!("listens.address_hint")
                                        }
                                    }}
                                </span>
                            </label>
                        </Show>

                        <label>
                            <span>{t!("listens.token")}</span>
                            // A password field: it is a secret, and it is being typed
                            // on a screen somebody may not be alone in front of.
                            <input
                                type="password"
                                autocomplete="off"
                                autofocus
                                prop:value=token
                                on:input:target=move |e| set_token.set(e.target().value())
                            />
                            <span class="hint">{t!("listens.token_hint")}</span>
                        </label>
                    </div>
                </div>

                <div class="sheet-foot">
                    {move || {
                        failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
                    }}
                    <button type="button" class="pill" disabled=waiting on:click=move |_| shut()>
                        {t!("common.cancel")}
                    </button>
                    <button
                        type="submit"
                        class="pill solid"
                        disabled=move || {
                            waiting.get() || chosen.get().is_empty()
                                || token.get().trim().is_empty()
                                || (!fixed() && address.get().trim().is_empty())
                        }
                    >
                        {move || {
                            if waiting.get() { t!("listens.checking") } else { t!("common.save") }
                        }}
                    </button>
                </div>
            </form>
        </dialog>
    }
}

/// What opens your account: the keys clients hold, and the logins you have here.
///
/// Both are yours to look at one at a time, which is the difference from what an
/// administrator sees: they get to cut you off, and cutting somebody off does not
/// need to know that your third key is called "phone".
///
/// Two lists rather than two tables. Each row is a name and one line under it
/// joining what a table gave three columns to, which is what lets the two lists
/// sit side by side: a key and a login are the same shape, and the shape is a
/// heading with a sentence under it.
///
/// The lists are loaded here and the figures over them are read out of the very
/// same lists. A count asked of the server separately would be a second answer to
/// a question already on the screen, and the two would disagree the moment
/// something was revoked in another tab.
#[component]
pub fn Access(on_expired: Callback<()>) -> impl IntoView {
    let (keys, set_keys) = signal(Option::<Vec<tocata::types::Key>>::None);
    let (logins, set_logins) = signal(Option::<Vec<tocata::types::Login>>::None);
    let (changed, set_changed) = signal(Option::<String>::None);

    // The name as it stands, not as it was when the panel was built: renaming
    // yourself on the profile screen and coming straight here used to ask about an
    // account that no longer exists, and every list on the screen came back empty.
    let me = expect_context::<crate::layout::Me>().username;

    // The one thing on this screen that neither list knows: when the password was
    // last set. It is a figure here and a line at the foot of the sessions, and
    // changing it is on the other screen, which is where the link goes.
    spawn_local(async move {
        if let Ok(account) = api::account(&me.get_untracked()).await {
            set_changed.set(Some(account.password_set_at));
        }
    });

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.access")}</h1>
                <p class="quiet lead">{t!("access.lead")}</p>
            </div>
        </header>

        <Figures keys logins changed />

        <div class="lists">
            <MyKeys username=me.get_untracked() keys set_keys on_expired />
            <MySessions username=me.get_untracked() logins set_logins changed on_expired />
        </div>
    }
}

/// The four figures across the top: what still opens the account, and how long it
/// has been since either of the two things that do was used.
///
/// Durations rather than dates, and short ones. "5 h" over "since a key was used"
/// answers the question somebody came here with; the date it happened is in the
/// row that says which key it was.
#[component]
fn Figures(
    keys: ReadSignal<Option<Vec<tocata::types::Key>>>,
    logins: ReadSignal<Option<Vec<tocata::types::Login>>>,
    changed: ReadSignal<Option<String>>,
) -> impl IntoView {
    // The ones that still open something. Revoked and expired keys stay in the
    // listing on purpose, so counting rows would count keys that do nothing.
    let working = move || {
        keys.get().map(|list| {
            list.iter()
                .filter(|key| standing(key) == Standing::Live)
                .count()
                .to_string()
        })
    };

    let open = move || logins.get().map(|list| list.len().to_string());

    // The most recent use across every key, whichever key that was: the figure is
    // how long the clients have been quiet, and one of several being busy is the
    // answer for all of them.
    let used = move || {
        keys.get().and_then(|list| {
            list.iter()
                .filter_map(|key| key.last_used_at.clone())
                .max()
                .map(|at| lapse(&at))
        })
    };

    let password = move || changed.get().map(|at| lapse(&at));

    view! {
        <div class="counts">
            <Tally
                label=t!("access.keys_in_use").to_string()
                figure=Signal::derive(working)
            />
            <Tally
                label=t!("access.sessions_open").to_string()
                figure=Signal::derive(open)
            />
            <Tally label=t!("access.since_used").to_string() figure=Signal::derive(used) />
            <Tally
                label=t!("access.since_password").to_string()
                figure=Signal::derive(password)
            />
        </div>
    }
}

/// What you have on this server, in four figures.
///
/// About the listening rather than about the account: what opens it is on the access
/// screen, said in the same four boxes, and the two figures a rail already carries
/// would be a third place to read them. These are the ones nothing else says.
///
/// Bookmarks are not among them. A bookmark is where an hour long recording was left
/// off, so the figure is how many are unfinished — which is not something anybody
/// collects, and reads as a reproach next to three things that are.
#[component]
fn Listened(held: ReadSignal<Option<Holdings>>) -> impl IntoView {
    let counted = move |of: fn(&Holdings) -> i64| {
        Signal::derive(move || held.get().map(|held| thousands(of(&held))))
    };

    view! {
        <div class="counts">
            <Tally
                label=t!("profile.plays").to_string()
                figure=counted(|held| held.plays)
                icon=Icon::Plays
            />
            <Tally
                label=t!("profile.favourites").to_string()
                figure=counted(|held| held.favourites)
                icon=Icon::Favourites
            />
            <Tally
                label=t!("profile.ratings").to_string()
                figure=counted(|held| held.ratings)
                icon=Icon::Ratings
            />
            <Tally
                label=t!("profile.playlists").to_string()
                figure=counted(|held| held.playlists)
                icon=Icon::Playlists
            />
        </div>
    }
}

/// One figure over what it counts, with a dash until the answer arrives.
///
/// The glyph, where there is one, goes on the label line at the size of the label:
/// it is part of the word rather than a mark beside the number, which is what the
/// four figures on the overview settled. Two of the figures on the access screen are
/// spans of time and have nothing to draw, so there it is left off altogether rather
/// than drawn on half a row.
#[component]
fn Tally(
    label: String,
    figure: Signal<Option<String>>,
    #[prop(optional)] icon: Option<Icon>,
) -> impl IntoView {
    view! {
        <div class="count">
            <span class="figure">
                {move || figure.get().unwrap_or_else(|| MISSING.to_string())}
            </span>
            {match icon {
                Some(icon) => {
                    view! {
                        <span class="quiet named-figure">
                            <Glyph icon />
                            {label}
                        </span>
                    }
                        .into_any()
                }
                None => view! { <span class="quiet">{label}</span> }.into_any(),
            }}
        </div>
    }
}

/// Which of three states a key is in, which is what decides both what its row
/// offers and what the line under its name says.
///
/// Revoked beats expired. A key that was withdrawn and then ran out was withdrawn,
/// and the moment worth reporting is the one somebody caused.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// It opens the account. The only state with anything left to stop.
    Live,
    /// Its date has passed. It stopped on its own, so there is nothing to revoke
    /// and it can go straight out of the listing.
    Expired,
    /// Withdrawn, and final: there is no way back to a key that works, only the
    /// way out.
    Revoked,
}

fn standing(key: &tocata::types::Key) -> Standing {
    if key.revoked_at.is_some() {
        Standing::Revoked
    } else if key.expired {
        Standing::Expired
    } else {
        Standing::Live
    }
}

/// The line under a key's name: when it was last used, what it does about
/// expiring, and either when it was made or that it is finished.
///
/// Joined rather than laid out. What each part says depends on the state, and
/// three columns for it would be three columns two of which are usually empty.
fn key_line(key: &tocata::types::Key) -> String {
    let used = match key.last_used_at.as_deref() {
        Some(at) => t!("keys.used", when = since(at)).to_string(),
        None => t!("keys.unused").to_string(),
    };

    let made = || t!("keys.created", when = since(&key.created_at)).to_string();
    let dead = || t!("keys.dead").to_string();

    // Read off the two timestamps rather than off the state above, because three of
    // the four branches have a moment to say and this is where it is in hand.
    let (until, note) = match (key.revoked_at.as_deref(), key.expires_at.as_deref()) {
        (Some(at), _) => (
            t!("keys.revoked_when", when = since(at)).to_string(),
            dead(),
        ),
        (None, Some(at)) if key.expired => {
            (t!("keys.ran_out", day = on_day(at)).to_string(), dead())
        }
        (None, Some(at)) => (t!("keys.expires", day = on_day(at)).to_string(), made()),
        (None, None) => (t!("keys.forever").to_string(), made()),
    };

    [used, until, note].join(" · ")
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
        <header class="titled">
            <div>
                <h1>{t!("nav.preferences")}</h1>
                <p class="quiet lead">{t!("prefs.lead")}</p>
            </div>
        </header>

        // Three rows of what it is against how it is set, and no headings over
        // them: three settings do not need to be grouped into sections of one, and
        // the words on the left already say which is which.
        <div class="settings">
            <div class="setting">
                <span>{t!("prefs.theme")}</span>
                // Words rather than buttons. Three of them fit on a line, and a
                // button apiece drew three boxes to say a thing the words say.
                <div class="options">
                    {theme::AVAILABLE
                        .iter()
                        .map(|choice| {
                            let choice = *choice;
                            view! {
                                <button
                                    class="option"
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
            </div>

            <div class="setting">
                <span>{t!("prefs.accent")}</span>
                // The colour of each swatch is the same rule that colours the panel,
                // applied to the button: the attribute redefines the variable for
                // whatever carries it, so plum is plum here whatever is in force.
                //
                // Written without `attr:`, which is not optional here. On a
                // component that prefix is what says "an attribute rather than a
                // property of mine"; on a plain element the macro leaves it in the
                // name, because anything with a hyphen in it is passed through
                // untouched. The attribute came out called `attr:data-accent`, no
                // selector matched it, and all six swatches inherited the accent in
                // force — six circles of the same colour.
                <div class="swatches">
                    {accent::AVAILABLE
                        .iter()
                        .map(|choice| {
                            let choice = *choice;
                            view! {
                                <button
                                    class="swatch"
                                    class:chosen=move || accent.get() == choice
                                    data-accent=choice
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

            <div class="setting">
                <label for="locale">{t!("prefs.language")}</label>
                <div>
                    <select
                        id="locale"
                        class="narrow"
                        on:change:target=move |event| {
                            let chosen = event.target().value();
                            pick_locale((!chosen.is_empty()).then_some(chosen));
                        }
                    >
                        // Empty rather than absent, because "whatever the browser
                        // asks for" is a choice somebody can come back to.
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
                    <p class="hint quiet">{t!("prefs.language_note")}</p>
                </div>
            </div>
        </div>

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

/// Your own keys: making them, rotating them, withdrawing them, and finally
/// taking them out of the list.
///
/// A key is readable once, when it is made and when it is rotated. What the
/// database keeps is a hash, so there is no second chance to look at it.
///
/// Which is why both of those happen in a dialogue that shows the secret and
/// forgets it on the way out. Shown in the page instead, it was a box that
/// appeared and disappeared — moving everything under it — and a piece of state
/// somebody had to remember to clear when the key it belonged to was revoked.
/// A dialogue closes, and closing it is the clearing.
///
/// Every row keeps its menu, where a session gets a single word. A session can
/// only be closed; a key can be rotated as well as withdrawn, and the way to
/// offer two things is not to put one of them behind the other's name.
#[component]
fn MyKeys(
    username: String,
    keys: ReadSignal<Option<Vec<tocata::types::Key>>>,
    set_keys: WriteSignal<Option<Vec<tocata::types::Key>>>,
    on_expired: Callback<()>,
) -> impl IntoView {
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
                    Ok(_) => load(),
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    Err(why) => set_failure.set(Some(said(&why))),
                },
                KeyAction::Remove => match api::remove_key(&who.get_value(), id).await {
                    Ok(()) => load(),
                    Err(Failure::Unauthenticated) => on_expired.run(()),
                    // A conflict here is the server saying the key still works,
                    // which the row it was pressed on did not offer: it takes
                    // another tab having brought the key back into use.
                    Err(why) => set_failure.set(Some(match &why {
                        Failure::Refused(code) if code == "conflict" => {
                            t!("keys.revoke_first").to_string()
                        }
                        other => said(other),
                    })),
                },
            }
            set_busy.set(false);
        });
    });

    // Asked for twice, because it is every client at once and there is no
    // unrevoking: what it stops has to be given a new key to start again.
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

    // The ones it would actually stop, which is also what it says: with one key
    // left working it is the action already in that key's own row, under a name
    // that sounds bigger.
    let working = move || {
        keys.get().map_or(0, |list| {
            list.iter()
                .filter(|key| standing(key) == Standing::Live)
                .count()
        })
    };

    let several = move || working() > 1;

    view! {
        <section class="pane">
            // The action that adds to the list, beside the name of the list. A pill
            // here would be the loudest thing in a column of hairlines, and what it
            // opens is a dialogue that asks for two words.
            <div class="parted">
                <h2>{t!("keys.heading")}</h2>
                <button class="offer" on:click=move |_| set_asking.set(true)>
                    <Glyph icon=Icon::Add />
                    {t!("keys.issue")}
                </button>
            </div>
            <p class="hint quiet">{t!("keys.lead")}</p>

            {move || match keys.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(list) if list.is_empty() => {
                    view! { <p class="nothing">{t!("keys.none")}</p> }.into_any()
                }
                Some(list) => {
                    view! {
                        <ul class="ways keyed">
                            {list
                                .into_iter()
                                .map(|key| view! { <KeyRow key act busy /> })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
            }}

            // At the foot of the list rather than at the top of it: what makes
            // somebody reach for this is having read the list and found a name they
            // no longer trust. Phrased as the question that brings them here, so the
            // answer beside it can be the plain word for what it does.
            <Show when=several>
                <p class="after">
                    <Show
                        when=move || cutting.get()
                        fallback=move || {
                            view! {
                                <span>{t!("keys.lost_device")}</span>
                                <button
                                    class="link risky"
                                    disabled=busy
                                    on:click=move |_| set_cutting.set(true)
                                >
                                    {t!("keys.revoke_all", count = working())}
                                </button>
                            }
                        }
                    >
                        <span>{t!("keys.revoke_all_sure")}</span>
                        <button class="link risky" disabled=busy on:click=revoke_all>
                            {t!("keys.revoke_all_yes")}
                        </button>
                        <button class="link" disabled=busy on:click=move |_| set_cutting.set(false)>
                            {t!("common.cancel")}
                        </button>
                    </Show>
                </p>
            </Show>

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

    let (copied, set_copied) = signal(false);

    // Straight to the clipboard, because the alternative is somebody selecting a
    // forty character string by hand at the one moment it can still be read. The
    // word on the button changes and stays changed for as long as the dialogue is
    // open: there is nothing else to say afterwards, and a message that faded would
    // be a message somebody missed.
    //
    // Nothing is said when it fails. The clipboard needs permission the browser may
    // not give, and the key is right there to be selected — `user-select: all` means
    // one press takes all of it.
    let copy = move |_| {
        let Some(key) = secret().map(|key| key.key) else {
            return;
        };
        let Some(window) = web_sys::window() else {
            return;
        };

        let _ = window.navigator().clipboard().write_text(&key);
        set_copied.set(true);
    };

    // Closing is the clearing, and "Copied" is one of the things being cleared: it
    // was said about a secret that no longer exists, and left standing it greeted the
    // next key with the news that it had already been taken.
    let shut = move || {
        set_asking.set(false);
        set_rotated.set(None);
        set_made.set(None);
        set_copied.set(false);
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
            <Show
                when=move || secret().is_some()
                fallback=move || {
                    view! {
                        <form on:submit=submit>
                            <div class="sheet-body">
                                <h2>{t!("keys.issue")}</h2>
                                <p class="sheet-lead">{t!("keys.issue_lead")}</p>

                                <div class="sheet-content">
                                    <label>
                                        <span>{t!("keys.label")}</span>
                                        <input
                                            placeholder=t!("keys.label_default")
                                            autofocus
                                            prop:value=label
                                            on:input:target=move |e| {
                                                set_label.set(e.target().value())
                                            }
                                        />
                                    </label>

                                    <label>
                                        <span>{t!("keys.until")}</span>
                                        // Dimmed while empty, because a date field
                                        // shows dd/mm/yyyy whether or not anything
                                        // has been typed, and in the ink of real text
                                        // it reads as a value already chosen.
                                        <input
                                            type="date"
                                            class:empty=move || expires.get().is_empty()
                                            prop:value=expires
                                            on:input:target=move |e| {
                                                set_expires.set(e.target().value())
                                            }
                                        />
                                        <span class="hint">{t!("keys.until_note")}</span>
                                    </label>
                                </div>
                            </div>

                            <div class="sheet-foot">
                                <button
                                    type="button"
                                    class="away"
                                    disabled=waiting
                                    on:click=move |_| shut()
                                >
                                    {t!("common.cancel")}
                                </button>
                                <button type="submit" class="pill solid" disabled=waiting>
                                    {move || {
                                        if waiting.get() {
                                            t!("login.working")
                                        } else {
                                            t!("keys.issue")
                                        }
                                    }}
                                </button>
                            </div>

                            {move || {
                                failure
                                    .get()
                                    .map(|why| view! { <p class="failure" role="alert">{why}</p> })
                            }}
                        </form>
                    }
                }
            >
                // The one moment this will ever be readable, so it is the only thing
                // in the panel on a surface of its own: a band tinted with the accent,
                // the secret in it, and one press to take it away whole.
                <div class="sheet-body">
                    <h2>{t!("keys.made")}</h2>
                    <p class="sheet-lead">{t!("keys.once")}</p>

                    <div class="handover">
                        <code>{move || secret().map(|key| key.key).unwrap_or_default()}</code>
                        <button type="button" on:click=copy>
                            {move || {
                                if copied.get() { t!("common.copied") } else { t!("common.copy") }
                            }}
                        </button>
                    </div>

                    // What it is, now that it is made. Read from the answer rather
                    // than from the form, so it says what was stored and not what was
                    // asked for.
                    <dl class="facts">
                        <div>
                            <dt>{t!("keys.label")}</dt>
                            <dd>
                                {move || {
                                    secret()
                                        .map(|key| key.label)
                                        .unwrap_or_default()
                                }}
                            </dd>
                        </div>
                        <div>
                            <dt>{t!("keys.until")}</dt>
                            <dd>
                                {move || {
                                    secret()
                                        .and_then(|key| key.expires_at)
                                        .map(|at| crate::pages::when(&at))
                                        .unwrap_or_else(|| t!("keys.never").to_string())
                                }}
                            </dd>
                        </div>
                    </dl>
                </div>

                <div class="sheet-foot">
                    <button type="button" class="pill solid" on:click=move |_| shut()>
                        {t!("common.done")}
                    </button>
                </div>
            </Show>
        </dialog>
    }
}

/// What the menu at the end of a key's row offers.
#[derive(Clone, Copy)]
enum KeyAction {
    Rotate,
    Revoke,
    Remove,
}

/// One key as a row: the glyph, its name, the sentence under it, and its actions
/// behind the dots.
///
/// What the menu offers is what the key's state leaves to be done. A working key
/// can be given a new secret or withdrawn; one that has stopped working, whether
/// it was withdrawn or ran out, can only go. Removing is never offered beside
/// revoking, because the two are the same step twice and the order is the point:
/// the row is where the key's name is, and the name is the only thing a revocation
/// can be checked against afterwards.
#[component]
fn KeyRow(
    key: tocata::types::Key,
    act: Callback<(KeyAction, i64)>,
    busy: ReadSignal<bool>,
) -> impl IntoView {
    let id = key.id;
    let state = standing(&key);
    let line = key_line(&key);
    let label = key.label;

    view! {
        <li class:off=move || state != Standing::Live>
            <span class="mark">
                <Glyph icon=Icon::Key />
            </span>

            <span class="what">
                {label}
                {match state {
                    Standing::Live => ().into_any(),
                    Standing::Expired => {
                        view! { <span class="tag">{t!("keys.is_expired")}</span> }.into_any()
                    }
                    Standing::Revoked => {
                        view! { <span class="tag">{t!("keys.is_revoked")}</span> }.into_any()
                    }
                }}
            </span>

            <span class="doing">
                <Dots title=t!("keys.actions").to_string() disabled=busy>
                    <Show when=move || state == Standing::Live>
                        <button
                            class="menu-item"
                            on:click=move |_| act.run((KeyAction::Rotate, id))
                        >
                            <Glyph icon=Icon::Rotate />
                            {t!("keys.rotate")}
                        </button>
                        <button
                            class="menu-item"
                            on:click=move |_| act.run((KeyAction::Revoke, id))
                        >
                            <Glyph icon=Icon::Remove />
                            {t!("keys.revoke")}
                        </button>
                    </Show>

                    <Show when=move || state != Standing::Live>
                        <button class="menu-item" on:click=move |_| act.run((KeyAction::Remove, id))>
                            <Glyph icon=Icon::Remove />
                            {t!("keys.remove")}
                        </button>
                    </Show>
                </Dots>
            </span>

            <span class="said">{line}</span>
        </li>
    }
}

/// Your own panel sessions, and closing them.
///
/// The same rows as the keys beside them, because the two are the same shape: a
/// name, one line saying since when and until when, and one thing to do about it.
/// A session has only ever had the one, so it is a word in the row rather than a
/// word behind a menu.
///
/// The column ends with the password, because that is what somebody who came here
/// to shut a browser out thinks of next — and it is a line saying when it changed
/// and a link to the screen that changes it, not a form. Two screens with the same
/// field on them is two places to get it wrong.
#[component]
fn MySessions(
    username: String,
    logins: ReadSignal<Option<Vec<tocata::types::Login>>>,
    set_logins: WriteSignal<Option<Vec<tocata::types::Login>>>,
    changed: ReadSignal<Option<String>>,
    on_expired: Callback<()>,
) -> impl IntoView {
    let (failure, set_failure) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);

    let who = StoredValue::new(username);

    let load = move || {
        spawn_local(async move {
            match api::sessions(&who.get_value()).await {
                Ok(list) => set_logins.set(Some(list)),
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

    // Every one of them except this one, which is why it needs no confirming: this
    // is the screen somebody is doing it from, and the only thing that should take
    // them out of it is the button that says Log out. The worst it can do is make
    // somebody log in again somewhere they are not sitting.
    let close_others = move |_| {
        set_busy.set(true);
        set_failure.set(None);

        spawn_local(async move {
            match api::close_sessions(&who.get_value()).await {
                Ok(_) => load(),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }
            set_busy.set(false);
        });
    };

    let several = move || logins.get().is_some_and(|list| list.len() > 1);

    view! {
        <section class="pane">
            <h2>{t!("sessions.heading")}</h2>
            <p class="hint quiet">{t!("sessions.lead")}</p>

            {move || match logins.get() {
                None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
                Some(mut list) => {
                    // This browser first, whatever the server's order was. It is the
                    // row somebody looks for to check they are reading the right list,
                    // and the one row that must not move about: the others are told
                    // apart by their dates, and it is told apart by being yours.
                    //
                    // A stable sort, so the rest stay as they came — oldest login
                    // first, which is the order the server lists them in.
                    list.sort_by_key(|login| !login.current);

                    view! {
                        <ul class="ways">
                            {list
                                .into_iter()
                                .map(|login| {
                                    let id = login.id;
                                    let current = login.current;
                                    view! {
                                        <li>
                                            <span class="what">
                                                {if current {
                                                    t!("sessions.this_browser")
                                                } else {
                                                    t!("sessions.another_browser")
                                                }}
                                                {current
                                                    .then(|| {
                                                        view! {
                                                            <span class="badge">
                                                                {t!("sessions.this_one")}
                                                            </span>
                                                        }
                                                    })}
                                            </span>

                                            <span class="doing">
                                                <button
                                                    class="link"
                                                    disabled=busy
                                                    on:click=move |_| close((id, current))
                                                >
                                                    {if current {
                                                        t!("sessions.log_out")
                                                    } else {
                                                        t!("sessions.close")
                                                    }}
                                                </button>
                                            </span>

                                            // Since when, for the one being used, and
                                            // when it was last used for one that is
                                            // not: what somebody wants of a browser
                                            // they are not sitting at is how long ago
                                            // it was.
                                            <span class="said">
                                                {if current {
                                                    t!(
                                                        "sessions.since",
                                                        when = since(&login.created_at),
                                                    )
                                                } else {
                                                    t!(
                                                        "sessions.seen",
                                                        when = since(&login.last_seen_at),
                                                    )
                                                }} " · "
                                                {t!(
                                                    "sessions.expires_on",
                                                    day = on_day(&login.expires_at),
                                                )}
                                            </span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
            }}

            <Show when=several>
                <p class="after">
                    <span>{t!("sessions.elsewhere")}</span>
                    <button class="link risky" disabled=busy on:click=close_others>
                        {t!("sessions.close_others")}
                    </button>
                </p>
            </Show>

            {move || {
                failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
            }}

            <h2>{t!("access.password")}</h2>
            <p class="tail">
                <span>
                    {move || match changed.get() {
                        Some(at) => t!("access.password_changed", when = since(&at)).to_string(),
                        None => MISSING.to_string(),
                    }}
                </span>
                <A href=crate::layout::MINE_PATH>{t!("access.change_password")}</A>
            </p>
        </section>
    }
}
