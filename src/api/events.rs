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

use super::error::ErrorBody;
use super::scan::Status;
use super::session::Panel;
use crate::scanner::{Progress, Snapshot};
use axum::extract::State;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use futures_util::stream::{Stream, unfold};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tracing::debug;

/// How often a comment is sent down an idle stream.
///
/// Not for us — we would notice a dead connection when we next tried to write —
/// but for whatever sits in between. A reverse proxy with a read timeout closes
/// a stream that has said nothing, and a scan that runs once a week says nothing
/// for a week.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Name the scan snapshot arrives under, so a panel can tell it from whatever
/// gets added next without inspecting the payload.
const SCAN_EVENT: &str = "scan";

/// What the stream is walking through: the updates, and the notice that the
/// server is going away.
struct Watching {
    updates: broadcast::Receiver<Snapshot>,
    shutdown: watch::Receiver<bool>,
}

/// Event stream
///
/// Stays open and reports what happens. Each event is named, and the only name so
/// far is `scan`, whose payload is exactly what `GET /scan` returns.
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
            description = "An open stream of named events",
            content_type = "text/event-stream",
            body = Status,
        ),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn stream(
    _panel: Panel,
    State(progress): State<Arc<Progress>>,
    State(shutdown): State<watch::Receiver<bool>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    // Subscribed before the first snapshot is taken, so an update landing between
    // the two is delivered rather than missed.
    let updates = progress.subscribe();
    let initial = progress.snapshot();

    let watching = Watching { updates, shutdown };

    let stream = unfold(Some((watching, Some(initial))), move |state| async move {
        let (mut watching, first) = state?;

        // A panel that connects in the middle of a scan wants to know that now,
        // not at the next tick.
        if let Some(snapshot) = first {
            return Some((event(snapshot.into()), Some((watching, None))));
        }

        loop {
            tokio::select! {
                _ = watching.shutdown.changed() => {
                    debug!("closing an event stream because the server is stopping");
                    return None;
                }
                received = watching.updates.recv() => match received {
                    Ok(snapshot) => return Some((event(snapshot.into()), Some((watching, None)))),
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

/// Serialisation cannot fail for these types, and a stream that ends because of
/// it would be worse than one that sends an empty payload.
fn event(status: Status) -> Result<Event, std::convert::Infallible> {
    Ok(Event::default()
        .event(SCAN_EVENT)
        .json_data(status)
        .unwrap_or_else(|_| Event::default().event(SCAN_EVENT)))
}
