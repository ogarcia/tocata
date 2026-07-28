// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Failures, as HTTP means them.
//!
//! Nothing here borrows the `/rest` envelope. That envelope exists because the
//! OpenSubsonic specification requires HTTP 200 on a failure, which leaves the
//! status line saying nothing and every client reaching into the body to find
//! out what happened. Here a 401 is a 401.
//!
//! Each failure carries a stable machine readable `code` and an English
//! `message`. The panel reads the code and chooses its own words, in the reader's
//! own language; the message is for whoever is looking at the raw response, and
//! is never what a user is shown.

use crate::types::ErrorBody;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tracing::error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 401 — no session, or one that has run out.
    NotAuthenticated,
    /// 401 — a login attempt that did not check out.
    WrongCredentials,
    /// 403 — a real session, but not one allowed to do this.
    NotAuthorized,
    /// 403 — the password given to confirm a change of your own is not the one on
    /// the account.
    ///
    /// Not 401, which is what a wrong password means when logging in: here the
    /// session is fine and stays fine, and 401 is what the panel reads as "your
    /// session is gone" before sending somebody back to the login form. Being told
    /// off for a typo should not cost anybody their session.
    WrongPassword,
    /// 404 — no such thing.
    NotFound,
    /// 400 — the request itself does not make sense.
    Invalid(&'static str),
    /// 409 — the request makes sense but conflicts with what is already going on.
    Conflict(&'static str),
    /// 500 — our fault.
    Internal,
}

impl ApiError {
    /// The stable name of the failure. Adding one is a feature; renaming one is a
    /// breaking change, which is what the `/v1` in the path is for.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAuthenticated => "notAuthenticated",
            Self::WrongCredentials => "wrongCredentials",
            Self::NotAuthorized => "notAuthorized",
            Self::WrongPassword => "wrongPassword",
            Self::NotFound => "notFound",
            Self::Invalid(_) => "invalidRequest",
            Self::Conflict(_) => "conflict",
            Self::Internal => "internalError",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            // Not 403 for either: 403 says "you, but not this", and a request
            // with no session at all has not said who "you" is yet.
            Self::NotAuthenticated | Self::WrongCredentials => StatusCode::UNAUTHORIZED,
            Self::NotAuthorized | Self::WrongPassword => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::NotAuthenticated => "No valid session",
            Self::WrongCredentials => "Wrong username or password",
            Self::NotAuthorized => "Not allowed to perform this operation",
            Self::WrongPassword => "That is not the current password",
            Self::NotFound => "No such thing",
            Self::Invalid(detail) | Self::Conflict(detail) => detail,
            Self::Internal => "An internal error occurred",
        }
    }

    /// Logs the cause and hides it. What went wrong on our side belongs in the
    /// server's log, not in an answer to whoever happened to ask.
    pub fn internal<E: std::fmt::Display>(error: E, doing: &str) -> Self {
        error!("{doing}: {error}");
        Self::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(ErrorBody {
                code: self.code().to_string(),
                message: self.message().to_string(),
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_say_what_happened() {
        assert_eq!(
            ApiError::NotAuthenticated.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ApiError::WrongCredentials.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn codes_are_camel_case_and_distinct() {
        let codes = [
            ApiError::NotAuthenticated.code(),
            ApiError::WrongCredentials.code(),
            ApiError::NotAuthorized.code(),
            ApiError::Conflict("x").code(),
            ApiError::NotFound.code(),
            ApiError::Invalid("x").code(),
            ApiError::Internal.code(),
        ];

        for code in codes {
            assert!(
                !code.contains('_') && code.starts_with(|c: char| c.is_ascii_lowercase()),
                "{code} is not camelCase"
            );
        }

        let distinct: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(distinct.len(), codes.len(), "two failures share a code");
    }
}
