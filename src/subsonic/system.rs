// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use super::auth::Authenticated;
use super::error::ApiError;
use super::response::{self, Empty, RequestFormat};
use crate::scanner::{self, Mode, Progress};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

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
    }

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
    Query(query): Query<StartScanQuery>,
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
