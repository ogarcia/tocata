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

/// How long a file whose name carries its own hash may be kept.
///
/// A year, and `immutable` on top, which says not even to revalidate. Both are
/// safe for the same reason: the name changes when the contents do, so the name
/// somebody has cached is never a stale answer to anything — it is a file nobody
/// will ask for again.
const KEEP_FOREVER: &str = "public, max-age=31536000, immutable";

/// And what everything else gets: keep it, but ask every time.
///
/// This is `index.html`, which is the one file that cannot carry a hash, because
/// it is the thing that names all the others. Left to a browser's own judgement,
/// one that decided to keep it would go on asking for the wasm the old page named
/// — an old panel against a new server, with nothing on screen to say so, until
/// somebody thought to reload twice.
const ASK_EVERY_TIME: &str = "no-cache";

/// Whether the name carries a content hash, which is what makes a file safe to
/// keep forever.
///
/// Trunk writes them as `panel-1b959b7158768c33.css`, and the wasm as
/// `tocata-panel-3a135bac20405c57_bg.wasm`. Sixteen hex digits after the last
/// dash, which is specific enough that a file arriving without one falls to the
/// careful side rather than being kept for a year by accident.
fn hashed(path: &str) -> bool {
    let stem = path.rsplit_once('.').map_or(path, |(stem, _)| stem);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);

    stem.rsplit_once('-')
        .is_some_and(|(_, hash)| hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Enough of a table for what a panel is made of. Guessing from a crate would
/// mean a dependency to answer four questions.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        // A favicon served as a stream of bytes is a favicon the browser does
        // not draw.
        Some("svg") => "image/svg+xml",
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
        Some(file) => {
            // Decided by the file that came back and not by the path that was
            // asked for: a route only the panel knows is answered with the page
            // itself, and the page is the one thing that must not be kept.
            let name = file.path().to_str().unwrap_or("");

            (
                [
                    (header::CONTENT_TYPE, content_type(name)),
                    (
                        header::CACHE_CONTROL,
                        if hashed(name) {
                            KEEP_FOREVER
                        } else {
                            ASK_EVERY_TIME
                        },
                    ),
                ],
                file.contents(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_with_a_hash_in_it_is_recognised() {
        assert!(hashed("panel-1b959b7158768c33.css"));
        assert!(hashed("tocata-panel-3a135bac20405c57.js"));
        assert!(hashed("tocata-panel-3a135bac20405c57_bg.wasm"));
        assert!(hashed("favicon-cf508203782903d2.svg"));
    }

    #[test]
    fn a_name_without_one_is_not() {
        assert!(!hashed("index.html"));
        assert!(!hashed("panel.css"));
        // Named after something rather than hashed, which is a name that can come
        // back meaning different bytes.
        assert!(!hashed("tocata-panel.js"));
        assert!(!hashed("apple-touch-icon.png"));
        // The right length and not hex, which is what a word of sixteen letters
        // would be.
        assert!(!hashed("something-abcdefghijklmnop.css"));
    }

    /// The rule this rests on: everything in the built panel carries a hash except
    /// the page that names them. If trunk ever writes an asset without one it will
    /// be served with `no-cache`, which is safe — and this is what says so out loud
    /// rather than leaving somebody to find out from a stale panel.
    #[test]
    fn everything_built_carries_a_hash_but_the_page() {
        let mut bare = Vec::new();

        for file in PANEL.files() {
            let name = file.path().to_str().unwrap_or_default();
            if name != "index.html" && !hashed(name) {
                bare.push(name.to_string());
            }
        }

        assert!(
            bare.is_empty(),
            "these are served without a hash in the name, so they cannot be kept: {bare:?}"
        );
    }
}
