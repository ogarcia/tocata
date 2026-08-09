// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! One stream the panel keeps open, for everything that happens without being
//! asked about.
//!
//! Server sent events rather than a WebSocket. Everything here goes one way —
//! the server tells, the panel listens — and the panel already has a perfectly
//! good way to ask questions, which is the rest of this API. A WebSocket would
//! buy a channel back that nothing needs, at the price of a protocol upgrade
//! that reverse proxies have to be told about and a reconnection loop we would
//! have to write ourselves. `EventSource` reconnects on its own.
//!
//! Two kinds of thing travel here, and they arrive for different reasons. A scan
//! update is sent because something happened; the resource figures are sent
//! because time passed. The second kind is why this samples on a timer, and why it
//! samples only while somebody is listening: a server nobody is watching should
//! be a server doing nothing.

use super::session::Panel;
use crate::resources::Meter;
use crate::scanner::{Progress, Snapshot};
use crate::types::{ErrorBody, Resources, Status};
use axum::extract::State;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use futures_util::stream::{Stream, unfold};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::time::{Interval, MissedTickBehavior, interval};
use tracing::{debug, warn};

/// How often a comment is sent down an idle stream.
///
/// Not for us — we would notice a dead connection when we next tried to write —
/// but for whatever sits in between. A reverse proxy with a read timeout closes
/// a stream that has said nothing, and a scan that runs once a week says nothing
/// for a week.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// How often the resource figures are taken and sent.
///
/// A meter is read at a glance, so it has to move soon enough to be believed and
/// slowly enough to be legible. Two seconds is both, and it is also the window the
/// processor share is averaged over, which is short enough to show a scan starting
/// and long enough not to be mostly noise.
const RESOURCES_EVERY: Duration = Duration::from_secs(2);

/// Name the scan snapshot arrives under, so a panel can tell it from whatever
/// gets added next without inspecting the payload.
const SCAN_EVENT: &str = "scan";

/// Likewise for the resource figures.
const RESOURCES_EVENT: &str = "resources";

/// And for how the walk out for artist portraits is going.
///
/// Only the part that moves. What that walk would find if it started — the
/// setting and how many artists are without a picture — comes out of the
/// database, and sending it once a second unchanged would make watching the walk
/// cost more than the walk.
const PORTRAITS_EVENT: &str = "portraits";

/// What the stream is walking through: the updates, the clock, and the notice
/// that the server is going away.
struct Watching {
    updates: broadcast::Receiver<Snapshot>,
    portraits: broadcast::Receiver<crate::portraits::Snapshot>,
    shutdown: watch::Receiver<bool>,
    meter: Arc<Meter>,
    clock: Interval,
    /// Handed over before anything is waited for, so a panel that has just
    /// connected does not start out blank.
    pending: VecDeque<Result<Event, Infallible>>,
}

/// Event stream
///
/// Stays open and reports what happens. Each event is named, and the name says
/// what the payload is:
///
/// - `scan` — exactly what `GET /scan` returns, sent when a scan gets anywhere.
/// - `resources` — exactly what `GET /resources` returns, sent every couple of
///   seconds for as long as the stream is open.
/// - `portraits` — the `run` half of what `GET /portraits` returns, sent when the
///   walk for artist portraits gets anywhere. Nothing at all while none is going,
///   which is nearly always.
///
/// The stream carries whole states, never deltas, so a client that falls behind
/// or reconnects has nothing to catch up on: whichever event it reads next is
/// complete on its own.
#[utoipa::path(
    get,
    path = "/events",
    tag = "events",
    responses(
        (
            status = 200,
            description = "An open stream of named events, each payload named by its event",
            content_type = "text/event-stream",
            body = Status,
        ),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn stream(
    _panel: Panel,
    State(progress): State<Arc<Progress>>,
    State(fetching): State<Arc<crate::portraits::Fetching>>,
    State(meter): State<Arc<Meter>>,
    State(shutdown): State<watch::Receiver<bool>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribed before the first snapshot is taken, so an update landing between
    // the two is delivered rather than missed.
    let updates = progress.subscribe();
    let portraits = fetching.subscribe();

    // A panel that connects in the middle of a scan wants to know that now, not at
    // the next tick. The figures need no such treatment: the first tick of an
    // interval completes immediately, so they are on their way already.
    let mut pending = VecDeque::new();
    pending.push_back(scan_event(progress.snapshot().into()));

    // And likewise for a walk already going, which is the longer of the two
    // things worth arriving in the middle of.
    if fetching.is_fetching() {
        pending.push_back(portraits_event(fetching.snapshot().into()));
    }

    let mut clock = interval(RESOURCES_EVERY);
    // A stream that fell behind wants the next reading, not the queue of readings
    // it missed. They would all be sent at once and none of them would be now.
    clock.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let watching = Watching {
        updates,
        portraits,
        shutdown,
        meter,
        clock,
        pending,
    };

    let stream = unfold(Some(watching), move |state| async move {
        let mut watching = state?;

        if let Some(next) = watching.pending.pop_front() {
            return Some((next, Some(watching)));
        }

        loop {
            tokio::select! {
                _ = watching.shutdown.changed() => {
                    debug!("closing an event stream because the server is stopping");
                    return None;
                }
                _ = watching.clock.tick() => match watching.meter.read() {
                    Ok(figures) => return Some((
                        resources_event(figures),
                        Some(watching),
                    )),
                    // Worth saying once, and not worth ending a stream that is
                    // also carrying scan progress. The next tick tries again.
                    Err(e) => {
                        warn!("cannot read what this process is using: {e}");
                        continue;
                    }
                },
                received = watching.portraits.recv() => match received {
                    Ok(snapshot) => return Some((
                        portraits_event(snapshot.into()),
                        Some(watching),
                    )),
                    // Same as below: every update is the whole state, so the one
                    // after the gap says everything the missed ones would have.
                    Err(RecvError::Lagged(missed)) => {
                        debug!("an event stream fell {missed} portrait updates behind");
                        continue;
                    }
                    // The walk's progress lives as long as the process, so this
                    // cannot happen — and if it did, it is no reason to close a
                    // stream that is also carrying a scan.
                    Err(RecvError::Closed) => continue,
                },
                received = watching.updates.recv() => match received {
                    Ok(snapshot) => return Some((
                        scan_event(snapshot.into()),
                        Some(watching),
                    )),
                    // Skipped updates cost nothing: the next one carries the
                    // whole state, which is why the channel is shallow.
                    Err(RecvError::Lagged(missed)) => {
                        debug!("an event stream fell {missed} updates behind");
                        continue;
                    }
                    Err(RecvError::Closed) => return None,
                },
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}

fn scan_event(status: Status) -> Result<Event, Infallible> {
    named(SCAN_EVENT, status)
}

fn portraits_event(run: crate::types::PortraitRun) -> Result<Event, Infallible> {
    named(PORTRAITS_EVENT, run)
}

fn resources_event(figures: Resources) -> Result<Event, Infallible> {
    named(RESOURCES_EVENT, figures)
}

/// Serialisation cannot fail for these types, and a stream that ended because of
/// it would be worse than one that sent an empty payload.
fn named<T: serde::Serialize>(name: &str, payload: T) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event(name)
        .json_data(payload)
        .unwrap_or_else(|_| Event::default().event(name)))
}
