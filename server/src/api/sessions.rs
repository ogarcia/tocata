// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The panel logins an account has open, and closing them.
//!
//! Every other thing this API creates could already be taken back: a library
//! removed, an account deleted, a key revoked. A session could only be counted.
//! That left the one credential nobody could withdraw, which matters most in the
//! case a person is most likely to be in a hurry over — a browser left open
//! somewhere it should not have been.
//!
//! There is no token here, in or out. A session is pointed at by its row, so
//! ending somebody else's never needs the thing that would let you use it.

use super::error::ApiError;
use super::session::Panel;
use crate::db::InTurn;
use crate::session;
use crate::types::{Closed, ErrorBody, Login, Naming};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;

/// Every column the listing selects, in the order it selects them.
type SessionRow = (i64, Option<String>, Option<String>, String, String, String);

/// Anybody may manage their own sessions; only an administrator somebody else's.
///
/// Returns the account's own identifier, which is what the rows hang off.
async fn owner(pool: &SqlitePool, panel: &Panel, username: &str) -> Result<i64, ApiError> {
    if panel.user.username != username && !panel.user.is_admin {
        return Err(ApiError::NotAuthorized);
    }

    sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
        .map_err(|e| ApiError::internal(e, "looking up an account"))?
        .ok_or(ApiError::NotFound)
}

/// List sessions
///
/// The panel logins an account has open. Yours, or anybody's if you administer
/// the server. Expired ones are not shown: they are not open.
#[utoipa::path(
    get,
    path = "/users/{username}/sessions",
    tag = "sessions",
    params(("username" = String, Path, description = "Whose sessions")),
    responses(
        (status = 200, description = "Every session still open", body = Vec<Login>),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's sessions", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn list(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Vec<Login>>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    // Expired rows are only cleared on the next login, so they can still be
    // sitting here. Showing one as an open session would be a lie.
    let rows: Vec<SessionRow> = sqlx::query_as(
        "SELECT id, label, user_agent, created_at, last_seen_at, expires_at
           FROM sessions WHERE user_id = ? AND expires_at > ?
          ORDER BY created_at",
    )
    .bind(user_id)
    .bind(crate::db::now())
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "listing sessions"))?;

    Ok(Json(
        rows.into_iter()
            .map(
                |(id, label, user_agent, created_at, last_seen_at, expires_at)| {
                    // Read on the way out rather than stored read. The sentence in
                    // the row is what the browser said; which browser that is, is a
                    // guess this server is free to get better at.
                    let (browser, system) = user_agent
                        .as_deref()
                        .map(crate::browser::read)
                        .unwrap_or_default();

                    Login {
                        id,
                        label,
                        browser: browser.map(str::to_string),
                        system: system.map(str::to_string),
                        created_at,
                        last_seen_at,
                        expires_at,
                        current: id == panel.id,
                    }
                },
            )
            .collect(),
    ))
}

/// Name a session
///
/// Gives one login a name of somebody's own choosing, or takes away the name it
/// has when what arrives is blank. Everything else about a session is either the
/// server's record of it or a guess read off what the browser said; this is the one
/// field a person writes, and it is the one that cannot be wrong.
///
/// Yours, or anybody's if you administer the server — the same reach as closing
/// one, and naming is the lesser of the two.
#[utoipa::path(
    patch,
    path = "/users/{username}/sessions/{id}",
    tag = "sessions",
    params(
        ("username" = String, Path, description = "Whose session"),
        ("id" = i64, Path, description = "Which one, from the listing"),
    ),
    request_body = Naming,
    responses(
        (status = 204, description = "Named"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's session", body = ErrorBody),
        (status = 404, description = "No such account, or no such session of theirs", body = ErrorBody),
    )
)]
pub async fn name(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
    Json(naming): Json<Naming>,
) -> Result<StatusCode, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let label = naming.label.trim();
    // A row that says nothing is a null and not an empty string, so that "has no
    // name" is one state in the table rather than two that read the same.
    let label = (!label.is_empty()).then_some(label);

    // Scoped to the account named in the path for the same reason closing one is:
    // an id on its own would otherwise be a way to write on a stranger's row.
    let done = sqlx::query("UPDATE sessions SET label = ? WHERE id = ? AND user_id = ?")
        .bind(label)
        .bind(id)
        .bind(user_id)
        .in_turn(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "naming a session"))?;

    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Close a session
///
/// Ends one login. Closing your own is the same as logging out, except that the
/// browser keeps a cookie that no longer resolves.
#[utoipa::path(
    delete,
    path = "/users/{username}/sessions/{id}",
    tag = "sessions",
    params(
        ("username" = String, Path, description = "Whose session"),
        ("id" = i64, Path, description = "Which one, from the listing"),
    ),
    responses(
        (status = 204, description = "Closed"),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's session", body = ErrorBody),
        (status = 404, description = "No such account, or no such session of theirs", body = ErrorBody),
    )
)]
pub async fn close(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path((username, id)): Path<(String, i64)>,
) -> Result<StatusCode, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    // Scoped to the account named in the path, so an id belonging to somebody
    // else is a miss rather than a way to close a stranger's session.
    let done = sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .in_turn(&pool)
        .await
        .map_err(|e| ApiError::internal(e, "closing a session"))?;

    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Close every other session
///
/// Ends all of the account's logins except the one asking. Somebody reaching for
/// this has left a browser open somewhere it should not have been, and what they
/// want is every one of those closed — not to be thrown out of the screen they
/// are doing it from. Leaving is a separate act, with a button of its own that
/// says so.
///
/// An administrator closing somebody else's sessions closes all of them, because
/// none of them is the one asking.
#[utoipa::path(
    delete,
    path = "/users/{username}/sessions",
    tag = "sessions",
    params(("username" = String, Path, description = "Whose sessions")),
    responses(
        (status = 200, description = "How many were closed", body = Closed),
        (status = 401, description = "No valid session", body = ErrorBody),
        (status = 403, description = "Somebody else's sessions", body = ErrorBody),
        (status = 404, description = "No such account", body = ErrorBody),
    )
)]
pub async fn close_all(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Path(username): Path<String>,
) -> Result<Json<Closed>, ApiError> {
    let user_id = owner(&pool, &panel, &username).await?;

    let closed = session::destroy_all(&pool, user_id, panel.id)
        .await
        .map_err(|e| ApiError::internal(e, "closing an account's sessions"))?;

    Ok(Json(Closed { closed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::A_MONTH;
    use crate::user::User;

    /// An account with three browsers logged in, one of which is asking.
    async fn logged_in_thrice() -> (SqlitePool, Panel) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let timestamp = crate::db::now();
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        for _ in 0..3 {
            session::create(&pool, user_id, A_MONTH, None)
                .await
                .unwrap();
        }

        // The middle one, so that neither the first nor the last row surviving
        // could be mistaken for the right answer arrived at by accident.
        let asking: i64 = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE user_id = ? ORDER BY id LIMIT 1 OFFSET 1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let panel = Panel {
            id: asking,
            user: User {
                id: user_id,
                username: "ana".to_string(),
                is_admin: false,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        };

        (pool, panel)
    }

    /// This is somebody's own Access screen, and the only thing that takes them
    /// out of it is logging out. Closing the browsers they left open elsewhere
    /// must not close the one they are doing it from.
    #[tokio::test]
    async fn closing_every_session_leaves_the_one_asking() {
        let (pool, panel) = logged_in_thrice().await;

        let Json(counted) = close_all(
            panel_like(&panel),
            State(pool.clone()),
            Path("ana".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(counted.closed, 2);

        let Json(left) = list(panel_like(&panel), State(pool.clone()), Path("ana".into()))
            .await
            .unwrap();

        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, panel.id);
        assert!(left[0].current, "and it is the one still being used");
    }

    /// A session id belonging to somebody else is a miss and not a way to throw a
    /// stranger out of the panel — the id alone would be enough if the account in
    /// the path were not part of the question.
    #[tokio::test]
    async fn a_session_of_another_account_cannot_be_closed_through_this_one() {
        let (pool, panel) = logged_in_thrice().await;

        let timestamp = crate::db::now();
        let other: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('beto', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();
        session::create(&pool, other, A_MONTH, None).await.unwrap();

        let his: i64 = sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = ?")
            .bind(other)
            .fetch_one(&pool)
            .await
            .unwrap();

        // His id, asked for under her name, which is the shape that would work if
        // the account in the path were decoration.
        let missed = close(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), his)),
        )
        .await
        .expect_err("his session is not hers to close");
        assert!(matches!(missed, ApiError::NotFound));

        // And asked for under his name, by her, which is the other way to try it.
        let refused = close(
            panel_like(&panel),
            State(pool.clone()),
            Path(("beto".to_string(), his)),
        )
        .await
        .expect_err("and she is not an administrator");
        assert!(matches!(refused, ApiError::NotAuthorized));

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = ?")
            .bind(other)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 1, "he is still logged in");
    }

    /// Her own, which is what the screen is for.
    #[tokio::test]
    async fn closing_one_of_her_own_sessions_closes_that_one() {
        let (pool, panel) = logged_in_thrice().await;

        let another: i64 = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE user_id = ? AND id != ? ORDER BY id LIMIT 1",
        )
        .bind(panel.user.id)
        .bind(panel.id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            close(
                panel_like(&panel),
                State(pool.clone()),
                Path(("ana".to_string(), another)),
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );

        let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM sessions ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left.len(), 2, "the one closed and no others");
        assert!(
            left.contains(&panel.id),
            "including the one she is asking from"
        );
    }

    /// The one row a person writes on. Blank is the way back to a row that says
    /// nothing, which is where every session starts.
    #[tokio::test]
    async fn naming_a_session_says_it_and_blank_unsays_it() {
        let (pool, panel) = logged_in_thrice().await;

        let named = |label: &str| {
            Json(Naming {
                label: label.to_string(),
            })
        };

        assert_eq!(
            name(
                panel_like(&panel),
                State(pool.clone()),
                Path(("ana".to_string(), panel.id)),
                named("  the laptop in the kitchen  "),
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );

        let Json(list) = list(panel_like(&panel), State(pool.clone()), Path("ana".into()))
            .await
            .unwrap();
        let mine = list.iter().find(|login| login.current).unwrap();
        assert_eq!(
            mine.label.as_deref(),
            Some("the laptop in the kitchen"),
            "and trimmed, since the spaces around it are typing rather than a name"
        );

        name(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), panel.id)),
            named("   "),
        )
        .await
        .unwrap();

        let written: Option<String> = sqlx::query_scalar("SELECT label FROM sessions WHERE id = ?")
            .bind(panel.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(written, None, "a name taken away is a null and not a blank");
    }

    /// The name is nobody else's to write, by the same rule that says whose session
    /// is whose to close.
    #[tokio::test]
    async fn a_session_of_another_account_cannot_be_named_through_this_one() {
        let (pool, panel) = logged_in_thrice().await;

        let timestamp = crate::db::now();
        let other: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('beto', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();
        session::create(&pool, other, A_MONTH, None).await.unwrap();

        let his: i64 = sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = ?")
            .bind(other)
            .fetch_one(&pool)
            .await
            .unwrap();

        let missed = name(
            panel_like(&panel),
            State(pool.clone()),
            Path(("ana".to_string(), his)),
            Json(Naming {
                label: "his".to_string(),
            }),
        )
        .await
        .expect_err("his session is not hers to name");
        assert!(matches!(missed, ApiError::NotFound));

        let refused = name(
            panel_like(&panel),
            State(pool.clone()),
            Path(("beto".to_string(), his)),
            Json(Naming {
                label: "his".to_string(),
            }),
        )
        .await
        .expect_err("and she is not an administrator");
        assert!(matches!(refused, ApiError::NotAuthorized));

        let written: Option<String> = sqlx::query_scalar("SELECT label FROM sessions WHERE id = ?")
            .bind(his)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(written, None, "his row is untouched");
    }

    /// What the browser said, read on the way out. Two rows, because the answer for
    /// a login that said nothing has to be nothing rather than a guess.
    #[tokio::test]
    async fn a_session_is_listed_as_the_browser_it_was_opened_from() {
        let (pool, panel) = logged_in_thrice().await;

        session::create(
            &pool,
            panel.user.id,
            A_MONTH,
            Some(
                "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0"
                    .to_string(),
            ),
        )
        .await
        .unwrap();

        let Json(list) = list(panel_like(&panel), State(pool.clone()), Path("ana".into()))
            .await
            .unwrap();

        let read: Vec<(Option<String>, Option<String>)> = list
            .iter()
            .map(|login| (login.browser.clone(), login.system.clone()))
            .collect();

        assert_eq!(
            read.iter()
                .filter(|pair| **pair == (Some("Firefox".to_string()), Some("Linux".to_string())))
                .count(),
            1,
            "the one opened from a browser"
        );
        assert_eq!(
            read.iter().filter(|pair| **pair == (None, None)).count(),
            3,
            "and the three that said nothing say nothing"
        );
    }

    fn panel_like(panel: &Panel) -> Panel {
        Panel {
            id: panel.id,
            user: panel.user.clone(),
            expires_at: panel.expires_at.clone(),
        }
    }
}
