// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The jobs somebody runs when something is off.
//!
//! Every row says three things and in this order: what the job would do, said as
//! the number it would affect; when it last ran and what it found then; and the
//! button. Read top to bottom that is a decision — what will happen, what
//! happened last time, go — and a button on its own would be none of those.
//!
//! Only the purge asks first. It is the one job here that cannot be undone, and a
//! confirmation in front of the other four would teach somebody to dismiss the
//! one in front of this one.

use crate::api::{self, Failure};
use crate::pages::{bytes, said, since, thousands};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{Job, JobState, Loss, Maintenance, Run};

#[component]
pub fn Maintenance(on_expired: Callback<()>) -> impl IntoView {
    let (state, set_state) = signal(Option::<Maintenance>::None);
    // What the database takes as a whole, which is the figure that makes the
    // reclaimable one mean something. It belongs to the collection's statistics
    // rather than to a job, so it comes from where that lives.
    let (occupied, set_occupied) = signal(Option::<i64>::None);
    let (failure, set_failure) = signal(Option::<String>::None);
    // Which job is running, since a POST that waits means the row has to say so
    // and nothing else may be started meanwhile.
    let (running, set_running) = signal(Option::<Job>::None);
    let (asking, set_asking) = signal(false);

    let load = move || {
        spawn_local(async move {
            match api::jobs().await {
                Ok(fresh) => set_state.set(Some(fresh)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(_) => set_failure.set(Some(t!("login.unreachable").to_string())),
            }
        });

        spawn_local(async move {
            if let Ok(stats) = api::stats().await {
                set_occupied.set(Some(stats.database_size));
            }
        });
    };

    load();

    let start = Callback::new(move |job: Job| {
        set_failure.set(None);
        set_running.set(Some(job));

        spawn_local(async move {
            match api::run_job(job).await {
                // Everything on the screen moves when a job runs — what the next
                // one would do as much as what this one did — so the answer is
                // read again rather than patched in.
                Ok(_) => load(),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => set_failure.set(Some(said(&why))),
            }

            set_running.set(None);
        });
    });

    view! {
        <header class="titled">
            <div>
                <h1>{t!("nav.maintenance")}</h1>
                <p class="quiet lead">{t!("chores.lead")}</p>
            </div>
        </header>

        {move || match state.get() {
            None => view! { <p class="quiet">{t!("common.loading")}</p> }.into_any(),
            Some(state) => {
                // Grouped rather than laid out one after another: the two that
                // take things away belong together, and so do the two that work on
                // the database file. As two boxes in the grid they are two columns
                // on a wide screen and still two groups when there is room for one
                // column, which one flat list of four could not manage.
                let (cleaning, database): (Vec<_>, Vec<_>) =
                    state.jobs.into_iter().partition(|state| cleans(state.job));

                view! {
                    <div class="chores">
                        {[cleaning, database]
                            .into_iter()
                            .map(|group| {
                                view! {
                                    <div class="group">
                                        {group
                                            .into_iter()
                                            .map(|job| {
                                                view! {
                                                    <Chore
                                                        job
                                                        occupied
                                                        running
                                                        start
                                                        set_asking
                                                    />
                                                }
                                            })
                                            .collect_view()}
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>

                    <Lately runs=state.lately />
                }
                    .into_any()
            }
        }}

        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}

        <Purging asking set_asking start />
    }
}

/// One job: what it would do, how it went last time, and the way to run it.
#[component]
fn Chore(
    job: JobState,
    occupied: ReadSignal<Option<i64>>,
    running: ReadSignal<Option<Job>>,
    start: Callback<Job>,
    set_asking: WriteSignal<bool>,
) -> impl IntoView {
    let which = job.job;
    let busy = move || running.get().is_some();
    let mine = move || running.get() == Some(which);

    // The figures line is read again when the size of the database lands, which
    // is a call of its own and arrives after this row is on screen. Written once,
    // the one row that uses that size would have kept the dash it was drawn with.
    let pending = job.pending;
    let last = job.last_run.clone();
    let figures = move || lately(which, pending, occupied, &last);

    // The purge is the one that cannot be undone, so it is the one that asks. The
    // dialogue is what runs it afterwards.
    let press = move |_| {
        if which == Job::Purge {
            set_asking.set(true);
        } else {
            start.run(which);
        }
    };

    view! {
        <div class="chore">
            <div>
                <span class="what">{name(which)}</span>
                <span class="why">{about(which, job.pending)}</span>
                <span class="ran">{figures}</span>

                // What a check found, or what stopped a job. Kept out of the line
                // above because it is prose and can run to several lines, and the
                // line above is a figure.
                {job
                    .last_run
                    .as_ref()
                    .and_then(|run| run.error.clone())
                    .map(|why| view! { <span class="wrong">{why}</span> })}
            </div>

            <button
                type="button"
                class="pill"
                disabled=busy
                on:click=press
            >
                {move || {
                    if mine() { t!("chores.running").to_string() } else { t!("chores.run").to_string() }
                }}
            </button>
        </div>
    }
}

/// The last few runs of anything, newest first.
///
/// Only what happened and when. Everything a row of the list above says about a
/// job is about the job; this is about the server, and it is the one place that
/// answers "did that ever actually run".
#[component]
fn Lately(runs: Vec<Run>) -> impl IntoView {
    view! {
        <h2 class="part">{t!("chores.lately")}</h2>

        {if runs.is_empty() {
            view! { <p class="quiet">{t!("chores.nothing_yet")}</p> }.into_any()
        } else {
            view! {
                <ul class="lately">
                    {runs
                        .into_iter()
                        .map(|run| {
                            view! {
                                <li>
                                    <span>{did(&run)}</span>
                                    <span class="when">{since(&run.at)}</span>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            }
                .into_any()
        }}
    }
}

/// The question in front of the purge, which is the only job that asks one.
///
/// What it reads out is not the number the row shows. The row says how many
/// tracks would go; this says what goes with them — the favourites, the ratings,
/// the plays — because those are the part a scan cannot bring back.
#[component]
fn Purging(
    asking: ReadSignal<bool>,
    set_asking: WriteSignal<bool>,
    start: Callback<Job>,
) -> impl IntoView {
    let dialog: NodeRef<Dialog> = NodeRef::new();
    let (loss, set_loss) = signal(Option::<Loss>::None);

    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        if asking.get() {
            set_loss.set(None);
            let _ = element.show_modal();

            spawn_local(async move {
                if let Ok(counted) = api::loss().await {
                    set_loss.set(Some(counted));
                }
            });
        } else {
            element.close();
        }
    });

    // In the order somebody would miss them.
    let losses = move || {
        let loss = loss.get()?;

        let counted = [
            (t!("chores.tracks").to_string(), loss.tracks),
            (t!("accounts.favourites").to_string(), loss.favourites),
            (t!("accounts.ratings").to_string(), loss.ratings),
            (t!("accounts.plays").to_string(), loss.played),
            (t!("chores.in_playlists").to_string(), loss.playlist_entries),
            (t!("accounts.bookmarks").to_string(), loss.bookmarks),
        ];

        Some(
            counted
                .into_iter()
                .filter(|(_, how_many)| *how_many > 0)
                .collect::<Vec<_>>(),
        )
    };

    view! {
        <dialog node_ref=dialog class="sheet" on:close=move |_| set_asking.set(false)>
            <div class="sheet-body">
                <h2>{t!("chores.purge_this")}</h2>
                <p class="sheet-lead">{t!("chores.purge_note")}</p>

                {move || match losses() {
                    None => ().into_any(),
                    Some(losses) if losses.is_empty() => {
                        view! { <p class="sheet-lead instead">{t!("chores.nothing_absent")}</p> }
                            .into_any()
                    }
                    Some(losses) => {
                        view! {
                            <dl class="facts">
                                {losses
                                    .into_iter()
                                    .map(|(what, how_many)| {
                                        view! {
                                            <div>
                                                <dt>{what}</dt>
                                                <dd>{thousands(how_many)}</dd>
                                            </div>
                                        }
                                    })
                                    .collect_view()}
                            </dl>
                        }
                            .into_any()
                    }
                }}
            </div>

            <div class="sheet-foot">
                <button type="button" class="away" on:click=move |_| set_asking.set(false)>
                    {t!("common.cancel")}
                </button>
                <button
                    type="button"
                    class="pill solid undoing"
                    on:click=move |_| {
                        set_asking.set(false);
                        start.run(Job::Purge);
                    }
                >
                    {t!("chores.purge_yes")}
                </button>
            </div>
        </dialog>
    }
}

/// Which of the two groups a job belongs to: the ones that take something away,
/// or the ones that work on the database file itself.
///
/// A match over every job rather than a list of the ones that clean, so that a
/// job added later cannot quietly land in whichever group the code happened to
/// default to — it will not compile until somebody says where it goes.
fn cleans(job: Job) -> bool {
    match job {
        Job::Purge | Job::Covers => true,
        Job::Compact | Job::Check => false,
    }
}

/// What the job is called.
fn name(job: Job) -> String {
    match job {
        Job::Purge => t!("chores.purge").to_string(),
        Job::Compact => t!("chores.compact").to_string(),
        Job::Covers => t!("chores.covers").to_string(),
        Job::Check => t!("chores.check").to_string(),
    }
}

/// What it will do, with the number it will do it to in the sentence.
///
/// Two sentences per job rather than one with a count interpolated into it: with
/// nothing to do, "removes the 0 tracks whose files are gone" is a sentence
/// nobody would write.
fn about(job: Job, pending: Option<i64>) -> String {
    let some = pending.is_some_and(|how_many| how_many > 0);
    let how_many = pending.map(thousands).unwrap_or_default();

    match (job, some) {
        (Job::Purge, true) => t!("chores.purge_why", count = how_many).to_string(),
        (Job::Purge, false) => t!("chores.purge_idle").to_string(),
        (Job::Compact, true) => t!("chores.compact_why").to_string(),
        (Job::Compact, false) => t!("chores.compact_idle").to_string(),
        (Job::Covers, true) => t!("chores.covers_why", count = how_many).to_string(),
        (Job::Covers, false) => t!("chores.covers_idle").to_string(),
        (Job::Check, _) => t!("chores.check_why").to_string(),
    }
}

/// The figures line: when it last ran and what it found, or what the database
/// measures for the one job whose subject is the file itself.
fn lately(
    job: Job,
    pending: Option<i64>,
    occupied: ReadSignal<Option<i64>>,
    last: &Option<Run>,
) -> String {
    // Compacting is about the file, so its own figures say more than a date: what
    // it takes now, and what is sitting in it unused.
    if job == Job::Compact {
        let size = occupied
            .get()
            .map(bytes)
            .unwrap_or_else(|| super::MISSING.to_string());

        return match pending {
            Some(free) if free > 0 => {
                t!("chores.size_and_free", size = size, free = bytes(free)).to_string()
            }
            _ => t!("chores.size_only", size = size).to_string(),
        };
    }

    match last {
        None => t!("chores.never").to_string(),
        Some(run) => format!(
            "{} · {}",
            t!("chores.ran", when = since(&run.at)),
            found(run)
        ),
    }
}

/// What a run came to, in the terms of the job that ran.
fn found(run: &Run) -> String {
    let count = thousands(run.affected);

    match run.job {
        Job::Purge => t!("chores.removed", count = count).to_string(),
        Job::Compact => t!("chores.reclaimed", size = bytes(run.affected)).to_string(),
        Job::Covers => t!("chores.deleted", count = count).to_string(),
        Job::Check if run.affected == 0 => t!("chores.sound").to_string(),
        Job::Check => t!("chores.problems", count = count).to_string(),
    }
}

/// A line of the history: the job in the past tense, and what it came to.
fn did(run: &Run) -> String {
    format!("{} · {}", name(run.job), found(run))
}
