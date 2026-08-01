// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Asking for the maintenance jobs, and asking for one to be run.
//!
//! One call answers the whole screen: every job with what it would do now and
//! how it went last time, and the last few runs of anything. They are always
//! wanted together, and separately they would be two round trips to draw one
//! page.
//!
//! Running one is a POST that waits. These are seconds rather than minutes —
//! unlike a scan, which is why a scan has a stream and these do not — and what
//! the caller wants back is what happened.

use super::error::ApiError;
use super::session::Administrator;
use crate::config::Config;
use crate::jobs;
use crate::state::AppState;
use crate::types::{ErrorBody, JobState, Maintenance, Run};
use axum::Json;
use axum::extract::{Path, State};
use sqlx::SqlitePool;
use std::sync::Arc;

/// How many past runs the screen is given. Enough to fill the short list it
/// shows and to say what happened yesterday, and not a page of history nobody
/// asked for.
const LATELY: i64 = 10;

/// The maintenance jobs
///
/// What each one would do if it ran right now, when it last ran, and the last
/// few runs of anything.
#[utoipa::path(
    get,
    path = "/jobs",
    tag = "jobs",
    responses(
        (status = 200, description = "The jobs and what they have been doing", body = Maintenance),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn list(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    State(config): State<Arc<Config>>,
) -> Result<Json<Maintenance>, ApiError> {
    let latest = jobs::latest(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "reading when each job last ran"))?;

    let mut listed = Vec::with_capacity(jobs::EVERY.len());

    for job in jobs::EVERY {
        let pending = jobs::pending(&pool, config.data_dir(), job)
            .await
            .map_err(|e| ApiError::internal(e, "working out what a job would do"))?;

        listed.push(JobState {
            job,
            pending,
            last_run: latest.iter().find(|run| run.job == job).cloned(),
        });
    }

    let lately = jobs::history(&pool, LATELY)
        .await
        .map_err(|e| ApiError::internal(e, "reading what has been run lately"))?;

    Ok(Json(Maintenance {
        jobs: listed,
        lately,
    }))
}

/// Run a job
///
/// Answers when it is done. A job that could not be done comes back as a run
/// carrying the reason rather than as an error status: it was attempted, which
/// is what was asked for, and the reason belongs in the history beside it.
#[utoipa::path(
    post,
    path = "/jobs/{job}",
    tag = "jobs",
    params(("job" = String, Path, description = "Which job, by name")),
    responses(
        (status = 200, description = "It ran; here is how it went", body = Run),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 404, description = "No job by that name", body = ErrorBody),
        (status = 409, description = "A scan is running", body = ErrorBody),
    )
)]
pub async fn start(
    _admin: Administrator,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Run>, ApiError> {
    let Some(job) = jobs::named(&name) else {
        return Err(ApiError::NotFound);
    };

    // A scan is writing the catalogue and deciding what is absent, and every one
    // of these either reads the whole database or rewrites it. None of them is
    // urgent enough to be worth doing to a moving target.
    if state.scan.is_scanning() {
        return Err(ApiError::Conflict(
            "A scan is running; wait for it to finish",
        ));
    }

    jobs::run(&state.pool, state.config.data_dir(), job)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e, "running a job"))
}
