// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The panel, carried inside the binary.
//!
//! Serving it from disk would mean the one-file delivery stops being one file,
//! so the built assets are compiled in. This is the measurement version: enough
//! to see what it costs and whether a static musl build survives it.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use include_dir::{Dir, include_dir};

static PANEL: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../panel/dist");

/// Enough of a table for what a panel is made of. Guessing from a crate would
/// mean a dependency to answer four questions.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Serves a built asset, or the page itself so that a route the panel knows and
/// the server does not still lands somewhere.
///
/// Except under the two prefixes that belong to somebody else. A fallback that
/// caught everything would answer `/rest/noSuchCall` with the panel's HTML and a
/// 200, so a client asking for a call we do not have would be told everything
/// went fine and handed a web page.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if path.starts_with("rest/") || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let path = if path.is_empty() { "index.html" } else { path };

    let file = PANEL
        .get_file(path)
        .or_else(|| PANEL.get_file("index.html"));

    match file {
        Some(file) => (
            [(
                header::CONTENT_TYPE,
                content_type(file.path().to_str().unwrap_or("")),
            )],
            file.contents(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
