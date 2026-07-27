// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Errors as the API defines them.
//!
//! These travel with HTTP 200 and a `status="failed"` envelope, which is how
//! the protocol works: the transport succeeded, the call did not.

use super::response::{self, Format};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Somewhere the user can go when authentication is refused, as the
/// apiKeyAuthentication extension asks for.
const HELP_URL: &str = env!("CARGO_PKG_REPOSITORY");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 10 — a required parameter is missing.
    MissingParameter(&'static str),
    /// 40 — wrong username or password.
    WrongCredentials,
    /// 42 — the mechanism the client offered is not one this server accepts.
    MechanismNotSupported,
    /// 43 — several conflicting mechanisms in the same request.
    ConflictingMechanisms,
    /// 44 — the API key is not valid.
    InvalidApiKey,
    /// 70 — no such thing here.
    NotFound,
    /// 50 — authenticated, but not allowed to do this.
    ///
    /// Not 40: a client that cannot tell the difference will ask the user to
    /// retype a password that was never the problem.
    NotAuthorized,
    /// 0 — anything unexpected on our side.
    Internal,
    /// 0 with something specific to say. For the refusals the protocol has no
    /// code for, where a bare "an internal error occurred" would be a lie.
    Generic(String),
}

impl ApiError {
    pub fn code(&self) -> u16 {
        match self {
            Self::MissingParameter(_) => 10,
            Self::WrongCredentials => 40,
            Self::MechanismNotSupported => 42,
            Self::ConflictingMechanisms => 43,
            Self::InvalidApiKey => 44,
            Self::NotFound => 70,
            Self::NotAuthorized => 50,
            Self::Internal | Self::Generic(_) => 0,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::MissingParameter(name) => format!("Required parameter {name} is missing"),
            Self::WrongCredentials => "Wrong username or password".into(),
            Self::MechanismNotSupported => "Provided authentication mechanism not supported".into(),
            Self::ConflictingMechanisms => {
                "Multiple conflicting authentication mechanisms provided".into()
            }
            Self::InvalidApiKey => "Invalid API key".into(),
            Self::NotFound => "The requested data was not found".into(),
            Self::NotAuthorized => "The user is not authorized for the given operation".into(),
            Self::Internal => "An internal error occurred".into(),
            Self::Generic(message) => message.clone(),
        }
    }

    /// Only the authentication failures point somewhere useful; on the rest a
    /// help link would be noise.
    fn help_url(&self) -> Option<&'static str> {
        match self {
            Self::MechanismNotSupported | Self::ConflictingMechanisms | Self::InvalidApiKey => {
                Some(HELP_URL)
            }
            _ => None,
        }
    }

    /// Pairs the error with the format the client asked for, so the failure
    /// comes back in the shape it expects.
    pub fn in_format(self, format: Format) -> Failure {
        Failure {
            format,
            error: self,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    code: u16,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    help_url: Option<&'static str>,
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

/// An `ApiError` that knows how to render itself.
#[derive(Debug)]
pub struct Failure {
    format: Format,
    error: ApiError,
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        response::failed(
            self.format,
            ErrorEnvelope {
                error: ErrorBody {
                    code: self.error.code(),
                    message: self.error.message(),
                    help_url: self.error.help_url(),
                },
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_the_specification() {
        assert_eq!(ApiError::MissingParameter("u").code(), 10);
        assert_eq!(ApiError::WrongCredentials.code(), 40);
        assert_eq!(ApiError::MechanismNotSupported.code(), 42);
        assert_eq!(ApiError::ConflictingMechanisms.code(), 43);
        assert_eq!(ApiError::InvalidApiKey.code(), 44);
        assert_eq!(ApiError::NotAuthorized.code(), 50);
        assert_eq!(ApiError::NotFound.code(), 70);
        assert_eq!(ApiError::Internal.code(), 0);
    }

    #[test]
    fn only_authentication_errors_carry_a_help_url() {
        assert!(ApiError::InvalidApiKey.help_url().is_some());
        assert!(ApiError::ConflictingMechanisms.help_url().is_some());
        assert!(ApiError::MechanismNotSupported.help_url().is_some());
        assert!(ApiError::WrongCredentials.help_url().is_none());
        assert!(ApiError::NotAuthorized.help_url().is_none());
        assert!(ApiError::Internal.help_url().is_none());
    }
}
