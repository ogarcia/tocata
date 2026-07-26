// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Serving the files themselves.

use super::auth::Authenticated;
use super::browsing::IdQuery;
use super::error::ApiError;
use super::response::Format;
use crate::artwork;
use crate::config::Config;
use axum::extract::{Query, Request, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use sqlx::SqlitePool;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::{error, warn};

/// Where a track lives, and the library root it must live under.
struct Located {
    path: PathBuf,
    content_type: String,
    library_root: PathBuf,
}

pub async fn stream(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
    request: Request,
) -> Response {
    match locate(&pool, &query.id).await {
        Ok(Some(track)) => serve(track, request).await,
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(Refused::Traversal) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(Refused::Database(e)) => {
            error!("locating a track to stream: {e}");
            ApiError::Internal.in_format(auth.format).into_response()
        }
    }
}

pub async fn download(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
    request: Request,
) -> Response {
    let track = match locate(&pool, &query.id).await {
        Ok(Some(track)) => track,
        Ok(None) | Err(Refused::Traversal) => {
            return ApiError::NotFound.in_format(auth.format).into_response();
        }
        Err(Refused::Database(e)) => {
            error!("locating a track to download: {e}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };

    let filename = track
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let mut response = serve(track, request).await;
    if let Ok(value) = content_disposition(&filename).parse() {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }

    response
}

enum Refused {
    /// The stored path resolved outside its library. Answered as not found:
    /// whoever asked has no business learning the difference.
    Traversal,
    Database(sqlx::Error),
}

async fn locate(pool: &SqlitePool, public_id: &str) -> Result<Option<Located>, Refused> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT t.path, t.content_type, l.path
           FROM tracks t
           JOIN libraries l ON l.id = t.library_id
          WHERE t.public_id = ? AND t.missing_since IS NULL",
    )
    .bind(public_id)
    .fetch_optional(pool)
    .await
    .map_err(Refused::Database)?;

    let Some((path, content_type, library_root)) = row else {
        return Ok(None);
    };

    let path = PathBuf::from(path);
    let library_root = PathBuf::from(library_root);

    // Defence in depth, and a deliberate exception to not writing guards for
    // conditions that cannot arise today.
    //
    // Nothing user supplied reaches this path: it comes from the scanner, which
    // walks real directory entries and skips symlinks. But this is the one
    // place in the program that opens an arbitrary file from disk and hands it
    // to whoever asked, so the cost of being wrong is serving /etc/passwd while
    // the cost of the check is comparing two prefixes. That asymmetry is what
    // justifies it: the day somebody adds a way to register a track by hand, or
    // decides to follow symlinks, this is already here.
    if !is_inside(&path, &library_root) {
        warn!(
            "refusing {}: it resolves outside its library root {}",
            path.display(),
            library_root.display()
        );
        return Err(Refused::Traversal);
    }

    Ok(Some(Located {
        path,
        content_type,
        library_root,
    }))
}

async fn serve(track: Located, request: Request) -> Response {
    // ServeFile brings range requests with it, which is what lets a client
    // seek into the middle of a song instead of fetching the whole thing, and
    // what makes a browser's audio element work at all.
    let service = ServeFile::new_with_mime(
        &track.path,
        &track
            .content_type
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
    );

    match service.oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            error!(
                "serving {} from {}: {e}",
                track.path.display(),
                track.library_root.display()
            );
            ApiError::Internal.in_format(Format::Xml).into_response()
        }
    }
}

/// Whether `path` sits under `root` once both are normalised.
///
/// Normalises rather than calling `canonicalize`: resolving on disk would
/// follow symlinks, which is the opposite of what a containment check wants,
/// and it would touch the filesystem for every request.
fn is_inside(path: &Path, root: &Path) -> bool {
    let path = normalise(path);
    let root = normalise(root);

    !root.as_os_str().is_empty() && path.starts_with(&root)
}

/// Resolves `.` and `..` textually, without consulting the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Refuses to climb above the start, so a path made of nothing
                // but `..` cannot escape by arithmetic.
                out.pop();
            }
            other => out.push(other),
        }
    }

    out
}

/// Builds the header for a download, giving both the plain and the encoded
/// form: a name with an accent in it is not representable in the first.
fn content_disposition(filename: &str) -> String {
    let sanitised: String = filename
        .chars()
        .map(|c| match c {
            // Quotes and control characters would end the field early or split
            // the header.
            '"' | '\\' | '\r' | '\n' => '_',
            c if c.is_control() => '_',
            c if c.is_ascii() => c,
            // Anything outside ASCII has no place in the plain form.
            _ => '_',
        })
        .collect();

    format!(
        "attachment; filename=\"{sanitised}\"; filename*=UTF-8''{}",
        percent_encode(filename)
    )
}

fn percent_encode(value: &str) -> String {
    use std::fmt::Write;

    value.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

// ---------------------------------------------------------------------------
// Cover art
// ---------------------------------------------------------------------------

/// Serves a cover image out of the cache.
///
/// The `size` parameter is not honoured: the original is returned whatever was
/// asked for, which the specification allows and every client copes with. Scaling
/// would mean decoding and re-encoding images, and that is worth doing only once
/// there is a measurement saying it matters.
pub async fn get_cover_art(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    State(config): State<Arc<Config>>,
    Query(query): Query<IdQuery>,
    request: Request,
) -> Response {
    let row: Option<(String, String)> =
        match sqlx::query_as("SELECT content_hash, mime_type FROM artworks WHERE public_id = ?")
            .bind(&query.id)
            .fetch_optional(&pool)
            .await
        {
            Ok(row) => row,
            Err(e) => {
                error!("locating cover art: {e}");
                return ApiError::Internal.in_format(auth.format).into_response();
            }
        };

    let Some((hash, mime_type)) = row else {
        return ApiError::NotFound.in_format(auth.format).into_response();
    };

    let path = artwork::cache_path(config.data_dir(), &hash);
    let service = ServeFile::new_with_mime(
        &path,
        &mime_type.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM),
    );

    match service.oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            // The row says there is an image but the file is not there, which
            // means the cache was cleared out from under us.
            error!("serving cover art from {}: {e}", path.display());
            ApiError::NotFound.in_format(auth.format).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_under_the_root_is_allowed() {
        assert!(is_inside(
            Path::new("/music/Queen/song.flac"),
            Path::new("/music")
        ));
        assert!(is_inside(
            Path::new("/music/song.flac"),
            Path::new("/music")
        ));
    }

    #[test]
    fn climbing_out_of_the_root_is_refused() {
        assert!(!is_inside(
            Path::new("/music/../etc/passwd"),
            Path::new("/music")
        ));
        assert!(!is_inside(
            Path::new("/music/Queen/../../etc/passwd"),
            Path::new("/music")
        ));
        assert!(!is_inside(Path::new("/etc/passwd"), Path::new("/music")));
    }

    #[test]
    fn a_sibling_directory_sharing_a_prefix_is_not_inside() {
        // "/music-private" starts with "/music" as a string but is a different
        // directory, which a naive string comparison would let through.
        assert!(!is_inside(
            Path::new("/music-private/secret.flac"),
            Path::new("/music")
        ));
    }

    #[test]
    fn current_directory_components_do_not_confuse_it() {
        assert!(is_inside(
            Path::new("/music/./Queen/./song.flac"),
            Path::new("/music")
        ));
    }

    #[test]
    fn an_empty_root_allows_nothing() {
        assert!(!is_inside(Path::new("/music/song.flac"), Path::new("")));
    }

    #[test]
    fn a_path_of_nothing_but_parents_cannot_escape() {
        assert!(!is_inside(
            Path::new("../../../etc/passwd"),
            Path::new("/music")
        ));
    }

    #[test]
    fn a_download_name_is_safe_to_put_in_a_header() {
        let header = content_disposition("song.flac");
        assert!(header.contains(r#"filename="song.flac""#));
        assert!(header.contains("filename*=UTF-8''song.flac"));
    }

    #[test]
    fn quotes_and_newlines_cannot_break_out_of_the_header() {
        let header = content_disposition("evil\"; rm -rf /\r\nX-Injected: yes.flac");

        // Exactly the two quotes that delimit the plain form, so none came from
        // the file name and closed the field early.
        assert_eq!(
            header.chars().filter(|c| *c == '"').count(),
            2,
            "got {header}"
        );
        // A newline here would split the header and let the rest of the name
        // become a header of its own.
        assert!(!header.contains('\r'), "got {header}");
        assert!(!header.contains('\n'), "got {header}");
        assert!(!header.contains("X-Injected: yes\""), "got {header}");
    }

    #[test]
    fn an_accented_name_survives_in_the_encoded_form() {
        let header = content_disposition("Björk.flac");
        // Unrepresentable in the plain form, so it degrades there...
        assert!(header.contains(r#"filename="Bj_rk.flac""#), "got {header}");
        // ...and travels intact in the encoded one.
        assert!(header.contains("filename*=UTF-8''Bj%C3%B6rk.flac"));
    }
}
