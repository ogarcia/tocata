// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use super::CommonParams;
use super::response::{self, Empty, Format};
use axum::extract::Query;
use axum::response::Response;

pub async fn ping(Query(params): Query<CommonParams>) -> Response {
    response::ok(Format::from_param(params.f.as_deref()), Empty {})
}
