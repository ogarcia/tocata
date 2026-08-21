// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Settings that belong to the collection rather than to the deployment.
//!
//! Reading is open to anybody with a session, since the panel draws the value
//! and none of it is a secret. Writing is administration: it changes what every
//! client sees.

use super::error::ApiError;
use super::session::{Administrator, Panel};
use crate::portraits::Fetching;
use crate::settings;
use crate::types::{ErrorBody, Settings, SettingsChanges};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;
use std::sync::Arc;

impl From<settings::Settings> for Settings {
    fn from(settings: settings::Settings) -> Self {
        Self {
            ignored_articles: settings.ignored_articles,
            scan_at_startup: settings.scan_at_startup,
            scan_at: settings.scan_at,
            absent_grace_days: settings.absent_grace_days,
            session_days: settings.session_days,
            reach_out: settings.reach_out,
        }
    }
}

/// Server settings
///
/// What the server has been told about the collection.
#[utoipa::path(
    get,
    path = "/settings",
    tag = "settings",
    responses(
        (status = 200, description = "The settings", body = Settings),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn read(
    _panel: Panel,
    State(pool): State<SqlitePool>,
) -> Result<Json<Settings>, ApiError> {
    settings::load(&pool)
        .await
        .map(|settings| Json(settings.into()))
        .map_err(|e| ApiError::internal(e, "reading the settings"))
}

/// Change the settings
///
/// Takes effect on the next request. Nothing here is baked into the scan, so
/// there is nothing to rebuild afterwards.
#[utoipa::path(
    patch,
    path = "/settings",
    tag = "settings",
    request_body = SettingsChanges,
    responses(
        (status = 200, description = "The settings as they now are", body = Settings),
        (status = 400, description = "A value the server could never act on", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn change(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    State(settings): State<Arc<settings::Current>>,
    State(fetching): State<Arc<Fetching>>,
    Json(changes): Json<SettingsChanges>,
) -> Result<Json<Settings>, ApiError> {
    let mut current = settings::load(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "reading the settings"))?;

    if let Some(articles) = changes.ignored_articles {
        // Only the first word of a name is ever compared against this list, so
        // an entry with a space in it could not match anything. Storing one
        // would be storing a setting that silently does nothing.
        if articles.iter().any(|a| a.split_whitespace().count() != 1) {
            return Err(ApiError::Invalid(
                "An ignored article must be a single word",
            ));
        }

        current.ignored_articles = articles;
    }

    if let Some(at_startup) = changes.scan_at_startup {
        current.scan_at_startup = at_startup;
    }

    if let Some(at) = changes.scan_at {
        // A time nothing could ever match is a schedule that silently never
        // runs, which is the worst way for a setting to be wrong.
        if let Some(written) = &at
            && chrono::NaiveTime::parse_from_str(written, settings::HOUR_AND_MINUTE).is_err()
        {
            return Err(ApiError::Invalid("The scan time is not an hour and minute"));
        }

        current.scan_at = at;
    }

    if let Some(days) = changes.absent_grace_days {
        if days.is_some_and(|days| days < 0) {
            return Err(ApiError::Invalid("A quarantine cannot be negative"));
        }

        current.absent_grace_days = days;
    }

    if let Some(days) = changes.session_days {
        if days < 1 {
            return Err(ApiError::Invalid("A session has to last at least a day"));
        }

        current.session_days = days;
    }

    if let Some(looking) = changes.reach_out {
        // A walk after portraits that is already going stops here. It is the one way
        // out of this machine that runs for an hour at a time, so without this,
        // switching the server off the network would be answered by it going on
        // asking somebody else's server until it had finished the queue.
        if !looking {
            fetching.cancel();
        }

        current.reach_out = looking;
    }

    // Which also tells the scheduler, and is the only way to: it watches the hour
    // from memory rather than reading the row every minute.
    settings
        .save(&pool, &current)
        .await
        .map_err(|e| ApiError::internal(e, "changing the settings"))?;

    // Read back rather than echo: what the server will use from now on is what
    // the row says, not what the request said.
    settings::load(&pool)
        .await
        .map(|settings| Json(settings.into()))
        .map_err(|e| ApiError::internal(e, "reading the settings"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    async fn a_seeded_server() -> (SqlitePool, Administrator) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        settings::seed(&pool, &["The".to_string()]).await.unwrap();

        let admin = Administrator {
            user: User {
                id: 1,
                username: "admin".to_string(),
                is_admin: true,
            },
        };

        (pool, admin)
    }

    fn same(admin: &Administrator) -> Administrator {
        Administrator {
            user: admin.user.clone(),
        }
    }

    fn asking(json: &str) -> Json<SettingsChanges> {
        Json(serde_json::from_str(json).unwrap())
    }

    /// The settings as the handler will publish them. Held rather than mocked,
    /// because publishing is half of what saving now means and a test that skipped
    /// it would not be exercising the handler.
    async fn held(pool: &SqlitePool) -> State<Arc<settings::Current>> {
        State(Arc::new(settings::Current::for_tests(pool).await))
    }

    /// A walk after portraits that is not going, which is what every test here has:
    /// the handler only ever asks it to stop, and asking a walk that is not running
    /// to stop is what it is for.
    fn idle() -> State<Arc<Fetching>> {
        State(Arc::new(Fetching::default()))
    }

    /// The point of changing one field at a time: two settings saved from the
    /// same screen must not undo each other, and neither must one saved alone.
    #[tokio::test]
    async fn what_is_not_mentioned_is_left_alone() {
        let (pool, admin) = a_seeded_server().await;

        let Json(after) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            idle(),
            asking(r#"{"sessionDays":7}"#),
        )
        .await
        .unwrap();

        assert_eq!(after.session_days, 7);
        assert_eq!(after.ignored_articles, ["The"], "not mentioned");
        assert!(after.scan_at_startup, "not mentioned either");
    }

    /// Null and absent are different answers, which is what the option inside an
    /// option is for: one stops the schedule, the other says nothing about it.
    #[tokio::test]
    async fn null_turns_a_schedule_off_and_absent_does_not() {
        let (pool, admin) = a_seeded_server().await;

        let Json(set) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            idle(),
            asking(r#"{"scanAt":"04:00"}"#),
        )
        .await
        .unwrap();
        assert_eq!(set.scan_at.as_deref(), Some("04:00"));

        let Json(untouched) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            idle(),
            asking(r#"{"sessionDays":30}"#),
        )
        .await
        .unwrap();
        assert_eq!(untouched.scan_at.as_deref(), Some("04:00"));

        let Json(cleared) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            idle(),
            asking(r#"{"scanAt":null}"#),
        )
        .await
        .unwrap();
        assert_eq!(cleared.scan_at, None);
    }

    /// Zero is a real quarantine — remove it as soon as a scan finds it gone —
    /// so it must not be mistaken for "no quarantine", which is null.
    #[tokio::test]
    async fn no_quarantine_and_none_at_all_are_different_answers() {
        let (pool, admin) = a_seeded_server().await;

        let Json(at_once) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            idle(),
            asking(r#"{"absentGraceDays":0}"#),
        )
        .await
        .unwrap();
        assert_eq!(at_once.absent_grace_days, Some(0));

        let Json(never) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            idle(),
            asking(r#"{"absentGraceDays":null}"#),
        )
        .await
        .unwrap();
        assert_eq!(never.absent_grace_days, None);
    }

    /// Closing the way out stops the walk that is already using it.
    ///
    /// The walk after portraits runs for the best part of an hour on a real
    /// collection, one request a second. Without this, an administrator switching the
    /// server off the network would be answered by it going on talking to somebody
    /// else's server until the queue ran out — which is the whole of what they had
    /// just said not to do.
    #[tokio::test]
    async fn switching_the_way_out_off_stops_a_walk_already_going() {
        let (pool, admin) = a_seeded_server().await;
        let walking = idle();

        assert!(
            !walking.0.should_stop(),
            "nothing has been told anything yet"
        );

        let Json(after) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            State(walking.0.clone()),
            asking(r#"{"reachOut":false}"#),
        )
        .await
        .unwrap();

        assert!(!after.reach_out);
        assert!(
            walking.0.should_stop(),
            "the walk has been told, not left to finish"
        );

        // And switching it back on does not ask anything to stop.
        let Json(after) = change(
            same(&admin),
            State(pool.clone()),
            held(&pool).await,
            idle(),
            asking(r#"{"reachOut":true}"#),
        )
        .await
        .unwrap();

        assert!(after.reach_out);
    }

    /// A setting the server could never act on is worse than no setting: it
    /// looks chosen and does nothing.
    #[tokio::test]
    async fn a_value_that_could_never_work_is_refused() {
        let (pool, admin) = a_seeded_server().await;

        for asked in [
            r#"{"scanAt":"tonight"}"#,
            r#"{"scanAt":"25:00"}"#,
            r#"{"absentGraceDays":-1}"#,
            r#"{"sessionDays":0}"#,
            r#"{"ignoredArticles":["Los Del"]}"#,
        ] {
            let refused = change(
                same(&admin),
                State(pool.clone()),
                held(&pool).await,
                idle(),
                asking(asked),
            )
            .await;

            assert!(
                matches!(refused, Err(ApiError::Invalid(_))),
                "{asked} should not have been accepted"
            );
        }

        // And none of them left anything behind on the way out.
        let settings = settings::load(&pool).await.unwrap();
        assert_eq!(settings.scan_at, None);
        assert_eq!(settings.session_days, 30);
    }
}
