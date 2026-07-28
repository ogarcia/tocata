// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The form, and nothing else.
//!
//! A real form with a submit button, not a div listening for clicks: pressing
//! enter in a password field has meant "log in" since before any of this, and a
//! password manager recognises this shape and does not recognise the other one.

use crate::api::{self, Failure};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::Identity;

#[component]
pub fn LogIn(on_in: Callback<Identity>) -> impl IntoView {
    let (username, set_username) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (failure, set_failure) = signal(Option::<String>::None);
    let (waiting, set_waiting) = signal(false);

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        set_waiting.set(true);
        set_failure.set(None);

        let (user, pass) = (username.get(), password.get());

        spawn_local(async move {
            match api::log_in(user, pass).await {
                Ok(identity) => on_in.run(identity),
                Err(why) => {
                    // Wrong credentials arrive as a 401 like an expired session
                    // does, and here it can only mean the first.
                    let said = match why {
                        Failure::Unreachable => t!("login.unreachable"),
                        _ => t!("login.failed"),
                    };
                    set_failure.set(Some(said.to_string()));
                    set_waiting.set(false);
                }
            }
        });
    };

    view! {
        <main class="entry">
            <form on:submit=submit>
                <h1>{t!("app.name")}</h1>

                <div class="field">
                    <label for="username">{t!("login.username")}</label>
                    <input
                        id="username"
                        name="username"
                        autocomplete="username"
                        autofocus
                        required
                        prop:value=username
                        on:input:target=move |e| set_username.set(e.target().value())
                    />
                </div>

                <div class="field">
                    <label for="password">{t!("login.password")}</label>
                    <input
                        id="password"
                        name="password"
                        type="password"
                        autocomplete="current-password"
                        required
                        prop:value=password
                        on:input:target=move |e| set_password.set(e.target().value())
                    />
                </div>

                <button type="submit" disabled=waiting>
                    {move || {
                        if waiting.get() { t!("login.working") } else { t!("login.submit") }
                    }}
                </button>

                {move || {
                    failure
                        .get()
                        .map(|why| view! { <p class="failure" role="alert">{why}</p> })
                }}
            </form>
        </main>
    }
}
