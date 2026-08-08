// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Where somebody's listens go besides here.
//!
//! Yours and only yours, all three of these. There is no administrator's version
//! of this screen and there should not be: a scrobbling account belongs to the
//! person who listens, and an administrator who could read these could send
//! somebody else's listening to a service of their own choosing.
//!
//! Not a dangerous change either, so none of it asks for the current password. What
//! that guard is for is somebody taking over an account — the name, the address, the
//! password — and a token for a music website is not that. Getting it wrong costs a
//! wrong destination, which the same screen undoes.
//!
//! **A token goes in and never comes out.** Setting one up sends it; reading the
//! screen back says only that there is one, and what the service called the account
//! it belongs to. Which is also why changing the address means giving the token
//! again: the panel never had it to send back.

use super::error::ApiError;
use super::session::Panel;
use crate::db;
use crate::db::InTurn;
use crate::net::Net;
use crate::scrobble::{self, Service, listenbrainz};
use crate::types::{ErrorBody, NewScrobbler, Offered, Scrobbler, Scrobbling, Switch};
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;
use tracing::{info, warn};

/// Where your listens go
///
/// What is set up, how each one is doing, and every service that could be set up.
/// The catalogue comes from here rather than from the panel so that a service added
/// to the server appears in a panel that knows nothing about it.
#[utoipa::path(
    get,
    path = "/scrobblers",
    tag = "scrobblers",
    responses(
        (status = 200, description = "Where your listens go", body = Scrobbling),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list(
    panel: Panel,
    State(pool): State<SqlitePool>,
) -> Result<Json<Scrobbling>, ApiError> {
    Ok(Json(Scrobbling {
        scrobblers: configured(&pool, panel.user.id).await?,
        offered: catalogue(),
    }))
}

/// Every service there is, in the shape the wire uses. There is one catalogue and
/// it is [`scrobble::EVERY`]; this only dresses it.
fn catalogue() -> Vec<Offered> {
    scrobble::EVERY
        .into_iter()
        .map(|service| Offered {
            service: service.name().to_string(),
            shown: service.shown().to_string(),
            url: service.home().map(str::to_string),
        })
        .collect()
}

/// Send your listens somewhere
///
/// Takes the address and the token, asks the service whether the token is any good,
/// and starts passing listens on.
///
/// The token is checked because the alternative is finding out days later from a
/// queue that will not move. A service that says the token is wrong gets a refusal
/// here and nothing is stored. A service that cannot be reached at all — a machine
/// that is off, an address that resolves to nothing — is stored anyway: refusing to
/// let somebody finish setting up their own scrobbler because it is not switched on
/// yet would be this server deciding what order to do things in.
///
/// Sending it again replaces what was there, which is how a token is renewed and
/// how a self hosted instance moves house.
#[utoipa::path(
    put,
    path = "/scrobblers/{service}",
    tag = "scrobblers",
    params(("service" = String, Path, description = "Which service, as it is named in the catalogue")),
    request_body = NewScrobbler,
    responses(
        (status = 200, description = "Where your listens now go", body = Scrobbler),
        (status = 400, description = "No token, no address for a service that needs one, or a token the service refused", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "No such service", body = ErrorBody),
    )
)]
pub async fn set(
    panel: Panel,
    State(pool): State<SqlitePool>,
    State(net): State<Net>,
    UrlPath(service): UrlPath<String>,
    Json(given): Json<NewScrobbler>,
) -> Result<Json<Scrobbler>, ApiError> {
    let service = scrobble::named(&service).ok_or(ApiError::NotFound)?;

    let token = given.token.trim();
    if token.is_empty() {
        return Err(ApiError::Invalid("A token is needed"));
    }

    let url = whereabouts(service, given.url.as_deref())?;

    // Asked before anything is written, so a refused token leaves no row behind to
    // wonder about.
    let remote_name = vouched(&net, service, &url, token).await?;

    let at = db::now();

    sqlx::query(
        "INSERT INTO scrobblers
              (user_id, service, url, token, remote_name, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, service) DO UPDATE SET
             url         = excluded.url,
             token       = excluded.token,
             remote_name = excluded.remote_name,
             enabled     = excluded.enabled,
             updated_at  = excluded.updated_at",
    )
    .bind(panel.user.id)
    .bind(service.name())
    .bind(&url)
    .bind(token)
    .bind(&remote_name)
    // Absent is yes: somebody who has just typed a token in means to use it.
    .bind(i64::from(given.enabled.unwrap_or(true)))
    .bind(&at)
    .bind(&at)
    .in_turn(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "storing a scrobbling destination"))?;

    // Whatever was waiting is due now. Found by doing it: a token typed wrongly the
    // first time had pushed the queue five minutes out, and correcting the token
    // was answered by nothing happening — for as long as the old wait had left to
    // run, which after a few failures is hours.
    scrobble::due_again(&pool, panel.user.id, service)
        .await
        .map_err(|e| ApiError::internal(e, "making a destination's listens due again"))?;

    info!(
        "listens will be sent to {}{}",
        service.name(),
        remote_name
            .as_deref()
            .map(|name| format!(" as {name}"))
            .unwrap_or_default()
    );

    one(&pool, panel.user.id, service).await
}

/// Start or stop sending
///
/// Keeps everything else, including whatever is waiting: switching off holds the
/// queue where it is and queues nothing new, and switching on again sends what was
/// waiting — at once, rather than after however long the last failure had asked for.
/// Removing the destination is the other thing.
#[utoipa::path(
    patch,
    path = "/scrobblers/{service}",
    tag = "scrobblers",
    params(("service" = String, Path, description = "Which service")),
    request_body = Switch,
    responses(
        (status = 200, description = "Where your listens now go", body = Scrobbler),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "That service is not set up", body = ErrorBody),
    )
)]
pub async fn switch(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(service): UrlPath<String>,
    Json(switch): Json<Switch>,
) -> Result<Json<Scrobbler>, ApiError> {
    let service = scrobble::named(&service).ok_or(ApiError::NotFound)?;

    let changed = sqlx::query(
        "UPDATE scrobblers SET enabled = ?, updated_at = ?
          WHERE user_id = ? AND service = ?",
    )
    .bind(i64::from(switch.enabled))
    .bind(db::now())
    .bind(panel.user.id)
    .bind(service.name())
    .in_turn(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "switching a scrobbling destination"))?;

    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    // Switching it on is somebody saying "go on then", and the reason it was off may
    // well be the reason it was failing.
    if switch.enabled {
        scrobble::due_again(&pool, panel.user.id, service)
            .await
            .map_err(|e| ApiError::internal(e, "making a destination's listens due again"))?;
    }

    one(&pool, panel.user.id, service).await
}

/// Stop sending listens there
///
/// Forgets the address and the token, and drops whatever was still waiting to be
/// sent there — which is the difference between this and switching it off. There is
/// no undoing it: the token is gone and has to be given again.
#[utoipa::path(
    delete,
    path = "/scrobblers/{service}",
    tag = "scrobblers",
    params(("service" = String, Path, description = "Which service")),
    responses(
        (status = 204, description = "Gone, along with anything that was waiting"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 404, description = "That service is not set up", body = ErrorBody),
    )
)]
pub async fn remove(
    panel: Panel,
    State(pool): State<SqlitePool>,
    UrlPath(service): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    let service = scrobble::named(&service).ok_or(ApiError::NotFound)?;

    let gone = sqlx::query("DELETE FROM scrobblers WHERE user_id = ? AND service = ?")
        .bind(panel.user.id)
        .bind(service.name())
        .in_turn(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "removing a scrobbling destination"))?;

    if gone.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Where a service is to be reached: its own address, or the one that was given.
///
/// A service with an address of its own ignores whatever came with the request.
/// There is one ListenBrainz, and letting somebody point "ListenBrainz" at another
/// host would make the name mean nothing — that is what the self hosted entries in
/// the catalogue are for.
fn whereabouts(service: Service, given: Option<&str>) -> Result<String, ApiError> {
    if let Some(home) = service.home() {
        return Ok(home.to_string());
    }

    let given = given.unwrap_or("").trim().trim_end_matches('/');

    if !given.starts_with("http://") && !given.starts_with("https://") {
        return Err(ApiError::Invalid(
            "That service needs an address, starting with http:// or https://",
        ));
    }

    Ok(given.to_string())
}

/// Asks a service what it makes of a token, and answers with the name it gave back.
///
/// Three outcomes, and the middle one is the whole reason this is not a boolean:
///
/// - it says the token is wrong, and the request is refused;
/// - it says nothing this understands — no such call, an HTML page from a proxy in
///   front of it, a machine that is off — and the token is stored unvouched for,
///   because nothing said it was bad;
/// - it says who the token belongs to, which is stored and shown.
async fn vouched(
    net: &Net,
    service: Service,
    url: &str,
    token: &str,
) -> Result<Option<String>, ApiError> {
    let asking = listenbrainz::checking(&service.root(url));

    let answer = match net.get(&asking, token).await {
        Ok(answer) => answer,
        // Not a refusal. Somebody setting up a scrobbler that is not running yet is
        // doing a reasonable thing in a reasonable order.
        Err(e) => {
            warn!("could not check a token with {}: {e:#}", service.name());
            return Ok(None);
        }
    };

    // The one status that is a refusal on its own, whatever the body says.
    if answer.status == 401 {
        return Err(ApiError::TokenRefused);
    }

    match listenbrainz::read_check(&answer) {
        Some(checked) if !checked.valid => Err(ApiError::TokenRefused),
        Some(checked) => Ok(checked.user_name),
        None => {
            warn!(
                "{} did not answer a token check like ListenBrainz: {}",
                service.name(),
                answer.status
            );
            Ok(None)
        }
    }
}

/// A row of `scrobblers` with the state of its queue beside it.
type Row = (
    String,
    String,
    Option<String>,
    bool,
    i64,
    Option<String>,
    Option<String>,
);

/// Everything set up, in the catalogue's order rather than the alphabet's, so the
/// screen does not reorder itself when somebody adds one.
async fn configured(pool: &SqlitePool, user_id: i64) -> Result<Vec<Scrobbler>, ApiError> {
    // The queue is counted here rather than asked for separately, because "3
    // waiting since Tuesday" is the one thing a screen has to be able to say and a
    // second call for it would be a second call every time this is read.
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT s.service, s.url, s.remote_name, s.enabled,
                (SELECT count(*) FROM scrobble_queue q
                  WHERE q.user_id = s.user_id AND q.service = s.service),
                (SELECT min(q.played_at) FROM scrobble_queue q
                  WHERE q.user_id = s.user_id AND q.service = s.service),
                (SELECT q.last_error FROM scrobble_queue q
                  WHERE q.user_id = s.user_id AND q.service = s.service
                    AND q.last_error IS NOT NULL
                  ORDER BY q.played_at DESC LIMIT 1)
           FROM scrobblers s
          WHERE s.user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(|e| ApiError::internal(e, "reading where listens go"))?;

    let mut found: Vec<Scrobbler> = rows
        .into_iter()
        .filter_map(read)
        .collect::<Vec<Scrobbler>>();

    found.sort_by_key(|scrobbler| {
        scrobble::EVERY
            .iter()
            .position(|service| service.name() == scrobbler.service)
    });

    Ok(found)
}

/// One of them, read back after being written, so what the screen gets is what a
/// reload would say.
async fn one(
    pool: &SqlitePool,
    user_id: i64,
    service: Service,
) -> Result<Json<Scrobbler>, ApiError> {
    configured(pool, user_id)
        .await?
        .into_iter()
        .find(|scrobbler| scrobbler.service == service.name())
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// A row as the shape that travels, or nothing for a service this version no longer
/// has — which is the same thing that happens to an old job's history.
fn read(row: Row) -> Option<Scrobbler> {
    let (service, url, remote_name, enabled, waiting, oldest, last_error) = row;
    let known = scrobble::named(&service)?;

    Some(Scrobbler {
        service,
        shown: known.shown().to_string(),
        url,
        remote_name,
        enabled,
        waiting,
        oldest,
        last_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    /// An account with a destination already set up, and one listen waiting to go
    /// out that has been pushed a long way into the future by failures.
    async fn a_destination() -> (SqlitePool, Panel) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&at)
        .bind(&at)
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO scrobblers
                  (user_id, service, url, token, remote_name, enabled, created_at, updated_at)
             VALUES (?, 'listenbrainz', 'https://api.listenbrainz.org', 'a token', 'ana', 1, ?, ?)",
        )
        .bind(user_id)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO scrobble_queue
                  (user_id, service, played_at, title, artist, attempts, next_try_at,
                   last_error, created_at)
             VALUES (?, 'listenbrainz', ?, 'Song', 'Artist', 4, '2999-01-01T00:00:00Z', 'no', ?)",
        )
        .bind(user_id)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        let panel = Panel {
            id: 1,
            user: User {
                id: user_id,
                username: "ana".to_string(),
                is_admin: false,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        };

        (pool, panel)
    }

    async fn waiting_until(pool: &SqlitePool) -> String {
        sqlx::query_scalar("SELECT next_try_at FROM scrobble_queue")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// A name no service answers to is a miss, and it is checked before anything
    /// else — so a request naming one neither writes nor asks the network.
    #[tokio::test]
    async fn a_service_nobody_offers_is_a_miss_on_all_three_calls() {
        let (pool, panel) = a_destination().await;

        assert!(matches!(
            switch(
                panel.clone_for_test(),
                State(pool.clone()),
                UrlPath("spotify".to_string()),
                Json(Switch { enabled: true }),
            )
            .await
            .expect_err("no such service"),
            ApiError::NotFound
        ));

        assert!(matches!(
            remove(
                panel.clone_for_test(),
                State(pool.clone()),
                UrlPath("spotify".to_string()),
            )
            .await
            .expect_err("no such service"),
            ApiError::NotFound
        ));

        assert!(matches!(
            set(
                panel,
                State(pool.clone()),
                State(Net::new()),
                UrlPath("spotify".to_string()),
                Json(NewScrobbler {
                    url: None,
                    token: "a token".to_string(),
                    enabled: None,
                }),
            )
            .await
            .expect_err("no such service"),
            ApiError::NotFound
        ));
    }

    /// The two ways a request to set one up is refused before the network is
    /// touched at all, which is what keeps a refused token from leaving a row
    /// behind to wonder about.
    #[tokio::test]
    async fn a_destination_that_cannot_be_reached_for_is_refused_before_asking() {
        let (pool, panel) = a_destination().await;

        let no_token = set(
            panel.clone_for_test(),
            State(pool.clone()),
            State(Net::new()),
            UrlPath("listenbrainz".to_string()),
            Json(NewScrobbler {
                url: None,
                token: "   ".to_string(),
                enabled: None,
            }),
        )
        .await
        .expect_err("whitespace is not a token");
        assert!(matches!(no_token, ApiError::Invalid(_)));

        // Koito runs on somebody's own machine, so it has no address of its own and
        // one has to be given.
        let no_address = set(
            panel,
            State(pool.clone()),
            State(Net::new()),
            UrlPath("koito".to_string()),
            Json(NewScrobbler {
                url: Some("kitchen.lan:4110".to_string()),
                token: "a token".to_string(),
                enabled: None,
            }),
        )
        .await
        .expect_err("an address has to say how to reach it");
        assert!(matches!(no_address, ApiError::Invalid(_)));

        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM scrobblers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1, "neither left anything behind");
    }

    /// Switching one back on says "go on then", and what was waiting is due now.
    ///
    /// Found by doing it: a token typed wrongly pushes the queue further out with
    /// every failure, and after a few that is hours. Without this, correcting the
    /// destination is answered by nothing happening for the rest of that wait.
    #[tokio::test]
    async fn switching_a_destination_on_makes_what_was_waiting_due_now() {
        let (pool, panel) = a_destination().await;

        let _ = switch(
            panel.clone_for_test(),
            State(pool.clone()),
            UrlPath("listenbrainz".to_string()),
            Json(Switch { enabled: false }),
        )
        .await
        .unwrap();

        assert_eq!(
            waiting_until(&pool).await,
            "2999-01-01T00:00:00Z",
            "switching it off is not an invitation to try again"
        );

        let Json(back) = switch(
            panel,
            State(pool.clone()),
            UrlPath("listenbrainz".to_string()),
            Json(Switch { enabled: true }),
        )
        .await
        .unwrap();

        assert!(back.enabled);
        assert!(
            waiting_until(&pool).await.as_str() < "2999-01-01T00:00:00Z",
            "and switching it on brings the wait back to now"
        );
    }

    /// Removing one that is not set up is a miss rather than a silent success, so
    /// a panel showing a destination that is not there finds out.
    #[tokio::test]
    async fn removing_a_destination_takes_it_and_missing_one_says_so() {
        let (pool, panel) = a_destination().await;

        assert_eq!(
            remove(
                panel.clone_for_test(),
                State(pool.clone()),
                UrlPath("listenbrainz".to_string()),
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM scrobblers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);

        let again = remove(
            panel,
            State(pool.clone()),
            UrlPath("listenbrainz".to_string()),
        )
        .await
        .expect_err("it is gone already");
        assert!(matches!(again, ApiError::NotFound));
    }

    /// Switching one this account never set up is a miss too, and not a row
    /// quietly created.
    #[tokio::test]
    async fn switching_a_destination_that_was_never_set_up_is_a_miss() {
        let (pool, panel) = a_destination().await;

        let missed = switch(
            panel,
            State(pool.clone()),
            UrlPath("koito".to_string()),
            Json(Switch { enabled: true }),
        )
        .await
        .expect_err("she never set Koito up");
        assert!(matches!(missed, ApiError::NotFound));

        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM scrobblers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1, "and nothing was created by asking");
    }
}
