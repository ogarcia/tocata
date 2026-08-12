// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Reading what a call was asked with, and complaining about it in the protocol.
//!
//! Every endpoint here took its parameters with axum's own `Query`, whose refusal
//! is an HTTP 400 with a line of English in the body. That is the one thing this
//! API never does: a client asking `getAlbum` with no `id` got a status it has no
//! way to read, from a server that answers 200 and puts the trouble in the body
//! for everything else — including for the same request with a bad password.
//!
//! Which mattered most where it was least visible. A client that meets a 400 has
//! no code, no message and nothing to show a user; several treat it as the server
//! being broken or unreachable and stop asking. Twelve calls answered that way.
//!
//! So: `Asked<T>` where it said `Query<T>`, and a parameter that is missing or
//! unreadable comes back as error 10 in the envelope the client already knows how
//! to read, in the format it asked for.

use super::error::ApiError;
use super::response::{Format, RequestFormat};
use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

/// What a call was asked with.
///
/// Stands where `Query<T>` stood and takes the same types: only the refusal is
/// different, which is the whole point of it.
#[derive(Debug)]
pub struct Asked<T>(pub T);

impl<T, S> FromRequestParts<S> for Asked<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    /// The answer itself rather than an error type: what comes back is already a
    /// finished response in the shape and format the client asked for.
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        // What was asked returns from here and nothing else does, which is not a
        // matter of taste: what a query parses into need not be `Send`, and this
        // future must be. Held across the await below, the parsed value — or the
        // refusal carrying it — makes the whole extractor unusable.
        let refusal = match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(asked)) => return Ok(Self(asked)),
            Err(refused) => refused.body_text(),
        };

        // The format is read from the same query string that just failed to parse,
        // which is safe because that reading cannot fail: a query nobody can make
        // sense of simply did not ask for JSON, and XML is what the specification
        // defaults to.
        let RequestFormat(format) = RequestFormat::from_request_parts(parts, state)
            .await
            .unwrap_or(RequestFormat(Format::Xml));

        Err(ApiError::UnreadableParameter(said(&refusal))
            .in_format(format)
            .into_response())
    }
}

/// Turns what the deserialiser said into what a client is told.
///
/// A missing parameter is named, because the name is the whole of the answer and
/// serde puts it in backticks. Anything else keeps the detail — a number that is
/// not a number, most of it — since this is a client being told about its own
/// request, and "one of the parameters is wrong" would send somebody reading the
/// specification for an afternoon.
fn said(refusal: &str) -> String {
    match named(refusal, "missing field ") {
        Some(name) => format!("Required parameter {name} is missing"),
        // The deserialiser names the field for a missing one and not for a value
        // it could not read, so there is nothing to name here.
        None => format!(
            "A parameter of this call cannot be read: {}",
            refusal.rsplit(": ").next().unwrap_or(refusal)
        ),
    }
}

/// The name in backticks after a phrase, if the refusal is of that shape.
fn named(refusal: &str, after: &str) -> Option<String> {
    let rest = refusal.split_once(after)?.1;
    let quoted = rest.split_once('`')?.1;

    quoted.split_once('`').map(|(name, _)| name.to_string())
}

/// Query parameters that may repeat.
///
/// `axum::extract::Query` goes through serde_urlencoded, which keeps only one
/// value per name. The API repeats names instead of using an array syntax —
/// `id=1&id=2&id=3` is how a client stars three songs at once — so those endpoints
/// need a reader that collects them.
///
/// Here rather than beside the answering, and refusing exactly as [`Asked`] does.
/// It used to name `id` whatever had been left out, and to answer in XML on the
/// grounds that a request this broken had not said what format it wanted — which
/// is not so: `getPlaylist?f=json` with no id had said, and got XML anyway.
#[derive(Debug)]
pub struct Repeated<T>(pub T);

impl<T, S> FromRequestParts<S> for Repeated<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        let refusal = match serde_html_form::from_str(parts.uri.query().unwrap_or_default()) {
            Ok(asked) => return Ok(Self(asked)),
            Err(refused) => refused.to_string(),
        };

        let RequestFormat(format) = RequestFormat::from_request_parts(parts, state)
            .await
            .unwrap_or(RequestFormat(Format::Xml));

        Err(ApiError::UnreadableParameter(said(&refusal))
            .in_format(format)
            .into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentence a client gets, from the sentence axum gives us. Pinned because
    /// it is read out of somebody else's wording: if a version of axum or serde
    /// changes how it says this, the parameter stops being named and this test is
    /// what says so.
    #[test]
    fn a_missing_parameter_is_named() {
        assert_eq!(
            said("Failed to deserialize query string: missing field `id`"),
            "Required parameter id is missing"
        );
    }

    #[test]
    fn a_value_that_will_not_read_keeps_its_reason() {
        assert_eq!(
            said("Failed to deserialize query string: invalid digit found in string"),
            "A parameter of this call cannot be read: invalid digit found in string"
        );
    }

    /// Nothing recognisable in it still says something true.
    #[test]
    fn anything_else_is_still_answered() {
        assert_eq!(
            said("nonsense"),
            "A parameter of this call cannot be read: nonsense"
        );
    }
}
