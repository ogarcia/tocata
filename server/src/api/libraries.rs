// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The directories Tocata reads music from.
//!
//! `TOCATA_LIBRARY_PATHS` still works and still runs on every start, but it only
//! adds and enables what it names. What exists beyond that is this table's
//! business, which is what makes a library added here survive a restart.

use super::error::ApiError;
use super::session::Administrator;
use crate::db;
use crate::db::InTurn;
use crate::scanner::overlaps;
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
///
/// Administrators only, and the reason is the path. A row here names the directory a
/// library reads from, which says where somebody's disks are mounted and how they have
/// arranged them — and that is answered nowhere else: a track's own panel reports its
/// path *relative* to its library, deliberately, so that everybody who may hear the
/// music does not thereby learn the shape of the machine serving it. Leaving this open
/// to any account that could log in would have handed back what that decision withheld.
///
/// Nothing needed it. Both screens that read this are administration screens, and the
/// one other caller — the sentence an empty collection shows on a first run — asks only
/// when the account is an administrator, because a listener with nothing to look at
/// needs a sentence they can repeat rather than a path.
#[utoipa::path(
    get,
    path = "/libraries",
    tag = "libraries",
    responses(
        (status = 200, description = "Every library there is", body = Vec<Library>),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Not an administrator", body = ErrorBody),
    )
)]
pub async fn list(
    _admin: Administrator,
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

    unclaimed(&pool, path, None).await?;

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
    .in_turn(&pool)
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
        (status = 409, description = "Another library already reads from that directory, or from one around or inside it", body = ErrorBody),
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
        Some(path) if !path.is_empty() => {
            let checked = usable(path).await?;
            unclaimed(&pool, Path::new(&checked), Some(id)).await?;
            Some(checked)
        }
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
    .in_turn(&pool)
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
        .in_turn(&pool)
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

/// Refuses a root that is the same as, inside, or around another library.
///
/// Two libraries that overlap have no meaning. Every file under the shared part
/// belongs to both, so it is scanned twice and counted twice — a directory added
/// twice this way turned 48 tracks into 96 and 4 albums into 8 — and there is no
/// answer to which library it is in, which is the question every per-account
/// permission is asked. Nothing downstream can sort that out, so it is refused
/// here.
///
/// `except` is the library being changed, which is allowed to overlap itself.
async fn unclaimed(pool: &SqlitePool, path: &Path, except: Option<i64>) -> Result<(), ApiError> {
    let existing: Vec<(i64, String)> = sqlx::query_as("SELECT id, path FROM libraries")
        .fetch_all(pool)
        .await
        .map_err(|e| ApiError::internal(e, "checking the other libraries"))?;

    for (id, other) in existing {
        if Some(id) == except {
            continue;
        }

        if overlaps(path, Path::new(&other)) {
            return Err(ApiError::Conflict(
                "That directory is inside another library, or holds one",
            ));
        }
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    /// Real directories, because a path is checked against the disk before it is
    /// accepted and there is no way to test that against a made up one.
    fn directories(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("tocata-libraries-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("outer/inner")).unwrap();
        std::fs::create_dir_all(root.join("apart")).unwrap();
        std::fs::write(root.join("a file"), b"not a directory").unwrap();
        root
    }

    async fn a_server() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
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

    fn asking(path: &std::path::Path, name: Option<&str>) -> Json<NewLibrary> {
        Json(NewLibrary {
            path: path.to_string_lossy().to_string(),
            name: name.map(str::to_string),
        })
    }

    /// Added with no name of its own, a library is called after its directory —
    /// the same thing the environment variable does, so a library added either way
    /// reads the same in the panel.
    #[tokio::test]
    async fn a_library_added_without_a_name_is_called_after_its_directory() {
        let pool = a_server().await;
        let root = directories("named");

        let (status, Json(added)) = add(
            an_administrator(),
            State(pool.clone()),
            asking(&root.join("outer"), None),
        )
        .await
        .unwrap();

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(added.name, "outer");
        assert!(added.enabled, "and it is on, or adding it would do nothing");
    }

    /// The three ways a path is no good, each answered as a mistake in the asking
    /// rather than as a failure of ours.
    #[tokio::test]
    async fn a_path_that_is_not_a_readable_directory_is_refused() {
        let pool = a_server().await;
        let root = directories("paths");

        for (path, what) in [
            ("relative/music".to_string(), "not absolute"),
            (
                root.join("nowhere").to_string_lossy().to_string(),
                "not there",
            ),
            (
                root.join("a file").to_string_lossy().to_string(),
                "not a directory",
            ),
        ] {
            let refused = add(
                an_administrator(),
                State(pool.clone()),
                Json(NewLibrary { path, name: None }),
            )
            .await
            .expect_err(what);

            assert!(matches!(refused, ApiError::Invalid(_)), "{what}");
        }
    }

    /// A library inside another, or holding another, is refused both ways round.
    ///
    /// Two libraries over the same files would scan them twice and count them
    /// twice, and which of the two a track belonged to would be whichever got
    /// there first.
    #[tokio::test]
    async fn a_library_inside_another_is_refused_from_either_end() {
        let pool = a_server().await;
        let root = directories("nesting");

        let _ = add(
            an_administrator(),
            State(pool.clone()),
            asking(&root.join("outer"), None),
        )
        .await
        .unwrap();

        let inside = add(
            an_administrator(),
            State(pool.clone()),
            asking(&root.join("outer/inner"), None),
        )
        .await
        .expect_err("inside one that is already a library");
        assert!(matches!(inside, ApiError::Conflict(_)));

        let holding = add(an_administrator(), State(pool.clone()), asking(&root, None))
            .await
            .expect_err("holding one that is already a library");
        assert!(matches!(holding, ApiError::Conflict(_)));

        let _ = add(
            an_administrator(),
            State(pool.clone()),
            asking(&root.join("apart"), None),
        )
        .await
        .expect("and one beside it is fine");
    }

    /// Moving a library is a change of one column, and the overlap rule must not
    /// count the library against itself while doing it.
    #[tokio::test]
    async fn a_library_can_be_moved_and_does_not_block_its_own_move() {
        let pool = a_server().await;
        let root = directories("moving");

        let (_, Json(added)) = add(
            an_administrator(),
            State(pool.clone()),
            asking(&root.join("outer"), None),
        )
        .await
        .unwrap();

        let Json(moved) = change(
            an_administrator(),
            State(pool.clone()),
            UrlPath(added.id),
            Json(LibraryChanges {
                name: None,
                path: Some(root.join("apart").to_string_lossy().to_string()),
                enabled: None,
            }),
        )
        .await
        .expect("moving it somewhere else");
        assert!(moved.path.ends_with("apart"));

        let _ = change(
            an_administrator(),
            State(pool.clone()),
            UrlPath(added.id),
            Json(LibraryChanges {
                name: Some("the same place".to_string()),
                path: Some(root.join("apart").to_string_lossy().to_string()),
                enabled: None,
            }),
        )
        .await
        .expect("and left where it is, it does not overlap itself");
    }

    /// A change that changes nothing is a mistake worth saying so, since the
    /// alternative is answering "done" to a request that did nothing.
    #[tokio::test]
    async fn a_change_that_names_nothing_is_refused() {
        let pool = a_server().await;
        let root = directories("empty-change");

        let (_, Json(added)) = add(
            an_administrator(),
            State(pool.clone()),
            asking(&root.join("outer"), None),
        )
        .await
        .unwrap();

        let refused = change(
            an_administrator(),
            State(pool.clone()),
            UrlPath(added.id),
            Json(LibraryChanges {
                name: Some("   ".to_string()),
                path: Some(String::new()),
                enabled: None,
            }),
        )
        .await
        .expect_err("whitespace is not a name and empty is not a path");
        assert!(matches!(refused, ApiError::Invalid(_)));
    }

    /// Removing one takes its music out of the collection, so it has to be
    /// switched off first — which is the step where the panel can say what is
    /// about to disappear.
    #[tokio::test]
    async fn a_library_is_switched_off_before_it_can_be_removed() {
        let pool = a_server().await;
        let root = directories("removing");

        let (_, Json(added)) = add(
            an_administrator(),
            State(pool.clone()),
            asking(&root.join("outer"), None),
        )
        .await
        .unwrap();

        let refused = remove(an_administrator(), State(pool.clone()), UrlPath(added.id))
            .await
            .expect_err("still on");
        assert!(matches!(refused, ApiError::Conflict(_)));

        let _ = change(
            an_administrator(),
            State(pool.clone()),
            UrlPath(added.id),
            Json(LibraryChanges {
                name: None,
                path: None,
                enabled: Some(false),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            remove(an_administrator(), State(pool.clone()), UrlPath(added.id))
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    /// One that was never there, told apart from one that cannot be removed yet.
    #[tokio::test]
    async fn removing_a_library_that_is_not_there_is_a_miss() {
        let pool = a_server().await;

        let missed = remove(an_administrator(), State(pool), UrlPath(404))
            .await
            .expect_err("no such library");
        assert!(matches!(missed, ApiError::NotFound));
    }
}
