// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Watching and steering the walk out for artist portraits.
//!
//! Shaped like the scan's three calls — how is it going, start one, stop it —
//! because it is the same kind of thing to a panel: something long that somebody
//! sets going and then watches. What it is not is a maintenance job. Those run
//! inside the request that asked for them and are measured in seconds; this is
//! three quarters of an hour of waiting politely on somebody else's server.

use super::error::ApiError;
use super::session::{Administrator, Panel};
use crate::portraits::{self, Fetching};
use crate::settings;
use crate::state::AppState;
use crate::types::{ErrorBody, Portraits};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Portrait status
///
/// How the walk in flight is going, or how the last one went — and, either way,
/// how many artists are still without a picture.
///
/// That last figure is what makes this worth asking for when nothing is running:
/// it is the answer to "is there anything to do here", which is what a panel
/// needs before it offers a button.
#[utoipa::path(
    get,
    path = "/portraits",
    tag = "portraits",
    responses(
        (status = 200, description = "How the walk is going", body = Portraits),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn status(
    _panel: Panel,
    State(pool): State<SqlitePool>,
    State(fetching): State<Arc<Fetching>>,
) -> Result<Json<Portraits>, ApiError> {
    Ok(Json(told(&pool, &fetching).await?))
}

/// Start looking for portraits
///
/// Answers as soon as the walk has been accepted rather than when it finishes,
/// which is three quarters of an hour later on a collection of any size.
///
/// Refused where the setting is off. That setting is the permission, not a
/// preference about what to show: with it off this server does not reach out to
/// anybody, and a button that quietly did anyway would make it a lie.
#[utoipa::path(
    post,
    path = "/portraits",
    tag = "portraits",
    responses(
        (status = 202, description = "The walk has been accepted"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 409, description = "One is already going, or looking is switched off", body = ErrorBody),
    )
)]
pub async fn start(
    _admin: Administrator,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let settings = settings::load(&state.pool)
        .await
        .map_err(|e| ApiError::internal(e, "reading the settings"))?;

    if !settings.fetch_portraits {
        return Err(ApiError::Conflict("Looking for portraits is switched off"));
    }

    // Asked before spawning so the answer is honest. The walk refuses a second of
    // its own accord, but it does so inside the task, by which time this handler
    // has already replied.
    if state.portraits.is_fetching() {
        return Err(ApiError::Conflict("Already looking for portraits"));
    }

    tokio::spawn(async move {
        portraits::walk(
            &state.pool,
            state.config.data_dir(),
            &state.net,
            &state.portraits,
        )
        .await;
    });

    Ok(StatusCode::ACCEPTED)
}

/// Stop looking
///
/// Stops the walk in flight, between artists rather than mid-request. What it
/// had already found is kept — every artist is written down as it goes — so
/// stopping this is pausing it rather than undoing it.
#[utoipa::path(
    delete,
    path = "/portraits",
    tag = "portraits",
    responses(
        (status = 204, description = "It has been asked to stop, if it was going"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn cancel(_admin: Administrator, State(fetching): State<Arc<Fetching>>) -> StatusCode {
    fetching.cancel();
    StatusCode::NO_CONTENT
}

/// The snapshot, the setting and the count of who is still wanting, as one
/// answer.
async fn told(pool: &SqlitePool, fetching: &Fetching) -> Result<Portraits, ApiError> {
    let settings = settings::load(pool)
        .await
        .map_err(|e| ApiError::internal(e, "reading the settings"))?;

    let wanting = portraits::wanting(pool)
        .await
        .map_err(|e| ApiError::internal(e, "counting who is without a picture"))?
        .len() as u64;

    let snapshot = fetching.snapshot();

    Ok(Portraits {
        fetching: snapshot.fetching,
        allowed: settings.fetch_portraits,
        artist: snapshot.artist,
        done: snapshot.done,
        total: snapshot.total,
        found: snapshot.found,
        wanting,
        started_at: snapshot.started_at,
        finished_at: snapshot.finished_at,
        cancelled: snapshot.cancelled,
        failure: snapshot.failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::user::User;

    async fn empty() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        settings::seed(&pool, &[]).await.unwrap();
        pool
    }

    fn admin() -> Administrator {
        Administrator {
            user: User {
                id: 1,
                username: "admin".to_string(),
                is_admin: true,
            },
        }
    }

    /// An artist with no picture and an identifier to look one up by is one this
    /// would walk out for; one without an identifier is not, because there is no
    /// way in that does not amount to guessing from a name.
    #[tokio::test]
    async fn only_an_artist_with_an_identifier_is_worth_a_walk() {
        let pool = empty().await;
        let at = db::now();

        for (id, name, mbid) in [
            (1, "Known", Some("2f0e8ef1-a0e0-4b1d-b1cf-1b7f0f6c3aa1")),
            (2, "Nameless", None),
        ] {
            sqlx::query(
                "INSERT INTO artists (id, public_id, name, mbid, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("ar{id}"))
            .bind(name)
            .bind(mbid)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let wanting = portraits::wanting(&pool).await.unwrap();
        assert_eq!(wanting.len(), 1);
        assert_eq!(wanting[0].name, "Known");
    }

    /// The setting is the permission. With it off there is no way to make this
    /// server talk to anybody, including by asking it nicely twice.
    #[tokio::test]
    async fn nothing_walks_out_while_the_setting_says_not_to() {
        let pool = empty().await;

        let mut settings = settings::load(&pool).await.unwrap();
        assert!(
            !settings.fetch_portraits,
            "a collection nobody has asked stays at home"
        );

        let state = AppState {
            pool: pool.clone(),
            scan: Arc::new(crate::scanner::Progress::default()),
            portraits: Arc::new(Fetching::default()),
            config: Arc::new(crate::config::Config::for_tests(
                crate::fixtures::temp_root("api-portraits"),
            )),
            meter: Arc::new(crate::resources::Meter::new().unwrap()),
            attempts: Arc::new(crate::attempts::Attempts::new()),
            net: crate::net::Net::new(),
            shutdown: tokio::sync::watch::channel(false).1,
        };

        let refused = start(admin(), State(state.clone())).await;
        assert!(
            matches!(refused, Err(ApiError::Conflict(_))),
            "switched off is switched off"
        );
        assert!(!state.portraits.is_fetching());

        // And the status says so, so a panel can offer the setting rather than a
        // button that would be refused.
        let reported = told(&pool, &state.portraits).await.unwrap();
        assert!(!reported.allowed);
        assert!(!reported.fetching);

        settings.fetch_portraits = true;
        settings::store(&pool, &settings).await.unwrap();

        let reported = told(&pool, &state.portraits).await.unwrap();
        assert!(reported.allowed);
        assert_eq!(
            reported.wanting, 0,
            "nobody in an empty collection wants one"
        );
    }
}
