// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Whether the server is worth sending requests to.
//!
//! The one call here that needs no session. Everything under `/rest` answers
//! HTTP 200 whatever happens, because in that protocol the error travels in the
//! payload, so no probe watching status lines can tell a working server from a
//! broken one. This is what a probe is meant to ask.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sqlx::SqlitePool;
use tracing::warn;
use utoipa::ToSchema;

/// Deliberately says nothing beyond whether the answer is yes.
///
/// No version, no counts, no scan state. This is the only call that answers
/// without a session, so what it discloses to a stranger is the whole of what it
/// discloses.
#[derive(Serialize, ToSchema)]
pub struct Health {
    #[schema(example = "ok")]
    status: &'static str,
}

/// Health
///
/// Answers 200 while the server can serve, and 503 when it cannot. Needs no
/// authentication, since a probe that has to hold a credential is a probe that
/// reports its own credential expiring as an outage.
#[utoipa::path(
    get,
    path = "/api/health",
    tag = "health",
    responses(
        (status = 200, description = "Able to serve", body = Health),
        (status = 503, description = "Not able to serve", body = Health),
    )
)]
pub async fn health(State(pool): State<SqlitePool>) -> Response {
    // Reaching the database is the part worth asking about. That the process
    // accepts connections is already answered by the response arriving at all,
    // and with SQLite there is no readiness apart from this: whatever the
    // scanner is doing, a pool that answers is a server that can serve.
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&pool)
        .await
    {
        Ok(_) => (StatusCode::OK, Json(Health { status: "ok" })).into_response(),
        Err(e) => {
            warn!("unhealthy: {e}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Health {
                    status: "unavailable",
                }),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_working_database_is_healthy() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        assert_eq!(health(State(pool)).await.status(), StatusCode::OK);
    }

    /// A closed pool is the cheapest stand in for a database that stopped
    /// answering, and it is the case the endpoint exists for: without the query
    /// this would still be a confident 200.
    #[tokio::test]
    async fn a_database_that_stopped_answering_is_not() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool.close().await;

        assert_eq!(
            health(State(pool)).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
