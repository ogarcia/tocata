// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How the panel looks and speaks, for whoever is asking.
//!
//! These belong to the account rather than to the browser, so logging in from
//! somewhere else brings them along. That is the only reason they are on this
//! side: nothing the server does depends on any of them.
//!
//! Which is why the values are opaque here. What a theme or an accent can be is
//! the panel's business, and a server that validated the list would have to be
//! migrated every time a colour was added. What this checks is that a value looks
//! like an identifier — short, no spaces, nothing to escape — and the panel
//! ignores anything it does not recognise.
//!
//! There is no call to read them. They arrive with the session, because the panel
//! needs them before it draws anything and a second round trip would mean painting
//! in the wrong theme first and correcting afterwards.

use super::error::ApiError;
use super::session::Panel;
use crate::types::{ErrorBody, PreferenceChanges, Preferences};
use axum::Json;
use axum::extract::State;
use sqlx::SqlitePool;

/// As long as an identifier here can be. Nothing legitimate comes close; the
/// limit is here so a row cannot be used as somewhere to keep a document.
const LONGEST: usize = 32;

/// What the panel has chosen, or nothing at all.
///
/// No row is the same answer as a row of nulls, which is what lets the first
/// choice be an insert without anything having to seed a row per account.
pub async fn load(pool: &SqlitePool, user_id: i64) -> Result<Preferences, ApiError> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT theme, locale, accent FROM panel_preferences WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::internal(e, "reading the panel preferences"))?;

    Ok(
        row.map_or_else(Preferences::default, |(theme, locale, accent)| {
            Preferences {
                theme,
                locale,
                accent,
            }
        }),
    )
}

/// Change the panel preferences
///
/// Yours, and only yours: these are not administered. Anything left out is left
/// alone, and an explicit `null` unchooses — which is a thing to be able to do,
/// since following the machine's theme or the browser's language is somewhere
/// somebody may want to go back to.
///
/// The values are identifiers the panel understands and the server does not
/// interpret. One it does not recognise is not an error here and shows as the
/// panel's own default there.
///
/// They are read back with the session rather than from a call of their own.
#[utoipa::path(
    patch,
    path = "/preferences",
    tag = "preferences",
    request_body = PreferenceChanges,
    responses(
        (status = 200, description = "The preferences as they now are", body = Preferences),
        (status = 400, description = "Nothing to change, or not an identifier", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn change(
    panel: Panel,
    State(pool): State<SqlitePool>,
    Json(changes): Json<PreferenceChanges>,
) -> Result<Json<Preferences>, ApiError> {
    if changes.theme.is_none() && changes.locale.is_none() && changes.accent.is_none() {
        return Err(ApiError::Invalid("Nothing to change was given"));
    }

    let theme = settled(changes.theme)?;
    let locale = settled(changes.locale)?;
    let accent = settled(changes.accent)?;

    // One statement for a row that may not exist yet, and for fields that may be
    // meant to become null — which is why the mention travels as its own bind
    // rather than as a null value. A coalesce cannot tell "leave it" from "clear
    // it", and those are different requests.
    sqlx::query(
        "INSERT INTO panel_preferences (user_id, theme, locale, accent)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE
            SET theme  = CASE WHEN ? THEN excluded.theme  ELSE theme  END,
                locale = CASE WHEN ? THEN excluded.locale ELSE locale END,
                accent = CASE WHEN ? THEN excluded.accent ELSE accent END",
    )
    .bind(panel.user.id)
    .bind(theme.clone().flatten())
    .bind(locale.clone().flatten())
    .bind(accent.clone().flatten())
    .bind(theme.is_some())
    .bind(locale.is_some())
    .bind(accent.is_some())
    .execute(&pool)
    .await
    .map_err(|e| ApiError::internal(e, "changing the panel preferences"))?;

    // Read back rather than echoed, so what comes out is what a reload would say.
    load(&pool, panel.user.id).await.map(Json)
}

/// Trims a mentioned value, turns an empty one into no choice at all, and refuses
/// anything that is not an identifier.
///
/// Emptied rather than refused because clearing a field by clearing it is what
/// somebody would expect, and the alternative is a panel that has to know to send
/// null for a box it has just seen somebody empty.
fn settled(given: Option<Option<String>>) -> Result<Option<Option<String>>, ApiError> {
    let Some(value) = given else {
        return Ok(None);
    };

    let value = value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if let Some(value) = value.as_deref()
        && (value.len() > LONGEST || !value.chars().all(is_identifier))
    {
        return Err(ApiError::Invalid(
            "A preference must be a short identifier: letters, digits, - and _",
        ));
    }

    Ok(Some(value))
}

fn is_identifier(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::user::User;

    async fn an_account() -> (SqlitePool, Panel) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let timestamp = db::now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES ('ana', 'x', 0, ?, ?) RETURNING id",
        )
        .bind(&timestamp)
        .bind(&timestamp)
        .fetch_one(&pool)
        .await
        .unwrap();

        let panel = Panel {
            id: 1,
            user: User {
                id,
                username: "ana".to_string(),
                is_admin: false,
            },
            expires_at: "2999-01-01T00:00:00Z".to_string(),
        };

        (pool, panel)
    }

    fn panel_like(panel: &Panel) -> Panel {
        Panel {
            id: panel.id,
            user: panel.user.clone(),
            expires_at: panel.expires_at.clone(),
        }
    }

    fn asking(json: &str) -> Json<PreferenceChanges> {
        Json(serde_json::from_str(json).unwrap())
    }

    /// An account that has chosen nothing has chosen nothing, rather than having
    /// been given a row of the server's guesses.
    #[tokio::test]
    async fn nothing_chosen_is_nothing_returned() {
        let (pool, panel) = an_account().await;

        let preferences = load(&pool, panel.user.id).await.unwrap();

        assert_eq!(preferences, Preferences::default());
        assert_eq!(preferences.theme, None, "follow the machine");
    }

    /// The first choice creates the row, and the second one leaves the first
    /// alone: two preferences changed from two screens must not undo each other.
    #[tokio::test]
    async fn one_choice_does_not_disturb_another() {
        let (pool, panel) = an_account().await;

        let Json(after_theme) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"theme":"dark"}"#),
        )
        .await
        .unwrap();

        assert_eq!(after_theme.theme.as_deref(), Some("dark"));
        assert_eq!(after_theme.accent, None, "not asked for");

        let Json(after_accent) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"accent":"plum"}"#),
        )
        .await
        .unwrap();

        assert_eq!(after_accent.accent.as_deref(), Some("plum"));
        assert_eq!(after_accent.theme.as_deref(), Some("dark"), "still dark");
    }

    /// Going back to following the machine, which is the case a coalesce over the
    /// value alone could not express: null there means "leave it".
    #[tokio::test]
    async fn a_choice_can_be_unmade() {
        let (pool, panel) = an_account().await;

        let Json(_) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"theme":"light","locale":"es"}"#),
        )
        .await
        .unwrap();

        let Json(now) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"theme":null}"#),
        )
        .await
        .unwrap();

        assert_eq!(now.theme, None, "back to the machine");
        assert_eq!(now.locale.as_deref(), Some("es"), "left alone");
    }

    /// Emptying the box says the same as unchoosing, because that is what somebody
    /// emptying a box means.
    #[tokio::test]
    async fn an_emptied_value_is_no_choice() {
        let (pool, panel) = an_account().await;

        let Json(_) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"accent":"plum"}"#),
        )
        .await
        .unwrap();

        let Json(now) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"accent":"  "}"#),
        )
        .await
        .unwrap();

        assert_eq!(now.accent, None);
    }

    /// The server does not know what the accents are called, so what it checks is
    /// the shape: an identifier, and a short one.
    #[tokio::test]
    async fn something_that_is_not_an_identifier_is_refused() {
        let (pool, panel) = an_account().await;

        for given in [
            r#"{"theme":"dark mode"}"#,
            r#"{"accent":"<script>"}"#,
            r#"{"locale":"es;DROP"}"#,
            r#"{"accent":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        ] {
            let refused = change(panel_like(&panel), State(pool.clone()), asking(given)).await;

            assert!(
                matches!(refused, Err(ApiError::Invalid(_))),
                "{given} should not be storable"
            );
        }

        assert_eq!(
            load(&pool, panel.user.id).await.unwrap(),
            Preferences::default(),
            "and nothing was written on the way to being refused"
        );
    }

    /// A locale with a region in it is an identifier, and has to stay one.
    #[tokio::test]
    async fn a_region_in_a_locale_is_allowed() {
        let (pool, panel) = an_account().await;

        let Json(now) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"locale":"pt-BR"}"#),
        )
        .await
        .unwrap();

        assert_eq!(now.locale.as_deref(), Some("pt-BR"));
    }

    #[tokio::test]
    async fn a_request_that_changes_nothing_is_refused() {
        let (pool, panel) = an_account().await;

        let refused = change(panel_like(&panel), State(pool.clone()), asking("{}")).await;

        assert!(matches!(refused, Err(ApiError::Invalid(_))));
    }

    /// Deleting the account takes these with it. Their own table means their own
    /// chance to be left behind, and the foreign key is what prevents it.
    #[tokio::test]
    async fn losing_the_account_loses_the_preferences() {
        let (pool, panel) = an_account().await;

        let Json(_) = change(
            panel_like(&panel),
            State(pool.clone()),
            asking(r#"{"theme":"dark"}"#),
        )
        .await
        .unwrap();

        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(panel.user.id)
            .execute(&pool)
            .await
            .unwrap();

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM panel_preferences")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(left, 0);
    }
}
