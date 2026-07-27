// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Watching a scan, and steering it.
//!
//! Nothing here polls. The figures come off the event stream, which the server
//! throttles to four updates a second, so this screen is as current as the scan
//! is and asks for nothing.
//!
//! What it shows while a scan runs and what it shows afterwards are the same
//! fields: the server keeps the last run's figures once it finishes, so a screen
//! opened later still says how it went.

use crate::api::{self, Failure};
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::Status;

#[component]
pub fn Scan(status: ReadSignal<Option<Status>>, admin: bool) -> impl IntoView {
    let (failure, set_failure) = signal(Option::<String>::None);

    // Set while a request is in flight, so a second click cannot start a second
    // scan before the first answer has arrived.
    let (asking, set_asking) = signal(false);

    let command = move |full: Option<bool>| {
        set_asking.set(true);
        set_failure.set(None);

        spawn_local(async move {
            let outcome = match full {
                Some(full) => api::start_scan(full).await,
                None => api::cancel_scan().await,
            };

            if let Err(why) = outcome {
                set_failure.set(Some(match why {
                    Failure::Unreachable => t!("login.unreachable").to_string(),
                    _ => t!("scan.refused").to_string(),
                }));
            }
            set_asking.set(false);
        });
    };

    view! {
        <h1>{t!("scan.heading")}</h1>

        {move || match status.get() {
            // Before the first message off the stream. Not the same as "no scan
            // is running", which is why it says nothing rather than nothing much.
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(status) => {
                let running = status.scanning;
                view! {
                    <p class="state">
                        {if running {
                            view! {
                                <span class="working">
                                    <Glyph icon=Icon::Scan />
                                    {t!("scan.running")}
                                </span>
                            }
                                .into_any()
                        } else if status.cancelled {
                            view! { <span class="quiet">{t!("scan.cancelled")}</span> }.into_any()
                        } else if status.finished_at.is_some() {
                            view! { <span class="quiet">{t!("scan.idle")}</span> }.into_any()
                        } else {
                            view! { <span class="quiet">{t!("scan.never")}</span> }.into_any()
                        }}
                    </p>

                    <Figures status=status.clone() />

                    // Only an administrator may start or stop one. Everybody else
                    // can watch, which is why this screen is not restricted.
                    <Show when=move || admin>
                        <p class="row">
                            {move || {
                                if running {
                                    view! {
                                        <button
                                            disabled=asking
                                            on:click=move |_| command(None)
                                        >
                                            {t!("scan.cancel")}
                                        </button>
                                    }
                                        .into_any()
                                } else {
                                    view! {
                                        <button
                                            disabled=asking
                                            on:click=move |_| command(Some(false))
                                        >
                                            {t!("scan.start")}
                                        </button>
                                        <button
                                            class="second"
                                            disabled=asking
                                            on:click=move |_| command(Some(true))
                                        >
                                            {t!("scan.start_full")}
                                        </button>
                                    }
                                        .into_any()
                                }
                            }}
                        </p>
                    </Show>
                }
                    .into_any()
            }
        }}

        {move || {
            failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })
        }}
    }
}

#[component]
fn Figures(status: Status) -> impl IntoView {
    // Where it has got to, which is the one field that says something is
    // happening rather than how much has happened.
    let where_ = status.path.clone().or_else(|| status.library.clone());

    view! {
        <table class="figures">
            <tbody>
                <tr><th>{t!("scan.folders")}</th><td>{status.folders}</td></tr>
                <tr><th>{t!("scan.tracks")}</th><td>{status.tracks}</td></tr>
                <tr><th>{t!("scan.unchanged")}</th><td>{status.unchanged}</td></tr>
                <tr><th>{t!("scan.failed")}</th><td>{status.failed}</td></tr>
                <tr><th>{t!("scan.gone")}</th><td>{status.gone}</td></tr>
                {status
                    .started_at
                    .map(|at| view! { <tr><th>{t!("scan.started")}</th><td>{at}</td></tr> })}
                {status
                    .finished_at
                    .map(|at| view! { <tr><th>{t!("scan.finished")}</th><td>{at}</td></tr> })}
            </tbody>
        </table>

        {where_.map(|where_| view! { <p class="path quiet">{where_}</p> })}
    }
}
