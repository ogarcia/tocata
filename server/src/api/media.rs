// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The bytes the panel plays and draws.
//!
//! What is here that `/rest` does not already do is only the way in. The panel
//! holds a session cookie and nothing else — no password to put in a query
//! string, no key to hand to an `<audio>` element — and a cookie is what a
//! browser sends on its own for an element's `src`, because it is the same
//! origin. Which is the whole reason these three exist: with them the player is
//! `<audio src="/api/v1/tracks/x/audio">` and the covers are `<img>` tags, and
//! without them the panel would need credentials in a URL.
//!
//! The cookie is scoped to `/api`, so it reaches exactly these and nothing
//! outside them.
//!
//! Everything under the surface — finding the file, refusing a path that climbs
//! out of its library, ranges so a player can seek, the cover cache — is the
//! same code `/rest` runs, in [`crate::media`]. Only the extractor differs.

use super::error::ApiError;
use super::session::Panel;
use crate::config::Config;
use crate::media;
use crate::types::ErrorBody;
use axum::extract::{Path as UrlPath, Request, State};
use axum::response::{IntoResponse, Response};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::error;

/// A track's audio
///
/// The file as it is on disk, with byte ranges, which is what lets a player seek
/// into the middle of a song rather than fetch the whole of it. Nothing is
/// transcoded: what was scanned is what is served.
#[utoipa::path(
    get,
    path = "/tracks/{id}/audio",
    tag = "collection",
    params(("id" = String, Path, description = "Which track")),
    responses(
        (status = 200, description = "The audio"),
        (status = 206, description = "The part of it that was asked for"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such track, or not one you may hear", body = ErrorBody),
    )
)]
pub async fn audio(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(id): UrlPath<String>,
    request: Request,
) -> Response {
    match media::locate(&pool, panel.user.id, &id).await {
        Ok(Some(track)) => media::serve(track, request).await,
        // A track in a library this account may not see is not a track it may
        // know exists, so both answers are the same one.
        Ok(None) | Err(media::Refused::Traversal) => ApiError::NotFound.into_response(),
        Err(media::Refused::Database(e)) => {
            ApiError::internal(e, "finding a track to play").into_response()
        }
    }
}

/// An album's cover
///
/// Found inside the music or beside it the first time it is asked for, then
/// served from the cache. An album with none answers 404, which is what lets a
/// grid draw the empty ones without asking twice.
#[utoipa::path(
    get,
    path = "/albums/{id}/cover",
    tag = "collection",
    params(("id" = String, Path, description = "Which album")),
    responses(
        (status = 200, description = "The cover"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No cover, or no such album", body = ErrorBody),
    )
)]
pub async fn cover(
    panel: Panel,
    State(pool): State<SqlitePool>,
    State(config): State<Arc<Config>>,
    UrlPath(id): UrlPath<String>,
    request: Request,
) -> Response {
    let album = match media::resolve_album(&pool, panel.user.id, &id).await {
        Ok(Some(album)) => album,
        Ok(None) => return ApiError::NotFound.into_response(),
        Err(e) => return ApiError::internal(e, "finding an album's cover").into_response(),
    };

    let cached = match media::cover_in_cache(&pool, album).await {
        Ok(cached) => cached,
        Err(e) => return ApiError::internal(e, "reading the cover cache").into_response(),
    };

    let found = match cached {
        Some(found) => Some(found),
        None => match media::extract_cover(&pool, config.data_dir(), album).await {
            Ok(found) => found,
            Err(e) => return ApiError::internal(e, "looking for a cover").into_response(),
        },
    };

    match found {
        Some((hash, mime_type)) => image(config.data_dir(), &hash, &mime_type, request).await,
        None => ApiError::NotFound.into_response(),
    }
}

/// An artist's picture
///
/// Looked for in the directory their records sit in, the first time it is asked
/// for, under the names the tools that fetch these already write. There is no
/// picture of an artist inside a music file, so a collection that has none on
/// disk has none at all — until Tocata learns to go and find them.
#[utoipa::path(
    get,
    path = "/artists/{id}/image",
    tag = "collection",
    params(("id" = String, Path, description = "Which artist")),
    responses(
        (status = 200, description = "The picture"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No picture, or no such artist", body = ErrorBody),
    )
)]
pub async fn portrait(
    panel: Panel,
    State(pool): State<SqlitePool>,
    State(config): State<Arc<Config>>,
    UrlPath(id): UrlPath<String>,
    request: Request,
) -> Response {
    let artist = match media::resolve_artist(&pool, panel.user.id, &id).await {
        Ok(Some(artist)) => artist,
        Ok(None) => return ApiError::NotFound.into_response(),
        Err(e) => return ApiError::internal(e, "finding an artist").into_response(),
    };

    match media::artist_picture(&pool, config.data_dir(), artist).await {
        Ok(Some((hash, mime_type))) => image(config.data_dir(), &hash, &mime_type, request).await,
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => ApiError::internal(e, "looking for an artist's picture").into_response(),
    }
}

/// A cached image, from the file its hash names.
async fn image(
    data_dir: &std::path::Path,
    hash: &str,
    mime_type: &str,
    request: Request,
) -> Response {
    let path = crate::artwork::path_of(data_dir, hash);
    let service = ServeFile::new_with_mime(
        &path,
        &mime_type.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM),
    );

    match service.oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            // The row says there is a file and there is not. Worth the log, and
            // a 404 for whoever asked: the picture is as absent as if there had
            // never been one.
            error!("serving a cached image from {}: {e}", path.display());
            ApiError::NotFound.into_response()
        }
    }
}
