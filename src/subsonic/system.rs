// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use super::auth::Authenticated;
use super::response::{self, Empty, RequestFormat};
use axum::response::Response;
use serde::Serialize;
use tracing::debug;

/// Extensions of OpenSubsonic this server implements.
///
/// This list is the canonical answer to "what does this server support": the
/// specification defines this endpoint so clients can find out by asking
/// instead of reading documentation. It must never claim something that does
/// not actually work.
const EXTENSIONS: &[Extension] = &[Extension {
    name: "apiKeyAuthentication",
    versions: &[1],
}];

#[derive(Serialize)]
struct Extension {
    name: &'static str,
    versions: &'static [u8],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Extensions {
    open_subsonic_extensions: &'static [Extension],
}

#[derive(Serialize)]
struct Licensed {
    license: License,
}

/// Only `valid` is required. The other fields of the object exist for the
/// commercial Subsonic's expiry dates and trial periods, which mean nothing
/// here.
#[derive(Serialize)]
struct License {
    valid: bool,
}

/// `ping` carries no payload, but it does authenticate: clients use it to
/// check that the credentials they were given actually work.
pub async fn ping(auth: Authenticated) -> Response {
    debug!("ping from '{}'", auth.user.username);
    response::ok(auth.format, Empty {})
}

pub async fn get_license(auth: Authenticated) -> Response {
    response::ok(
        auth.format,
        Licensed {
            license: License { valid: true },
        },
    )
}

/// The one endpoint that must answer without credentials. A client has to know
/// whether api key authentication exists here before it can choose how to
/// authenticate, so requiring authentication to find out would be circular.
pub async fn get_open_subsonic_extensions(RequestFormat(format): RequestFormat) -> Response {
    response::ok(
        format,
        Extensions {
            open_subsonic_extensions: EXTENSIONS,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_key_extension_is_advertised() {
        assert!(
            EXTENSIONS.iter().any(|e| e.name == "apiKeyAuthentication"),
            "the auth layer accepts apiKey, so clients must be told"
        );
    }

    #[test]
    fn every_advertised_extension_declares_a_version() {
        for extension in EXTENSIONS {
            assert!(
                !extension.versions.is_empty(),
                "{} claims no versions",
                extension.name
            );
        }
    }
}
