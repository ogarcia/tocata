// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use super::auth::Authenticated;
use super::response::{self, Empty};
use axum::response::Response;
use tracing::debug;

/// `ping` carries no payload, but it does authenticate: clients use it to
/// check that the credentials they were given actually work.
pub async fn ping(auth: Authenticated) -> Response {
    debug!("ping from '{}'", auth.user.username);
    response::ok(auth.format, Empty {})
}
