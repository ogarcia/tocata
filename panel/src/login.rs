// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The way in, and the only screen with no panel around it.
//!
//! Two bands: the name of the program, and the form. Side by side on a wide
//! screen, with the name on the left and one line under it saying what this is;
//! stacked on a narrow one, with the name at the top and the form under it. That
//! is the order they are written in, so neither layout has to reorder the other's
//! and neither is read in an order nobody would say it in.
//!
//! A real form with a submit button, not a div listening for clicks: pressing
//! enter in a password field has meant "log in" since before any of this, and a
//! password manager recognises this shape and does not recognise the other one.
//!
//! Nothing here says anything about the server to somebody who has not got in
//! yet. Not what it is called, not how much music it holds, not which version it
//! is: this is the one page a stranger can reach, and none of that would help the
//! person who is actually trying to log in.

use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::Identity;

#[component]
pub fn LogIn(on_in: Callback<Identity>) -> impl IntoView {
    let (username, set_username) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (remember, set_remember) = signal(true);
    let (shown, set_shown) = signal(false);
    let (failure, set_failure) = signal(Option::<String>::None);
    let (waiting, set_waiting) = signal(false);

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        set_waiting.set(true);
        set_failure.set(None);

        let (user, pass, keep) = (username.get(), password.get(), remember.get());

        spawn_local(async move {
            match api::log_in(user, pass, keep).await {
                Ok(identity) => on_in.run(identity),
                Err(why) => {
                    // Wrong credentials arrive as a 401 like an expired session
                    // does, and here it can only mean the first.
                    //
                    // A server that broke is not a password that is wrong, and this
                    // used to say it was: on a first run scanning eleven thousand
                    // files the login came back 500 — the scan had the write lock and
                    // the session could not be recorded — and this screen answered
                    // that the username and password did not go together. Somebody
                    // spent that scan doubting a password that was right.
                    //
                    // Which is why the catch-all is gone. What is left in it is 401,
                    // and 401 here does mean the credentials.
                    let said = match &why {
                        Failure::Unreachable => t!("login.unreachable"),
                        Failure::Refused(code) if code == "tooManyAttempts" => {
                            t!("login.too_many")
                        }
                        Failure::Refused(_) => t!("login.server_wrong"),
                        Failure::Unauthenticated => t!("login.failed"),
                    };
                    set_failure.set(Some(said.to_string()));
                    set_waiting.set(false);
                }
            }
        });
    };

    view! {
        <main class="entry">
            // The name, and on a wide screen the one line that says what this is.
            // Nothing else — what a stranger can read here is the whole of what
            // this page tells anybody who has not got in.
            <div class="banner">
                <p class="mark">
                    <Glyph icon=Icon::Logo />
                    {t!("app.name")}
                </p>

                // Beside the form, never over it: on a narrow screen the band has
                // nothing to be beside and this line is a second heading arguing
                // with the first, so the stylesheet takes it away.
                <p class="claim">{t!("login.claim")}</p>
            </div>

            <div class="entering">
                <form on:submit=submit>
                    <h1>{t!("login.heading")}</h1>
                    <p class="quiet lead">{t!("login.lead")}</p>

                    // A block above the fields rather than red borders on them:
                    // nothing is wrong with either field on its own, and marking
                    // both would be saying which one to look at when the server
                    // deliberately did not say.
                    {move || {
                        failure
                            .get()
                            .map(|why| {
                                view! {
                                    <p class="trouble" role="alert">
                                        <Glyph icon=Icon::Alert />
                                        <span>{why}</span>
                                    </p>
                                }
                            })
                    }}

                    // The label wraps its field rather than pointing at it by id:
                    // it is one thing, and what a password manager reads is the
                    // name and the autocomplete, which are still here.
                    <label>
                        <span>{t!("login.username")}</span>
                        // Either the account's name or the address on it, which the
                        // server tries in that order. `username` is still the right
                        // autocomplete: it is what a password manager fills a login
                        // field with, whichever of the two it has stored.
                        <input
                            name="username"
                            autocomplete="username"
                            autofocus
                            required
                            prop:value=username
                            on:input:target=move |e| set_username.set(e.target().value())
                        />
                    </label>

                    // The field comes before the button that reveals it, because
                    // tab follows the markup: written the way it is drawn — the
                    // word "Show" sits up on the label's line — tabbing out of the
                    // username landed on Show rather than on the password, which
                    // is not where anybody typing their way in is going. The
                    // stylesheet puts the button back up on that line, so the
                    // order somebody sees is unchanged and the order they walk
                    // through is username, password, Show.
                    <label class="secret">
                        <span>{t!("login.password")}</span>
                        <input
                            name="password"
                            type=move || if shown.get() { "text" } else { "password" }
                            autocomplete="current-password"
                            required
                            prop:value=password
                            on:input:target=move |e| set_password.set(e.target().value())
                        />
                        // Beside the label rather than inside the field: a glyph
                        // sitting on the line the text is typed on is a target
                        // nobody can name, and this one has to say which of the two
                        // things it will do.
                        <button
                            type="button"
                            class="reveal"
                            on:click=move |_| set_shown.update(|is| *is = !*is)
                        >
                            {move || {
                                if shown.get() {
                                    t!("common.hide").to_string()
                                } else {
                                    t!("common.show").to_string()
                                }
                            }}
                        </button>
                    </label>

                    // The words loose rather than in a span, like every other
                    // checkbox here: the span in a field label is what the small
                    // capitals are hung on, and this is a sentence.
                    <label class="checkbox">
                        <input
                            type="checkbox"
                            prop:checked=remember
                            on:change:target=move |e| set_remember.set(e.target().checked())
                        />
                        {t!("login.remember")}
                    </label>

                    // The width of the form, and tall enough to be a target on a
                    // phone. The one screen where the action is the only thing to
                    // do is the one screen where it fills the column.
                    <button type="submit" class="pill solid wide" disabled=waiting>
                        {move || {
                            if waiting.get() {
                                t!("login.working").to_string()
                            } else {
                                t!("login.submit").to_string()
                            }
                        }}
                    </button>

                    <p class="recovery">{t!("login.forgotten")}</p>
                </form>
            </div>
        </main>
    }
}
