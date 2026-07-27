// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The directories Tocata reads music from.
//!
//! `TOCATA_LIBRARY_PATHS` still works and still runs on every start, but it only
//! adds and enables what it names. What exists beyond that is this table's
//! business, which is what makes a library added here survive a restart.

use super::error::ApiError;
use super::session::{Administrator, Panel};
use crate::db;
use crate::types::{ErrorBody, Library, LibraryChanges, NewLibrary};
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;
use std::path::Path;

type LibraryRow = (i64, String, String, bool, i64, i64, Option<String>);

impl From<LibraryRow> for Library {
    fn from((id, name, path, enabled, tracks, missing, last_scanned_at): LibraryRow) -> Self {
        Self {
            id,
            name,
            path,
            enabled,
            tracks,
            missing,
            last_scanned_at,
        }
    }
}

/// A macro rather than a constant so `concat!` builds each statement at compile
/// time: sqlx does not take SQL assembled at runtime.
macro_rules! library_columns {
    () => {
        "SELECT l.id, l.name, l.path, l.enabled,
                (SELECT count(*) FROM tracks t
                  WHERE t.library_id = l.id AND t.missing_since IS NULL),
                (SELECT count(*) FROM tracks t
                  WHERE t.library_id = l.id AND t.missing_since IS NOT NULL),
                (SELECT max(r.finished_at) FROM scan_runs r WHERE r.library_id = l.id)
           FROM libraries l"
    };
}

/// List libraries
///
/// Every library, enabled or not, with how much is in it and when it was last
/// scanned.
#[utoipa::path(
    get,
    path = "/libraries",
    tag = "libraries",
    responses(
        (status = 200, description = "Every library there is", body = Vec<Library>),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list(
    _panel: Panel,
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<Library>>, ApiError> {
    let rows: Vec<LibraryRow> = sqlx::query_as(concat!(library_columns!(), " ORDER BY l.name"))
        .fetch_all(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "listing libraries"))?;

    Ok(Json(rows.into_iter().map(Library::from).collect()))
}

/// Add a library
///
/// Registers a directory and enables it. Nothing is read yet: start a scan when
/// you want its contents, so that adding several does not mean waiting through
/// each one.
#[utoipa::path(
    post,
    path = "/libraries",
    tag = "libraries",
    request_body = NewLibrary,
    responses(
        (status = 201, description = "Added, with nothing scanned yet", body = Library),
        (status = 400, description = "The path is not an absolute existing directory", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 409, description = "That path is already a library", body = ErrorBody),
    )
)]
pub async fn add(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    Json(new): Json<NewLibrary>,
) -> Result<(StatusCode, Json<Library>), ApiError> {
    let checked = usable(new.path.trim()).await?;
    let path = Path::new(&checked);

    let name = new
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| new.path.clone());

    let timestamp = db::now();
    let id: Result<i64, _> = sqlx::query_scalar(
        "INSERT INTO libraries (name, path, enabled, created_at, updated_at)
         VALUES (?, ?, 1, ?, ?) RETURNING id",
    )
    .bind(&name)
    .bind(path.to_string_lossy().as_ref())
    .bind(&timestamp)
    .bind(&timestamp)
    .fetch_one(&pool)
    .await;

    let id = match id {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::Conflict("That path is already a library"));
        }
        Err(e) => return Err(ApiError::internal(e, "adding a library")),
    };

    Ok((StatusCode::CREATED, Json(load(&pool, id).await?)))
}

/// Change a library
///
/// Its name, where it reads from, or whether it is switched on.
///
/// Moving one takes effect at once and needs no scan. Every track and folder is
/// stored relative to the library's directory, so the directory is named in one
/// row and this is that row: the files are found at their new place on the next
/// request, with their ratings, play counts and playlists untouched.
///
/// What it does not do is check that the music is there. A path that exists and
/// holds something else will serve nothing until a scan sorts it out.
///
/// Renames it, or turns it on and off. Disabling keeps scans out of it and drops
/// it from the folder list, without touching a single row, which makes it the
/// reversible way to take a library out of service.
#[utoipa::path(
    patch,
    path = "/libraries/{id}",
    tag = "libraries",
    params(("id" = i64, Path, description = "Which library")),
    request_body = LibraryChanges,
    responses(
        (status = 200, description = "The library as it now is", body = Library),
        (status = 400, description = "Nothing worth changing was asked for", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 404, description = "No such library", body = ErrorBody),
        (status = 409, description = "Another library already reads from that directory", body = ErrorBody),
    )
)]
pub async fn change(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<i64>,
    Json(changes): Json<LibraryChanges>,
) -> Result<Json<Library>, ApiError> {
    let name = changes
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let moved = match changes.path.as_deref().map(str::trim) {
        Some(path) if !path.is_empty() => Some(usable(path).await?),
        _ => None,
    };

    if name.is_none() && moved.is_none() && changes.enabled.is_none() {
        return Err(ApiError::Invalid("Give a name, a path or an enabled flag"));
    }

    // Coalesce rather than assembling the statement: sqlx will not take SQL built
    // at runtime, and a null bind meaning "leave it" says the same thing in one
    // query instead of one per field.
    let changed = sqlx::query(
        "UPDATE libraries
            SET name = coalesce(?, name),
                path = coalesce(?, path),
                enabled = coalesce(?, enabled),
                updated_at = ?
          WHERE id = ?",
    )
    .bind(name)
    .bind(moved.as_deref())
    .bind(changes.enabled.map(i64::from))
    .bind(db::now())
    .bind(id)
    .execute(&pool)
    .await;

    let changed = match changed {
        Ok(changed) => changed,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::Conflict("That directory is already a library"));
        }
        Err(e) => return Err(ApiError::internal(e, "changing a library")),
    };

    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(Json(load(&pool, id).await?))
}

/// Remove a library
///
/// Takes the library and everything recorded from it out of the database, which
/// includes the favourites, ratings, play counts and playlist entries that point
/// at its tracks. There is no undoing it short of a backup.
///
/// Which is why it will only do it to a library that is already disabled.
/// Disabling costs nothing and is undone by asking again, so requiring it first
/// means no single misdirected call can destroy a collection's history.
#[utoipa::path(
    delete,
    path = "/libraries/{id}",
    tag = "libraries",
    params(("id" = i64, Path, description = "Which library")),
    responses(
        (status = 204, description = "Gone, along with everything scanned from it"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
        (status = 404, description = "No such library", body = ErrorBody),
        (status = 409, description = "Still enabled; disable it first", body = ErrorBody),
    )
)]
pub async fn remove(
    _admin: Administrator,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<i64>,
) -> Result<StatusCode, ApiError> {
    let enabled: Option<bool> = sqlx::query_scalar("SELECT enabled FROM libraries WHERE id = ?")
        .bind(id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "looking up a library to remove"))?;

    match enabled {
        None => return Err(ApiError::NotFound),
        Some(true) => {
            return Err(ApiError::Conflict("Disable the library before removing it"));
        }
        Some(false) => {}
    }

    sqlx::query("DELETE FROM libraries WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "removing a library"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Checks a path is somewhere music could be read from, and hands it back
/// trimmed.
///
/// Absolute because a relative path is resolved against a working directory the
/// caller cannot see: whoever sets the environment variable knows where the
/// process starts, and whoever calls this does not.
async fn usable(path: &str) -> Result<String, ApiError> {
    if !Path::new(path).is_absolute() {
        return Err(ApiError::Invalid("The path must be absolute"));
    }

    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_dir() => Ok(path.to_string()),
        Ok(_) => Err(ApiError::Invalid("That path is not a directory")),
        Err(_) => Err(ApiError::Invalid("That path cannot be read")),
    }
}

/// Reads one back, for the handlers that answer with what they just wrote.
async fn load(pool: &SqlitePool, id: i64) -> Result<Library, ApiError> {
    let row: Option<LibraryRow> = sqlx::query_as(concat!(library_columns!(), " WHERE l.id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ApiError::internal(e, "loading a library"))?;

    row.map(Library::from).ok_or(ApiError::NotFound)
}
