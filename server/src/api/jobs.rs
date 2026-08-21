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

        let look_again = jobs::revisiting(&pool, job)
            .await
            .map_err(|e| ApiError::internal(e, "working out what a job would look at again"))?;

        listed.push(JobState {
            job,
            pending,
            look_again,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::user::User;
    use crate::{attempts, db, net, resources, scanner, settings};
    use tokio::sync::watch;

    /// A server with a database of its own, on disk and in its own directory, the way
    /// the real one has.
    ///
    /// Not one held in memory: these tests run the check job, and that job cannot be
    /// run against a shared-cache database — which is the only kind sqlx makes in
    /// memory — without the chance of stopping the whole test binary. What that is and
    /// how it was found is written down on `jobs::tests::empty`.
    async fn a_server() -> AppState {
        let data_dir =
            std::env::temp_dir().join(format!("tocata-jobs-api-{}", db::public_id().unwrap()));
        std::fs::create_dir_all(&data_dir).unwrap();

        let pool = db::connect(&data_dir.join("tocata.db")).await.unwrap();
        settings::seed(&pool, &[]).await.unwrap();

        AppState {
            pool: pool.clone(),
            scan: Arc::new(scanner::Progress::default()),
            portraits: Arc::new(crate::portraits::Fetching::default()),
            attempts: Arc::new(attempts::Attempts::new()),
            config: Arc::new(Config::for_tests(data_dir)),
            meter: Arc::new(resources::Meter::new().unwrap()),
            settings: Arc::new(settings::Current::for_tests(&pool).await),
            net: net::Net::new(),
            shutdown: watch::channel(false).1,
        }
    }

    fn an_administrator() -> Administrator {
        Administrator {
            user: User {
                id: 1,
                username: "ana".to_string(),
                is_admin: true,
            },
        }
    }

    #[tokio::test]
    async fn a_job_nobody_offers_is_a_miss() {
        let state = a_server().await;

        let missed = start(
            an_administrator(),
            State(state),
            Path("polish the covers".to_string()),
        )
        .await
        .expect_err("no such job");
        assert!(matches!(missed, ApiError::NotFound));
    }

    /// Every one of these either reads the whole database or rewrites it, and a
    /// scan is writing the catalogue and deciding what is absent. None of them is
    /// urgent enough to be worth doing to a moving target.
    #[tokio::test]
    async fn a_job_will_not_start_while_a_scan_is_running() {
        let state = a_server().await;
        state.scan.pretend_a_scan_is_running();

        let refused = start(
            an_administrator(),
            State(state.clone()),
            Path("check".to_string()),
        )
        .await
        .expect_err("a scan is running");
        assert!(matches!(refused, ApiError::Conflict(_)));

        let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM job_runs")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(runs, 0, "and nothing was started");
    }

    /// The run is written down as it happens, which is what the panel reads to say
    /// when a job last ran and how it went.
    #[tokio::test]
    async fn a_job_that_runs_leaves_its_run_behind() {
        let state = a_server().await;

        let Json(run) = start(
            an_administrator(),
            State(state.clone()),
            Path("check".to_string()),
        )
        .await
        .expect("nothing is in its way");

        assert!(run.finished);
        assert!(run.error.is_none(), "a fresh database checks out clean");

        let (job, finished): (String, Option<String>) =
            sqlx::query_as("SELECT job, finished_at FROM job_runs")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(job, "check");
        assert!(
            finished.is_some(),
            "written down as finished, not just begun"
        );
    }
}
