// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The envelope every endpoint of the API answers with.

use super::xml;
use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tracing::error;

/// Version of the API this server implements.
pub const API_VERSION: &str = "1.16.1";

/// Identifies the implementation, as OpenSubsonic requires.
pub const SERVER_TYPE: &str = "tocata";

const ROOT_ELEMENT: &str = "subsonic-response";
const JSON_KEY: &str = "subsonic-response";

/// A body with no content of its own, for endpoints that answer with nothing
/// but the envelope. Braces matter: a unit struct would serialize as null and
/// could not be flattened.
#[derive(Serialize)]
pub struct Empty {}

/// What the `f` parameter asked for. Absent means XML, which is the default
/// the specification defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Xml,
}

impl Format {
    pub fn from_param(f: Option<&str>) -> Self {
        match f {
            Some(value) if value.eq_ignore_ascii_case("json") => Self::Json,
            _ => Self::Xml,
        }
    }
}

/// The requested format on its own, for the endpoints that answer without
/// authenticating. Extracting this cannot fail: an unreadable query simply
/// means nobody asked for JSON.
#[derive(Debug)]
pub struct RequestFormat(pub Format);

#[derive(Deserialize)]
struct FormatParam {
    f: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for RequestFormat {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Infallible> {
        let format = Query::<FormatParam>::from_request_parts(parts, state)
            .await
            .map(|Query(params)| Format::from_param(params.f.as_deref()))
            .unwrap_or(Format::Xml);

        Ok(Self(format))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<T> {
    status: &'static str,
    version: &'static str,
    #[serde(rename = "type")]
    server_type: &'static str,
    server_version: &'static str,
    open_subsonic: bool,
    #[serde(flatten)]
    body: T,
}

impl<T> Envelope<T> {
    fn new(status: &'static str, body: T) -> Self {
        Self {
            status,
            version: API_VERSION,
            server_type: SERVER_TYPE,
            server_version: env!("CARGO_PKG_VERSION"),
            open_subsonic: true,
            body,
        }
    }

    fn ok(body: T) -> Self {
        Self::new("ok", body)
    }

    fn failed(body: T) -> Self {
        Self::new("failed", body)
    }
}

/// Renders a successful response in the requested format.
///
/// The API answers 200 even for its own errors, so a failure to build the
/// response is the only thing that can produce a different status here.
pub fn ok<T: Serialize>(format: Format, body: T) -> Response {
    render(format, Envelope::ok(body))
}

/// Renders a failed response. Still HTTP 200: in this protocol the error
/// lives in the payload, not in the status line.
pub fn failed<T: Serialize>(format: Format, body: T) -> Response {
    render(format, Envelope::failed(body))
}

fn render<T: Serialize>(format: Format, envelope: Envelope<T>) -> Response {
    let value = match serde_json::to_value(&envelope) {
        Ok(value) => value,
        Err(e) => {
            error!("serializing response: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    match format {
        Format::Json => {
            let payload = serde_json::json!({ JSON_KEY: value });
            match serde_json::to_string(&payload) {
                Ok(body) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
                Err(e) => {
                    error!("serializing JSON response: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Format::Xml => match xml::render(ROOT_ELEMENT, &value) {
            Ok(body) => ([(header::CONTENT_TYPE, "application/xml")], body).into_response(),
            Err(e) => {
                error!("serializing XML response: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
    }
}

/// Query parameters that may repeat.
///
/// `axum::extract::Query` goes through serde_urlencoded, which keeps only one
/// value per name. The API repeats names instead of using an array syntax —
/// `id=1&id=2&id=3` is how a client stars three songs at once — so those
/// endpoints need a reader that collects them.
#[derive(Debug)]
pub struct Repeated<T>(pub T);

impl<T, S> FromRequestParts<S> for Repeated<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = BadParams;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, BadParams> {
        let query = parts.uri.query().unwrap_or_default();

        serde_html_form::from_str(query)
            .map(Repeated)
            .map_err(|e| BadParams(e.to_string()))
    }
}

/// A rejection carrying why the parameters could not be read. Rendered as the
/// protocol's own missing-parameter error, in XML, since a request this broken
/// has not told us what format it wanted either.
#[derive(Debug)]
pub struct BadParams(String);

impl IntoResponse for BadParams {
    fn into_response(self) -> Response {
        error!("reading repeated query parameters: {}", self.0);
        super::error::ApiError::MissingParameter("id")
            .in_format(Format::Xml)
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_defaults_to_xml() {
        assert_eq!(Format::from_param(None), Format::Xml);
        assert_eq!(Format::from_param(Some("xml")), Format::Xml);
        // jsonp is not supported, so it falls back to the default.
        assert_eq!(Format::from_param(Some("jsonp")), Format::Xml);
    }

    #[test]
    fn format_accepts_json_in_any_case() {
        assert_eq!(Format::from_param(Some("json")), Format::Json);
        assert_eq!(Format::from_param(Some("JSON")), Format::Json);
    }

    #[test]
    fn the_envelope_carries_the_opensubsonic_fields() {
        let value = serde_json::to_value(Envelope::ok(Empty {})).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["version"], API_VERSION);
        assert_eq!(value["type"], SERVER_TYPE);
        assert_eq!(value["serverVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(value["openSubsonic"], true);
    }

    #[test]
    fn a_flattened_body_sits_beside_the_envelope_fields() {
        #[derive(Serialize)]
        struct Body {
            license: License,
        }
        #[derive(Serialize)]
        struct License {
            valid: bool,
        }

        let value = serde_json::to_value(Envelope::ok(Body {
            license: License { valid: true },
        }))
        .unwrap();

        assert_eq!(value["status"], "ok");
        assert_eq!(value["license"]["valid"], true);
    }
}
