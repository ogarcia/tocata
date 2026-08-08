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
use crate::types::{Closed, ErrorBody, Login};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;

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
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, created_at, last_seen_at, expires_at
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
            .map(|(id, created_at, last_seen_at, expires_at)| Login {
                id,
                created_at,
                last_seen_at,
                expires_at,
                current: id == panel.id,
            })
            .collect(),
    ))
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
            session::create(&pool, user_id, A_MONTH).await.unwrap();
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
        session::create(&pool, other, A_MONTH).await.unwrap();

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

    fn panel_like(panel: &Panel) -> Panel {
        Panel {
            id: panel.id,
            user: panel.user.clone(),
            expires_at: panel.expires_at.clone(),
        }
    }
}
