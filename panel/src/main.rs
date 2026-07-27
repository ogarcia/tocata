// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A skeleton of the panel, built to be measured rather than used.
//!
//! Two screens, chosen because between them they exercise everything the real
//! panel would do against `/api/v1`: a form that posts credentials and gets a
//! cookie back, and a view that reads JSON behind that cookie and draws it.
//!
//! The shapes come from the server itself, with everything that needs a database
//! or a socket switched off by a feature. Rename a field there and this stops
//! compiling, which is the entire reason the panel is written in Rust.

use leptos::prelude::*;
use leptos::task::spawn_local;
use tocata::types::{Credentials, Identity, Stats};

/// Everything is relative, since the panel is served by the server it talks to.
const API: &str = "/api/v1";

/// The cookie is `HttpOnly`, so the panel cannot read it and does not try. What
/// it does is ask who it is; a 401 means the form goes up.
async fn whoami() -> Option<Identity> {
    gloo_net::http::Request::get(&format!("{API}/session"))
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .send()
        .await
        .ok()
        .filter(|response| response.ok())?
        .json()
        .await
        .ok()
}

async fn log_in(username: String, password: String) -> Result<Identity, String> {
    let response = gloo_net::http::Request::post(&format!("{API}/session"))
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .json(&Credentials { username, password })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.ok() {
        return Err("Wrong username or password".to_string());
    }

    response.json().await.map_err(|e| e.to_string())
}

async fn stats() -> Result<Stats, String> {
    gloo_net::http::Request::get(&format!("{API}/stats"))
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

#[component]
fn LogIn(on_in: Callback<Identity>) -> impl IntoView {
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
            match log_in(user, pass).await {
                Ok(identity) => on_in.run(identity),
                Err(why) => set_failure.set(Some(why)),
            }
            set_waiting.set(false);
        });
    };

    view! {
        <form on:submit=submit>
            <h1>"Tocata"</h1>
            <input
                placeholder="Username"
                autofocus
                prop:value=username
                on:input:target=move |e| set_username.set(e.target().value())
            />
            <input
                type="password"
                placeholder="Password"
                prop:value=password
                on:input:target=move |e| set_password.set(e.target().value())
            />
            <button type="submit" disabled=waiting>
                {move || if waiting.get() { "…" } else { "Log in" }}
            </button>
            {move || failure.get().map(|why| view! { <p class="failure">{why}</p> })}
        </form>
    }
}

#[component]
fn Dashboard(identity: Identity) -> impl IntoView {
    let figures = LocalResource::new(stats);

    view! {
        <h1>"Tocata"</h1>
        <p>
            {identity.username.clone()}
            {if identity.admin { " (administrator)" } else { "" }}
        </p>
        <Suspense fallback=|| view! { <p>"Counting…"</p> }>
            {move || Suspend::new(async move {
                match figures.await {
                    Ok(s) => view! {
                        <table>
                            <tr><td>"Version"</td><td>{s.version}</td></tr>
                            <tr><td>"Artists"</td><td>{s.artists}</td></tr>
                            <tr><td>"Albums"</td><td>{s.albums}</td></tr>
                            <tr><td>"Tracks"</td><td>{s.tracks}</td></tr>
                            <tr><td>"Missing"</td><td>{s.missing}</td></tr>
                            <tr><td>"Accounts"</td><td>{s.users}</td></tr>
                            <tr><td>"Libraries"</td><td>{s.libraries}</td></tr>
                            <tr><td>"Bytes"</td><td>{s.total_size}</td></tr>
                        </table>
                    }.into_any(),
                    Err(why) => view! { <p class="failure">{why}</p> }.into_any(),
                }
            })}
        </Suspense>
    }
}

#[component]
fn Panel() -> impl IntoView {
    let (identity, set_identity) = signal(Option::<Identity>::None);
    let (asked, set_asked) = signal(false);

    // One question on load, so a reload with a live cookie lands on the
    // dashboard instead of asking for a password that is not needed.
    spawn_local(async move {
        set_identity.set(whoami().await);
        set_asked.set(true);
    });

    view! {
        <main>
            {move || {
                if !asked.get() {
                    return view! { <p>"…"</p> }.into_any();
                }
                match identity.get() {
                    Some(identity) => view! { <Dashboard identity /> }.into_any(),
                    None => view! {
                        <LogIn on_in=Callback::new(move |who| set_identity.set(Some(who))) />
                    }.into_any(),
                }
            }}
        </main>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(Panel);
}
