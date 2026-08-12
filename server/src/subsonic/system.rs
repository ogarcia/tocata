// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use super::asked::Asked;
use super::auth::Authenticated;
use super::error::ApiError;
use super::response::{self, Empty, RequestFormat};
use crate::scanner::{self, Mode, Progress};
use crate::state::AppState;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

/// Extensions of OpenSubsonic this server implements.
///
/// This list is the canonical answer to "what does this server support": the
/// specification defines this endpoint so clients can find out by asking instead
/// of reading documentation. It must never claim something that does not actually
/// work.
///
/// And it must claim everything that does, which is the half that had gone
/// unnoticed. Structured lyrics with their timings have been answered here since
/// the lyrics went in, and no client ever asked for them — because a client reads
/// this list first and this list did not say so. Nothing failed, which is what
/// made it last: an extension implemented and not declared is an extension nobody
/// has.
const EXTENSIONS: &[Extension] = &[
    Extension {
        name: "apiKeyAuthentication",
        versions: &[1],
    },
    // Version 1 is `getLyricsBySongId` answering in `structuredLyrics`, which is
    // what this server does. Version 2 is timings by word and lyric layers, which
    // it does not, so it is not claimed.
    Extension {
        name: "songLyrics",
        versions: &[1],
    },
];

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
#[serde(rename_all = "camelCase")]
struct TokenInfoBody {
    token_info: TokenInfo,
}

#[derive(Serialize)]
struct TokenInfo {
    username: String,
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

/// Who a key belongs to.
///
/// Part of the same extension as the key itself, and it was the half that was
/// missing: this server has been telling clients it does `apiKeyAuthentication`
/// while answering nothing here.
///
/// A key that is not valid never reaches this. The extension asks for a 44 in that
/// case, and 44 is what the authentication answers before any of this runs — which
/// is the whole of what a client uses this call for: holding a key, finding out
/// whether it still works and whose it is.
///
/// Asked with a password instead of a key, it answers the same way rather than
/// refusing. The response has one field in it and it is the username, which is as
/// true of the one mechanism as of the other; a 44 would be saying a key is
/// invalid when there was no key in the request at all.
pub async fn token_info(auth: Authenticated) -> Response {
    response::ok(
        auth.format,
        TokenInfoBody {
            token_info: TokenInfo {
                username: auth.user.username,
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element names travel to clients, and getting one wrong breaks them
    /// silently. This has already happened twice in this project, both times in
    /// a field that no unit test looked at.
    #[test]
    fn response_bodies_use_the_names_the_api_defines() {
        let scan = serde_json::to_value(ScanStatusBody {
            scan_status: ScanStatus {
                scanning: true,
                count: 42,
            },
        })
        .unwrap();
        assert!(scan.get("scanStatus").is_some(), "got {scan}");
        assert_eq!(scan["scanStatus"]["scanning"], true);
        assert_eq!(scan["scanStatus"]["count"], 42);

        let extensions = serde_json::to_value(Extensions {
            open_subsonic_extensions: EXTENSIONS,
        })
        .unwrap();
        assert!(
            extensions.get("openSubsonicExtensions").is_some(),
            "got {extensions}"
        );

        let licensed = serde_json::to_value(Licensed {
            license: License { valid: true },
        })
        .unwrap();
        assert_eq!(licensed["license"]["valid"], true);

        let token = serde_json::to_value(TokenInfoBody {
            token_info: TokenInfo {
                username: "ana".to_string(),
            },
        })
        .unwrap();
        assert_eq!(token["tokenInfo"]["username"], "ana");
    }

    /// Everything this server can do beyond the base protocol is declared, and the
    /// list is the only way a client learns any of it.
    ///
    /// Which is why each one is named here with what makes it true. Structured
    /// lyrics were answered for months and never asked for, because the answer was
    /// there and the declaration was not — and nothing failed, which is what made
    /// it last. The count is asserted so that an extension added without a line in
    /// here stops the suite instead of going out undeclared or, worse, declared and
    /// unimplemented.
    #[test]
    fn every_extension_declared_is_one_this_server_keeps() {
        for (name, because) in [
            (
                "apiKeyAuthentication",
                "the auth layer accepts apiKey, and tokenInfo answers for a key",
            ),
            (
                "songLyrics",
                "getLyricsBySongId answers in structuredLyrics, timings and all",
            ),
        ] {
            assert!(
                EXTENSIONS.iter().any(|e| e.name == name),
                "{name} is not declared: {because}"
            );
        }

        assert_eq!(
            EXTENSIONS.len(),
            2,
            "an extension was declared without saying here what makes it true"
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanStatusBody {
    scan_status: ScanStatus,
}

/// Only what the specification defines: whether a scan is running and how much
/// it has got through. Other servers add fields here; inventing our own would
/// give clients nothing they could rely on.
#[derive(Serialize)]
struct ScanStatus {
    scanning: bool,
    count: u64,
}

impl ScanStatus {
    fn of(progress: &Progress) -> Self {
        Self {
            scanning: progress.is_scanning(),
            count: progress.counted(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartScanQuery {
    /// Not in the specification, but the way to ask for the reread that catches
    /// tags edited with their timestamps preserved.
    full_scan: Option<bool>,
}

pub async fn get_scan_status(auth: Authenticated, State(scan): State<Arc<Progress>>) -> Response {
    response::ok(
        auth.format,
        ScanStatusBody {
            scan_status: ScanStatus::of(&scan),
        },
    )
}

/// Administrative: this one goes and reads the whole library.
pub async fn start_scan(
    auth: Authenticated,
    State(state): State<AppState>,
    Asked(query): Asked<StartScanQuery>,
) -> Response {
    if !auth.user.is_admin {
        return ApiError::NotAuthorized
            .in_format(auth.format)
            .into_response();
    }

    let mode = if query.full_scan.unwrap_or(false) {
        Mode::Full
    } else {
        Mode::Incremental
    };

    // Answering before the scan finishes is the point: a client polls
    // getScanStatus to follow it.
    let spawned = state.clone();
    tokio::spawn(async move {
        match scanner::scan_all(&spawned.pool, mode, &spawned.scan).await {
            Ok(Some(outcome)) => info!(
                "scan finished: {} folders, {} tracks ({} unchanged), {} failed, {} gone",
                outcome.folders, outcome.tracks, outcome.unchanged, outcome.failed, outcome.gone
            ),
            Ok(None) => {}
            Err(e) => error!("scan failed: {e:#}"),
        }
    });

    response::ok(
        auth.format,
        ScanStatusBody {
            scan_status: ScanStatus::of(&state.scan),
        },
    )
}
