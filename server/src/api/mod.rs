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
//!
//! Two things sit outside the version: the reference, and the health check.
//! Neither has a contract that can evolve, so numbering them would only promise
//! a second one that is never coming.

mod attention;
mod collection;
mod error;
mod events;
mod favourites;
mod health;
mod jobs;
mod keys;
mod libraries;
mod media;
mod playlists;
mod portraits;
mod preferences;
mod purge;
mod resources;
mod scan;
mod scrobblers;
mod session;
mod sessions;
mod settings;
mod stats;
mod users;

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
        (name = "health", description = "Whether the server can serve"),
        (name = "session", description = "Logging in and out of the panel"),
        (name = "scan", description = "Watching and steering a scan of the libraries"),
        (name = "events", description = "The stream the panel keeps open"),
        (name = "libraries", description = "The directories music is read from"),
        (name = "users", description = "Accounts"),
        (name = "keys", description = "API keys, for clients that authenticate with one"),
        (name = "portraits", description = "Looking for pictures of the artists"),
        (name = "sessions", description = "The panel logins an account has open"),
        (name = "settings", description = "What the server knows about the collection"),
        (name = "preferences", description = "How the panel looks and speaks, per account"),
        (name = "scrobblers", description = "Where an account's listens are passed on to"),
        (name = "stats", description = "What there is, in figures"),
        (name = "resources", description = "What the server is costing the machine"),
        (name = "purge", description = "What removing what a scan marked would cost"),
        (name = "jobs", description = "The maintenance jobs, and running one"),
        (name = "maintenance", description = "The files a scan could not account for"),
        (name = "collection", description = "Browsing what has been scanned"),
        (name = "playlists", description = "Lists an account made, and who else may see them"),
    )
)]
struct Reference;

pub fn router(state: AppState) -> Router {
    let (router, reference) = assemble();

    router
        .merge(Scalar::with_url(DOCS_PATH, reference).custom_html(SCALAR_HTML))
        .with_state(state)
}

/// The routes and the document describing them, which are the same thing said
/// twice by the same builder.
///
/// Apart from [`router`] so a test can have the document as well. That is what
/// lets it call every path this API serves without keeping a list of its own —
/// a list which would be right on the day it was written and wrong on the day
/// somebody adds an endpoint.
fn assemble() -> (Router<AppState>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(Reference::openapi())
        // Not nested, so this one declares its whole path rather than a path
        // relative to a version it does not belong to.
        .routes(routes!(health::health))
        .nest("/api/v1", v1())
        .split_for_parts()
}

/// The one version there is. Paths are declared relative to it, so moving the
/// whole set under `/v2` one day is one line here and nothing in the handlers.
fn v1() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(session::log_in, session::log_out, session::current))
        .routes(routes!(scan::status, scan::start, scan::cancel))
        .routes(routes!(
            portraits::status,
            portraits::start,
            portraits::cancel
        ))
        .routes(routes!(events::stream))
        .routes(routes!(libraries::list, libraries::add))
        .routes(routes!(libraries::change, libraries::remove))
        .routes(routes!(users::list, users::create))
        .routes(routes!(users::one, users::change, users::delete))
        .routes(routes!(users::restrict))
        .routes(routes!(users::holdings))
        .routes(routes!(keys::list, keys::issue))
        .routes(routes!(keys::revoke_all))
        .routes(routes!(keys::remove, keys::change))
        .routes(routes!(keys::revoke))
        .routes(routes!(keys::rotate))
        .routes(routes!(sessions::list, sessions::close_all))
        .routes(routes!(sessions::close, sessions::name))
        .routes(routes!(settings::read, settings::change))
        .routes(routes!(preferences::change))
        .routes(routes!(scrobblers::list))
        .routes(routes!(
            scrobblers::set,
            scrobblers::switch,
            scrobblers::remove
        ))
        .routes(routes!(stats::stats))
        .routes(routes!(resources::read))
        .routes(routes!(purge::preview))
        .routes(routes!(attention::needing_attention))
        .routes(routes!(playlists::list, playlists::create))
        .routes(routes!(
            playlists::one,
            playlists::change,
            playlists::remove
        ))
        .routes(routes!(
            playlists::tracks,
            playlists::add,
            playlists::reorder
        ))
        .routes(routes!(playlists::drop_one))
        .routes(routes!(playlists::holding))
        .routes(routes!(favourites::counts))
        .routes(routes!(favourites::mark, favourites::unmark))
        .routes(routes!(collection::tracks))
        .routes(routes!(collection::track))
        .routes(routes!(collection::detail))
        .routes(routes!(collection::tags))
        .routes(routes!(collection::lyrics))
        .routes(routes!(collection::queue))
        .routes(routes!(collection::playing))
        .routes(routes!(collection::albums))
        .routes(routes!(collection::album))
        .routes(routes!(collection::artists))
        .routes(routes!(collection::artist))
        .routes(routes!(collection::genres))
        .routes(routes!(collection::genre))
        .routes(routes!(collection::played))
        .routes(routes!(media::audio))
        .routes(routes!(media::cover))
        .routes(routes!(media::portrait))
        .routes(routes!(jobs::list))
        .routes(routes!(jobs::start))
}

#[cfg(test)]
mod every_endpoint {
    use super::*;
    use crate::config::Config;
    use crate::{attempts, auth, db, net, resources, scanner, session, settings};
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use std::sync::Arc;
    use tokio::sync::watch;
    use tower::ServiceExt;

    const WHO: &str = "ana";

    /// A server with an administrator logged in, and nothing else in it.
    ///
    /// Empty for the same reason the other walk is: a collection with nothing in
    /// it is where a statement that cannot run still has to run, and that is the
    /// fault this is looking for.
    async fn a_server() -> (Router, String) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        settings::seed(&pool, &[]).await.unwrap();

        let at = db::now();
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES (?, ?, 1, ?, ?) RETURNING id",
        )
        .bind(WHO)
        .bind(auth::hash_password("unused").unwrap())
        .bind(&at)
        .bind(&at)
        .fetch_one(&pool)
        .await
        .unwrap();

        let (token, _) = session::create(&pool, user, 1, None).await.unwrap();

        // The sender is dropped here on purpose. The event stream ends when the
        // server says it is stopping, and a dropped sender says exactly that —
        // otherwise that one path would hold this test open for ever.
        let shutdown = watch::channel(false).1;

        let state = AppState {
            pool: pool.clone(),
            scan: Arc::new(scanner::Progress::default()),
            portraits: Arc::new(crate::portraits::Fetching::default()),
            attempts: Arc::new(attempts::Attempts::new()),
            config: Arc::new(Config::for_tests(
                std::env::temp_dir().join("tocata-panel-endpoints"),
            )),
            meter: Arc::new(resources::Meter::new().unwrap()),
            settings: Arc::new(crate::settings::Current::for_tests(&pool).await),
            net: net::Net::new(),
            shutdown,
        };

        (router(state), format!("tocata_session={token}"))
    }

    /// Every path that can be asked for is asked for, and none of them answers
    /// with our own mistake.
    ///
    /// Reading only, which is the honest limit of a walk like this: a request
    /// with no body is turned away by the extractor before the handler runs, so
    /// sweeping the ones that write would prove nothing about them. They need
    /// tests of their own, and most already have them.
    ///
    /// A 404 is a pass and not a hole. Where a path names something, it is asked
    /// about something that is not there — the statement still runs, which is the
    /// whole point, and running is what the favourites of the other API could not
    /// do.
    #[tokio::test]
    async fn every_path_that_reads_answers_without_a_fault_of_ours() {
        let (router, cookie) = a_server().await;
        let (_, reference) = assemble();

        let mut asked = 0;
        let mut broken = Vec::new();

        for (path, item) in reference.paths.paths.iter() {
            if item.get.is_none() {
                continue;
            }

            // Somebody real where a name is wanted, so those paths answer about
            // an account instead of about nothing.
            let uri = path
                .replace("{username}", WHO)
                .replace("{id}", "nothing-by-this-name");

            asked += 1;
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(&uri)
                        .header("cookie", &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();

            if status.is_server_error() {
                broken.push(format!(
                    "{uri}: {status} {}",
                    String::from_utf8_lossy(&body).replace('\n', " ")
                ));
            }
        }

        assert!(
            broken.is_empty(),
            "paths answering with a fault of ours:\n  {}",
            broken.join("\n  ")
        );
        assert!(
            asked > 25,
            "only {asked} paths walked; the document is thin"
        );
    }
}
