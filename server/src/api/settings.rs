// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Settings that belong to the collection rather than to the deployment.
//!
//! Reading is open to anybody with a session, since the panel draws the value
//! and none of it is a secret. Writing is administration: it changes what every
//! client sees.

use super::error::ApiError;
use super::session::{Administrator, Panel};
use crate::settings;
use crate::types::{ErrorBody, Settings, SettingsChanges};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;

impl From<settings::Settings> for Settings {
    fn from(settings: settings::Settings) -> Self {
        Self {
            ignored_articles: settings.ignored_articles,
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
        (status = 400, description = "An article that could never match", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn change(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    Json(changes): Json<SettingsChanges>,
) -> Result<Json<Settings>, ApiError> {
    if let Some(articles) = changes.ignored_articles {
        // Only the first word of a name is ever compared against this list, so
        // an entry with a space in it could not match anything. Storing one
        // would be storing a setting that silently does nothing.
        if articles.iter().any(|a| a.split_whitespace().count() != 1) {
            return Err(ApiError::Invalid(
                "An ignored article must be a single word",
            ));
        }

        settings::set_ignored_articles(&pool, &articles)
            .await
            .map_err(|e| ApiError::internal(e, "changing the ignored articles"))?;
    }

    // Read back rather than echo: what the server will use from now on is what
    // the row says, not what the request said.
    settings::load(&pool)
        .await
        .map(|settings| Json(settings.into()))
        .map_err(|e| ApiError::internal(e, "reading the settings"))
}
