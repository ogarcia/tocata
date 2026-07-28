// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the server is costing the machine it runs on.

use super::error::ApiError;
use super::session::Panel;
use crate::resources::Meter;
use crate::types::{ErrorBody, Resources};
use axum::Json;
use axum::extract::State;
use std::sync::Arc;

/// What the server is using
///
/// The share of the machine's processors this process is using and the memory it
/// is holding, as of now.
///
/// The share is worked out from the time since these figures were last asked for
/// by anybody, because processor time is a counter and a rate needs two readings
/// of it. Asking on a timer therefore gives the share over that timer's interval;
/// asking once gives the average since the last time anyone did.
///
/// The same figures arrive on the event stream as `resources` while a panel is
/// open, which is where a meter that redraws itself should read them from. This is
/// for the first reading and for asking without keeping a stream open.
#[utoipa::path(
    get,
    path = "/resources",
    tag = "resources",
    responses(
        (status = 200, description = "What the process is using", body = Resources),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn read(
    _panel: Panel,
    State(meter): State<Arc<Meter>>,
) -> Result<Json<Resources>, ApiError> {
    meter
        .read()
        .map(Json)
        .map_err(|e| ApiError::internal(e, "reading what this process is using"))
}
