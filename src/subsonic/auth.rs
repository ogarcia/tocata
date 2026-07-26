// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Authenticating a request against the API's several mechanisms.

use super::error::{ApiError, Failure};
use super::response::Format;
use crate::auth;
use crate::user::{self, User};
use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::error;

/// The authentication parameters a request may carry.
///
/// The rename is load bearing: the extension spells the key `apiKey`, and
/// without it serde looks for `api_key`, never finds it, and every api key
/// request degrades into "missing parameter u".
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthParams {
    /// Requested response format, read first so a rejection comes back in the
    /// shape the client expects.
    f: Option<String>,
    /// Username, for the mechanisms that use one.
    u: Option<String>,
    /// Password, plain or hex encoded behind `enc:`.
    p: Option<String>,
    /// Salted token, `md5(password + salt)`.
    t: Option<String>,
    /// Salt that went into the token.
    s: Option<String>,
    /// Key from the apiKeyAuthentication extension.
    api_key: Option<String>,
}

/// What a request offers as proof, once the combination has been validated.
///
/// Modelling this as a closed set rather than a chain of conditionals is what
/// makes the conflict rules of the extension expressible: illegal combinations
/// cannot be represented, so they are rejected in one place instead of being
/// checked again at every use.
#[derive(Debug, PartialEq, Eq)]
enum Credentials {
    ApiKey(String),
    Password {
        username: String,
        password: String,
    },
    /// The legacy salted token. Kept as a variant because recognising it is
    /// what lets us answer 42 instead of a confusing 40.
    SaltedToken,
}

impl AuthParams {
    fn credentials(&self) -> Result<Credentials, ApiError> {
        let has_user = self.u.is_some();
        let has_password = self.p.is_some();
        let has_token = self.t.is_some() || self.s.is_some();

        // The extension is explicit: an api key must travel alone.
        if let Some(key) = &self.api_key {
            if has_user || has_password || has_token {
                return Err(ApiError::ConflictingMechanisms);
            }
            return Ok(Credentials::ApiKey(key.clone()));
        }

        // A password and a token are two ways of proving the same thing, and
        // sending both says the client does not know which it wants.
        if has_password && has_token {
            return Err(ApiError::ConflictingMechanisms);
        }

        let Some(username) = self.u.clone() else {
            // Nothing at all was offered, so the missing piece is the user.
            return Err(ApiError::MissingParameter("u"));
        };

        if has_token {
            // Both halves are required for the mechanism to even be complete.
            if self.t.is_none() {
                return Err(ApiError::MissingParameter("t"));
            }
            if self.s.is_none() {
                return Err(ApiError::MissingParameter("s"));
            }
            return Ok(Credentials::SaltedToken);
        }

        let Some(password) = &self.p else {
            return Err(ApiError::MissingParameter("p"));
        };

        // A password we cannot decode is not a password.
        let password = auth::decode_password(password).ok_or(ApiError::WrongCredentials)?;

        Ok(Credentials::Password { username, password })
    }
}

/// An authenticated request, along with the format its response must take.
#[derive(Debug)]
pub struct Authenticated {
    pub user: User,
    pub format: Format,
}

impl FromRequestParts<SqlitePool> for Authenticated {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, pool: &SqlitePool) -> Result<Self, Failure> {
        // Parameters that fail to parse still get an answer in some format,
        // and XML is what the specification defaults to.
        let params = Query::<AuthParams>::from_request_parts(parts, pool)
            .await
            .map(|Query(params)| params)
            .map_err(|_| ApiError::MissingParameter("u").in_format(Format::Xml))?;

        let format = Format::from_param(params.f.as_deref());
        let credentials = params.credentials().map_err(|e| e.in_format(format))?;

        let outcome = match credentials {
            Credentials::ApiKey(key) => user::authenticate_api_key(pool, &key)
                .await
                .map(|user| user.ok_or(ApiError::InvalidApiKey)),
            Credentials::Password { username, password } => {
                user::authenticate_password(pool, &username, &password)
                    .await
                    .map(|user| user.ok_or(ApiError::WrongCredentials))
            }
            // Verifying md5(password + salt) requires the password back in
            // clear, and passwords here are Argon2id hashes. The mechanism is
            // not merely unimplemented, it is incompatible with storing
            // passwords properly, which is exactly what error 42 describes.
            Credentials::SaltedToken => Ok(Err(ApiError::MechanismNotSupported)),
        };

        match outcome {
            Ok(Ok(user)) => Ok(Self { user, format }),
            Ok(Err(e)) => Err(e.in_format(format)),
            Err(e) => {
                error!("authenticating request: {e:#}");
                Err(ApiError::Internal.in_format(format))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> AuthParams {
        AuthParams {
            f: None,
            u: None,
            p: None,
            t: None,
            s: None,
            api_key: None,
        }
    }

    /// Guards the parameter names themselves. The tests below build the struct
    /// directly, so they would happily pass while no request ever populated it.
    #[test]
    fn parameters_deserialize_under_the_names_the_api_uses() {
        let params: AuthParams =
            serde_json::from_str(r#"{"apiKey":"abc","f":"json","u":null}"#).unwrap();
        assert_eq!(params.api_key.as_deref(), Some("abc"));
        assert_eq!(params.f.as_deref(), Some("json"));
        assert_eq!(params.u, None);
    }

    #[test]
    fn an_api_key_on_its_own_is_accepted() {
        let p = AuthParams {
            api_key: Some("abc".into()),
            ..params()
        };
        assert_eq!(p.credentials(), Ok(Credentials::ApiKey("abc".into())));
    }

    #[test]
    fn an_api_key_with_a_username_conflicts() {
        let p = AuthParams {
            api_key: Some("abc".into()),
            u: Some("admin".into()),
            ..params()
        };
        assert_eq!(p.credentials(), Err(ApiError::ConflictingMechanisms));
    }

    #[test]
    fn an_api_key_with_a_password_conflicts() {
        let p = AuthParams {
            api_key: Some("abc".into()),
            p: Some("hunter2".into()),
            ..params()
        };
        assert_eq!(p.credentials(), Err(ApiError::ConflictingMechanisms));
    }

    #[test]
    fn an_api_key_with_a_token_conflicts() {
        let p = AuthParams {
            api_key: Some("abc".into()),
            t: Some("deadbeef".into()),
            s: Some("salt".into()),
            ..params()
        };
        assert_eq!(p.credentials(), Err(ApiError::ConflictingMechanisms));
    }

    #[test]
    fn a_password_and_a_token_together_conflict() {
        let p = AuthParams {
            u: Some("admin".into()),
            p: Some("hunter2".into()),
            t: Some("deadbeef".into()),
            s: Some("salt".into()),
            ..params()
        };
        assert_eq!(p.credentials(), Err(ApiError::ConflictingMechanisms));
    }

    #[test]
    fn a_username_and_password_are_accepted() {
        let p = AuthParams {
            u: Some("admin".into()),
            p: Some("hunter2".into()),
            ..params()
        };
        assert_eq!(
            p.credentials(),
            Ok(Credentials::Password {
                username: "admin".into(),
                password: "hunter2".into()
            })
        );
    }

    #[test]
    fn a_hex_encoded_password_is_decoded_before_use() {
        let p = AuthParams {
            u: Some("admin".into()),
            p: Some("enc:68756e74657232".into()),
            ..params()
        };
        assert_eq!(
            p.credentials(),
            Ok(Credentials::Password {
                username: "admin".into(),
                password: "hunter2".into()
            })
        );
    }

    #[test]
    fn a_salted_token_is_recognised_so_it_can_be_refused_clearly() {
        let p = AuthParams {
            u: Some("admin".into()),
            t: Some("deadbeef".into()),
            s: Some("salt".into()),
            ..params()
        };
        assert_eq!(p.credentials(), Ok(Credentials::SaltedToken));
    }

    #[test]
    fn half_a_token_is_a_missing_parameter() {
        let without_salt = AuthParams {
            u: Some("admin".into()),
            t: Some("deadbeef".into()),
            ..params()
        };
        assert_eq!(
            without_salt.credentials(),
            Err(ApiError::MissingParameter("s"))
        );

        let without_token = AuthParams {
            u: Some("admin".into()),
            s: Some("salt".into()),
            ..params()
        };
        assert_eq!(
            without_token.credentials(),
            Err(ApiError::MissingParameter("t"))
        );
    }

    #[test]
    fn an_empty_request_is_missing_the_username() {
        assert_eq!(params().credentials(), Err(ApiError::MissingParameter("u")));
    }

    #[test]
    fn a_username_with_nothing_else_is_missing_the_password() {
        let p = AuthParams {
            u: Some("admin".into()),
            ..params()
        };
        assert_eq!(p.credentials(), Err(ApiError::MissingParameter("p")));
    }
}
