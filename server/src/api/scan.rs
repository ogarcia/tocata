// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Watching and steering a scan.
//!
//! `/rest` has `getScanStatus`, and it reports two things because that is all the
//! specification defines. This says everything the scanner knows, which is the
//! difference between a spinner and a panel.

use super::error::ApiError;
use super::session::{Administrator, Panel};
use crate::scanner::{self, Progress, Snapshot};
use crate::state::AppState;
use crate::types::{ErrorBody, Status};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use std::sync::Arc;
use tracing::{error, info};
use utoipa::IntoParams;

impl From<Snapshot> for Status {
    fn from(snapshot: Snapshot) -> Self {
        Self {
            scanning: snapshot.scanning,
            library: snapshot.library,
            path: snapshot.path,
            folders: snapshot.folders,
            tracks: snapshot.tracks,
            unchanged: snapshot.unchanged,
            failed: snapshot.failed,
            gone: snapshot.gone,
            started_at: snapshot.started_at,
            finished_at: snapshot.finished_at,
            cancelled: snapshot.cancelled,
        }
    }
}

/// What kind of scan to run.
#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct StartQuery {
    /// Read every file again instead of skipping the ones whose size and
    /// modification time have not moved. For when tags were edited with their
    /// timestamps preserved, or when our own reading of them improved.
    #[serde(default)]
    full: bool,
}

/// Scan status
///
/// Everything the scanner knows about the run in flight, or about the last one to
/// finish. The same values arrive unprompted on the event stream, which is the
/// better way to follow a scan; this is for the first paint.
#[utoipa::path(
    get,
    path = "/scan",
    tag = "scan",
    responses(
        (status = 200, description = "How the scan is going", body = Status),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn status(_panel: Panel, State(progress): State<Arc<Progress>>) -> Json<Status> {
    Json(progress.snapshot().into())
}

/// Start a scan
///
/// Answers as soon as the scan has been accepted rather than when it finishes,
/// so follow it on the event stream. Deliberately with no body: at the moment
/// this returns, the scan has been spawned but may not have claimed its first
/// counter yet, and a snapshot taken here would report the run before it.
///
/// A second request while one is running changes nothing and says so.
#[utoipa::path(
    post,
    path = "/scan",
    tag = "scan",
    params(StartQuery),
    responses(
        (status = 202, description = "The scan has been accepted; watch /events"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 409, description = "A scan is already running", body = ErrorBody),
    )
)]
pub async fn start(
    _admin: Administrator,
    State(state): State<AppState>,
    Query(query): Query<StartQuery>,
) -> Result<StatusCode, ApiError> {
    // Asked before spawning so the answer is honest. The scanner refuses a second
    // run of its own accord, but it does so after the task has been created, and
    // by then this handler has already replied.
    if state.scan.is_scanning() {
        return Err(ApiError::Conflict("A scan is already running"));
    }

    let mode = if query.full {
        scanner::Mode::Full
    } else {
        scanner::Mode::Incremental
    };

    let spawned = state.clone();
    tokio::spawn(async move {
        match scanner::scan_all(&spawned.pool, mode, &spawned.scan).await {
            Ok(Some(outcome)) => info!(
                "scan finished: {} folders, {} tracks ({} unchanged), {} failed, {} gone",
                outcome.folders, outcome.tracks, outcome.unchanged, outcome.failed, outcome.gone
            ),
            Ok(None) => {}
            Err(e) => error!("scan failed: {e:#}"),
        }
    });

    Ok(StatusCode::ACCEPTED)
}

/// Cancel a scan
///
/// Stops the run in flight. Nothing it had written is kept: a scan that stopped
/// early has not seen most of the library, and keeping its work would mean
/// marking everything it never reached as gone. The next scan starts over.
///
/// Cancelling when nothing is running is not an error, since the caller wanted no
/// scan running and there is none.
#[utoipa::path(
    delete,
    path = "/scan",
    tag = "scan",
    responses(
        (status = 204, description = "The scan has been asked to stop"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn cancel(_admin: Administrator, State(progress): State<Arc<Progress>>) -> StatusCode {
    progress.cancel();
    StatusCode::NO_CONTENT
}
