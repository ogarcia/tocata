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
use crate::pages::home::Counted;
use crate::pages::{bytes, elapsed, on_day, said, since, thousands};
use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::{
    Job, JobState, Loss, Maintenance, MissingTrack, NeedingAttention, PortraitRun, Run,
};

#[component]
pub fn Maintenance(on_expired: Callback<()>) -> impl IntoView {
    let (state, set_state) = signal(Option::<Maintenance>::None);
    // The figures the whole panel holds, which this screen both reads from and moves.
    // What the database takes as a whole is one of them — the figure that makes the
    // reclaimable one mean something — and what a job removes moves several more.
    let counted = use_context::<Counted>().expect("the figures are read above the router");
    let occupied = counted.database_size();
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
                //
                // And so are the figures, which are not this screen's: a purge
                // takes tracks out of the collection and a compact changes what
                // the file weighs, and until this was here the Overview went on
                // sending somebody back here for missing tracks that were gone.
                // Every job but the check, which reads the database and leaves it
                // exactly as it was — and is the slowest of them to boot.
                Ok(_) => {
                    load();

                    if job != Job::Check {
                        counted.read();
                    }
                }
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
                // take things away belong together, the two that work on the
                // database file belong together, and what reaches the network is
                // its own thing entirely. As boxes in the grid they are columns on
                // a wide screen and still groups when there is room for one
                // column, which one flat list could not manage.
                let mut cleaning = Vec::new();
                let mut database = Vec::new();
                let mut outward = Vec::new();

                for job in state.jobs {
                    match band(job.job) {
                        Band::Cleaning => cleaning.push(job),
                        Band::Database => database.push(job),
                        Band::Outward => outward.push(job),
                    }
                }

                let chores = move |group: Vec<JobState>| {
                    group
                        .into_iter()
                        .map(|job| view! { <Chore job occupied running start set_asking /> })
                        .collect_view()
                };

                view! {
                    <div class="chores">
                        <div class="group">{chores(cleaning)}</div>
                        <div class="group">{chores(database)}</div>

                        // The walk out for pictures and the way to throw away what
                        // it brought back, in one box: they are the two halves of
                        // the same decision, and neither is a job of the kind the
                        // other four are.
                        <div class="group">
                            <Portraits on_expired />
                            {chores(outward)}
                        </div>
                    </div>

                    <Attention on_expired />
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
    occupied: Signal<Option<i64>>,
    running: ReadSignal<Option<Job>>,
    start: Callback<Job>,
    set_asking: WriteSignal<bool>,
) -> impl IntoView {
    let which = job.job;
    let busy = move || running.get().is_some();
    let mine = move || running.get() == Some(which);

    // The figures line is read again when the size of the database changes, which
    // it does under this row: a compact gives space back, and the figure comes from
    // the panel's own statistics rather than from this screen. Written once, the one
    // row that uses that size would have kept whatever it was drawn with.
    let pending = job.pending;
    let last = job.last_run.clone();
    let figures = move || lately(which, pending, occupied, &last);
    let look_again = job.look_again;

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
                <span class="why">{about(which, job.pending, look_again)}</span>
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

/// Looking for pictures of the artists: whether the server may, how many are
/// without one, and the way to set it going or stop it.
///
/// Its own block rather than a fifth job, because it is not one. A job runs
/// inside the request that asked for it and is over in seconds; this is a walk
/// out to two other people's servers at one request a second, and the useful
/// thing on screen is not "run it" but where it has got to.
///
/// **Watched rather than asked after.** Where it has got to arrives on the
/// stream the panel already keeps open, so a walk that says nothing sends
/// nothing — the alternative was a request every two seconds for three quarters
/// of an hour to be told the same number most of the time. The two figures the
/// stream does not carry are the ones that do not move while it runs: whether
/// the server may look at all, and how many artists are still without a picture.
/// Those are asked for once, and again when a walk ends, which is the only
/// moment either can have changed.
#[component]
fn Portraits(on_expired: Callback<()>) -> impl IntoView {
    let state = RwSignal::new(None::<tocata::types::Portraits>);
    let failure = RwSignal::new(None::<String>);

    let look = Callback::new(move |()| {
        spawn_local(async move {
            match api::portraits().await {
                Ok(fresh) => state.set(Some(fresh)),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => failure.set(Some(said(&why))),
            }
        });
    });

    look.run(());

    // What the stream says, folded into what was asked for. The run it carries
    // replaces the one in hand; the two figures beside it stay until the walk
    // ends, and then the whole answer is asked for again — that is the moment
    // "how many are without a picture" has a new answer.
    let live = use_context::<ReadSignal<Option<PortraitRun>>>();
    if let Some(live) = live {
        Effect::new(move |was_fetching: Option<bool>| {
            let Some(run) = live.get() else {
                return was_fetching.unwrap_or(false);
            };

            let going = run.fetching;
            state.update(|held| {
                if let Some(held) = held {
                    held.run = run;
                }
            });

            // Just stopped, whether it finished or was told to. Nothing else
            // moves those two figures while a panel is open on this screen.
            if was_fetching == Some(true) && !going {
                look.run(());
            }

            going
        });
    }

    let press = move |_| {
        let going = state.with(|read| read.as_ref().is_some_and(|read| read.run.fetching));
        failure.set(None);

        spawn_local(async move {
            let asked = if going {
                api::stop_portraits().await
            } else {
                api::start_portraits().await
            };

            match asked {
                Ok(()) => look.run(()),
                Err(Failure::Unauthenticated) => on_expired.run(()),
                Err(why) => failure.set(Some(said(&why))),
            }
        });
    };

    view! {
        {move || {
            state
                .get()
                .map(|read| {
                    let going = read.run.fetching;
                    let allowed = read.allowed;

                    view! {
                        <div class="chore">
                            <div>
                                <span class="what">{t!("portraits.title")}</span>
                                <span class="why">{about_portraits(&read)}</span>
                                <span class="ran">{lately_portraits(&read.run)}</span>

                                {read
                                    .run
                                    .failure
                                    .clone()
                                    .map(|why| view! { <span class="wrong">{why}</span> })}
                            </div>

                            // No button where the setting is off. What would
                            // happen is a refusal, and a button whose whole
                            // behaviour is to be refused is a button that should
                            // not be there — the line above says where the switch
                            // is instead.
                            <Show when=move || allowed>
                                <button type="button" class="pill" on:click=press>
                                    {if going {
                                        t!("portraits.stop").to_string()
                                    } else {
                                        t!("portraits.start").to_string()
                                    }}
                                </button>
                            </Show>
                        </div>
                    }
                })
        }}

        {move || failure.get().map(|why| view! { <p class="failure" role="alert">{why}</p> })}
    }
}

/// What it would do, or what it is doing — which are different sentences.
fn about_portraits(read: &tocata::types::Portraits) -> String {
    if !read.allowed {
        return t!("portraits.not_allowed").to_string();
    }

    if read.run.fetching {
        return match &read.run.artist {
            Some(artist) => t!("portraits.looking_at", artist = artist).to_string(),
            None => t!("portraits.starting").to_string(),
        };
    }

    match read.wanting {
        0 => t!("portraits.nobody_wanting").to_string(),
        1 => t!("portraits.one_wanting").to_string(),
        count => t!("portraits.many_wanting", count = thousands(count as i64)).to_string(),
    }
}

/// Where it has got to, or where the last one got to. Empty before anything has
/// ever run, which is the one state with nothing to say.
fn lately_portraits(read: &PortraitRun) -> String {
    if read.started_at.is_none() {
        return String::new();
    }

    let counted = t!(
        "portraits.got_through",
        done = thousands(read.done as i64),
        total = thousands(read.total as i64),
        found = thousands(read.found as i64)
    )
    .to_string();

    if read.fetching {
        return counted;
    }

    match (read.cancelled, read.finished_at.as_deref()) {
        (true, _) => format!("{counted} · {}", t!("portraits.stopped")),
        (false, Some(when)) => format!("{counted} · {}", since(when)),
        (false, None) => counted,
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

/// The files a scan could not account for, named.
///
/// Both counts existed before this did and neither could be opened: the last scan
/// says how many would not read and the collection says how many are gone, and
/// somebody who wanted to do something about either had to go to the log. A count
/// that names a problem should be a way into it.
///
/// Two lists rather than one, because they look alike in a count and are opposite
/// problems. An unreadable file is on the disk and its music is not in the
/// collection: the row leads with the path, because the path is where somebody is
/// going next, and it owes an explanation. A missing track is in the collection and
/// its file is not on the disk: the row leads with the track, because Tocata still
/// remembers it, and what makes the decision hard is what would go with it — so that
/// is a column and not small print inside a confirmation.
///
/// Nothing here has a button. Fixing an unreadable file happens outside Tocata and
/// the next scan picks it up on its own; forgetting the missing ones is the purge,
/// which is four rows further up this same screen and asks first.
///
/// Loaded on its own rather than with the jobs. It is the slower of the two answers
/// and the one further down the screen, and a screen that waits for it is a screen
/// that waits to show the buttons.
#[component]
fn Attention(on_expired: Callback<()>) -> impl IntoView {
    let (found, set_found) = signal(Option::<NeedingAttention>::None);

    spawn_local(async move {
        match api::needing_attention().await {
            Ok(fresh) => set_found.set(Some(fresh)),
            Err(Failure::Unauthenticated) => on_expired.run(()),
            // Silent, and the only place in this file that is. The jobs above are
            // the screen; this is an extra that appears when there is something to
            // say, and a red line about a listing nobody asked for would be the
            // loudest thing on a screen where nothing is wrong.
            Err(_) => {}
        }
    });

    view! {
        {move || {
            // Absent entirely while there is nothing wrong. A heading over two empty
            // lists is a screen inviting somebody to look for a problem they do not
            // have.
            let Some(found) = found.get().filter(|found| {
                found.unreadable_total.is_positive() || found.missing_total.is_positive()
            }) else {
                return ().into_any();
            };

            let NeedingAttention {
                unreadable,
                missing,
                unreadable_total,
                missing_total,
                grace_days,
            } = found;

            let all = unreadable_total + missing_total;

            view! {
                <h2 class="part">{t!("attention.heading")}</h2>
                <p class="quiet lead">{t!("attention.lead", count = all)}</p>

                {unreadable_total
                    .is_positive()
                    .then(|| view! { <Unreadable files=unreadable total=unreadable_total /> })}

                {missing_total
                    .is_positive()
                    .then(|| {
                        view! { <Astray tracks=missing total=missing_total grace=grace_days /> }
                    })}
            }
                .into_any()
        }}
    }
}

/// The files that would not open: path, size, and why.
#[component]
fn Unreadable(files: Vec<tocata::types::UnreadableFile>, total: i64) -> impl IntoView {
    let shown = files.len() as i64;

    view! {
        <p class="lettering">{t!("attention.unreadable", count = total)}</p>
        <p class="quiet">{t!("attention.unreadable_why")}</p>

        <ul class="shut">
            {files
                .into_iter()
                .map(|file| {
                    // The reason and how long it has been going on, in one sentence.
                    // Two lines would put the date on a line of its own, where it
                    // reads as a fact about the file rather than about the failure.
                    let failing = t!("attention.failing_since", when = on_day(&file.since));
                    let said = match file.why {
                        Some(why) => format!("{why}. {failing}"),
                        None => failing.to_string(),
                    };

                    view! {
                        <li>
                            <span class="path">{file.path}</span>
                            <span class="weighs">{bytes(file.size)}</span>
                            <span class="wrong">{said}</span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>

        <Rest shown total />
    }
}

/// The tracks whose files are gone: what they are, when they went, and what they
/// hold.
#[component]
fn Astray(tracks: Vec<MissingTrack>, total: i64, grace: Option<i64>) -> impl IntoView {
    let shown = tracks.len() as i64;

    view! {
        <p class="lettering">{t!("attention.astray", count = total)}</p>
        <p class="quiet">{t!("attention.astray_why")}</p>

        // Scrolls inside its own box rather than pushing the page sideways, like
        // every other run of rows in this panel that cannot narrow any further.
        <div class="scrolls">
            <ul class="astray">
                <li class="astray-head">
                    <span>{t!("attention.track")}</span>
                    <span>{t!("attention.last_seen")}</span>
                    <span>{t!("attention.held")}</span>
                </li>
                {tracks
                    .into_iter()
                    .map(|track| view! { <Gone track grace /> })
                    .collect_view()}
            </ul>
        </div>

        <Rest shown total />

        // The line that prevents the expensive mistake. A collection that was moved
        // looks exactly like a collection that was deleted, and the difference is one
        // scan away.
        <p class="quiet moved">{t!("attention.moved")}</p>
    }
}

/// One track that is no longer where it was.
#[component]
fn Gone(track: MissingTrack, grace: Option<i64>) -> impl IntoView {
    // What it would take with it, in the terms somebody weighs: plays are everybody's
    // added up, because on a server with more than one account there is no other
    // honest number, and a rating is said as how many people wrote one rather than as
    // a score that would be one of theirs.
    let mut held = Vec::new();
    if track.plays > 0 {
        held.push(t!("attention.plays", count = thousands(track.plays)).to_string());
    }
    if track.raters > 0 {
        held.push(t!("attention.rated_by", count = track.raters).to_string());
    }
    if track.playlists > 0 {
        held.push(t!("attention.in_playlists", count = track.playlists).to_string());
    }

    // Nothing rather than an empty cell: a track nobody ever played is the easy case
    // and the row should say so out loud.
    let held = if held.is_empty() {
        t!("attention.held_nothing").to_string()
    } else {
        held.join(" · ")
    };

    // How long is left before a scan clears it out, where a limit is set at all. Days
    // and not hours: the setting is in days, and an hour's precision on a month-long
    // wait is precision nobody asked for.
    let leaving = grace.and_then(|days| {
        let gone_for = elapsed(&track.since)? / 86_400.0;
        let left = (days as f64 - gone_for).ceil();

        Some(if left <= 0.0 {
            t!("attention.dropped_next_scan").to_string()
        } else {
            t!("attention.dropped_in", count = left).to_string()
        })
    });

    let by = [track.artist, track.album, track.year.map(|y| y.to_string())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

    view! {
        <li>
            <span class="what">
                <span class="titling">{track.title}</span>
                <span class="quiet by">{by}</span>
                <span class="quiet path">{track.path}</span>
            </span>
            <span class="when">
                {since(&track.since)}
                {leaving.map(|soon| view! { <span class="soon">{soon}</span> })}
            </span>
            <span class="quiet holds">{held}</span>
        </li>
    }
}

/// How many did not fit, where any did not.
///
/// The lists are cut short on purpose and a list that quietly stopped at fifty would
/// read as a collection with exactly fifty problems.
#[component]
fn Rest(shown: i64, total: i64) -> impl IntoView {
    let more = total - shown;

    view! {
        <Show when=move || more.is_positive()>
            <p class="quiet elsewhere">
                {move || t!("attention.and_more", count = more).to_string()}
            </p>
        </Show>
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
/// Which box a job belongs in.
enum Band {
    /// Takes something away that is no longer wanted.
    Cleaning,
    /// Works on the database file itself.
    Database,
    /// Has to do with what came off somebody else's server.
    Outward,
}

fn band(job: Job) -> Band {
    match job {
        Job::Purge | Job::Covers => Band::Cleaning,
        Job::Compact | Job::Check => Band::Database,
        Job::Forget => Band::Outward,
    }
}

/// What the job is called.
fn name(job: Job) -> String {
    match job {
        Job::Purge => t!("chores.purge").to_string(),
        Job::Compact => t!("chores.compact").to_string(),
        Job::Covers => t!("chores.covers").to_string(),
        Job::Check => t!("chores.check").to_string(),
        Job::Forget => t!("chores.forget").to_string(),
    }
}

/// What it will do, with the number it will do it to in the sentence.
///
/// Two sentences per job rather than one with a count interpolated into it: with
/// nothing to do, "removes the 0 tracks whose files are gone" is a sentence
/// nobody would write.
fn about(job: Job, pending: Option<i64>, look_again: Option<i64>) -> String {
    let some = pending.is_some_and(|how_many| how_many > 0);
    let how_many = pending.map(thousands).unwrap_or_default();

    // The one job that does two things says both, each with its own figure and
    // each left out when it is nought. A total of the two would be a number of
    // nothing: files that are rubbish added to answers that are true.
    if job == Job::Covers {
        let stale = look_again.unwrap_or_default();

        let said: Vec<String> = [
            some.then(|| t!("chores.covers_files", count = how_many).to_string()),
            (stale > 0).then(|| t!("chores.covers_again", count = thousands(stale)).to_string()),
        ]
        .into_iter()
        .flatten()
        .collect();

        return match said.is_empty() {
            true => t!("chores.covers_idle").to_string(),
            false => said.join(" "),
        };
    }

    match (job, some) {
        (Job::Purge, true) => t!("chores.purge_why", count = how_many).to_string(),
        (Job::Purge, false) => t!("chores.purge_idle").to_string(),
        (Job::Compact, true) => t!("chores.compact_why").to_string(),
        (Job::Compact, false) => t!("chores.compact_idle").to_string(),
        // Said above, because it is the one with two figures.
        (Job::Covers, _) => unreachable!("covers answers before this"),
        (Job::Check, _) => t!("chores.check_why").to_string(),
        (Job::Forget, true) => t!("chores.forget_why", count = how_many).to_string(),
        (Job::Forget, false) => t!("chores.forget_idle").to_string(),
    }
}

/// The figures line: when it last ran and what it found, or what the database
/// measures for the one job whose subject is the file itself.
fn lately(
    job: Job,
    pending: Option<i64>,
    occupied: Signal<Option<i64>>,
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
        Job::Forget => t!("chores.forgotten", count = count).to_string(),
        Job::Check if run.affected == 0 => t!("chores.sound").to_string(),
        Job::Check => t!("chores.problems", count = count).to_string(),
    }
}

/// A line of the history: the job in the past tense, and what it came to.
fn did(run: &Run) -> String {
    format!("{} · {}", name(run.job), found(run))
}
