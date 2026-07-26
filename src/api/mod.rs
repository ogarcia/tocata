// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Tocata's own API, served under `/api/v1`.
//!
//! Everything the panel needs that OpenSubsonic has no call for lives here
//! rather than as an extension of `/rest`. Stretching `/rest` would mean
//! inventing calls a client might reasonably expect to mean something, wrapping
//! them in an envelope that exists to satisfy a specification they are not part
//! of, and pretending they are a protocol somebody else might implement.
//!
//! The version is in the path because the panel and the server are released
//! together but not necessarily deployed together, and because a stable name is
//! only stable if there is somewhere to put the next one.

mod error;
mod session;

use crate::state::AppState;
use axum::Router;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_scalar::{Scalar, Servable};

/// Where the reference lives. Outside `/v1` on purpose: it documents whatever
/// versions the server happens to speak, and there is no reason to move it when
/// a second one appears.
const DOCS_PATH: &str = "/api/docs";

/// Scalar's own page, with two changes. The title, and `operationTitleSource`,
/// which makes the sidebar list paths rather than summaries: a reference is
/// something you navigate by URL, and the summary is already the heading of what
/// you land on.
const SCALAR_HTML: &str = r#"<!doctype html>
<html>
  <head>
    <title>Tocata API</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
  </head>
  <body>
    <script
      id="api-reference"
      type="application/json"
      data-configuration='{"operationTitleSource":"path"}'
    >$spec</script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
  </body>
</html>
"#;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Tocata",
        description = "The native API behind Tocata's administration panel. \
                       OpenSubsonic clients want /rest instead.",
        license(name = "GPL-3.0-or-later", identifier = "GPL-3.0-or-later"),
    ),
    tags(
        (name = "session", description = "Logging in and out of the panel"),
    )
)]
struct Reference;

pub fn router(state: AppState) -> Router {
    let (router, reference) = OpenApiRouter::with_openapi(Reference::openapi())
        .nest("/api/v1", v1())
        .split_for_parts();

    router
        .merge(Scalar::with_url(DOCS_PATH, reference).custom_html(SCALAR_HTML))
        .with_state(state)
}

/// The one version there is. Paths are declared relative to it, so moving the
/// whole set under `/v2` one day is one line here and nothing in the handlers.
fn v1() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(session::log_in, session::log_out, session::current))
}
