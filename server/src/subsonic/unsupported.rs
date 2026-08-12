// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Endpoints Tocata does not implement, answered in the way that hurts a client
//! least.
//!
//! Registering them beats leaving them to a 404. A client meeting an HTTP 404
//! cannot tell a server that lacks the feature from one that is broken or behind
//! a misconfigured proxy, and some retry or refuse to continue. An answer in the
//! protocol's own shape says plainly that there is nothing here.
//!
//! Which answer depends on what was asked. A **listing** returns an empty one
//! with `status="ok"`, because a client that gets no videos simply draws no video
//! section, while one that gets an error may show the user a failure. Anything
//! naming a **particular thing** answers 70: that thing genuinely is not here.
//!
//! And an **action** answers 70 as well, which is the one place where the empty
//! `ok` would be worse than an error. A chat message handed to a server that keeps
//! none, and acknowledged, is somebody's words disappearing without a word back;
//! the listing beside it always comes back empty, so nothing would ever show. Told
//! plainly, the client says so and the words stay in the box.
//!
//! None of this is documented anywhere else. The specification defines
//! getOpenSubsonicExtensions for a client to discover what a server can do, so
//! documentation listing what it cannot would be a second list to keep in sync.

use super::auth::Authenticated;
use super::error::ApiError;
use super::response::{self, Empty};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
struct VideosBody {
    /// Present and empty: the element is what a client looks for, and its
    /// emptiness is the answer.
    videos: Empty,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatMessagesBody {
    chat_messages: Empty,
}

#[derive(Serialize)]
struct SharesBody {
    shares: Empty,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PodcastsBody {
    podcasts: Empty,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RadioBody {
    internet_radio_stations: Empty,
}

/// Video is out of scope, so there are no videos. An empty list rather than an
/// error, and no mention of it anywhere else.
pub async fn get_videos(auth: Authenticated) -> Response {
    response::ok(auth.format, VideosBody { videos: Empty {} })
}

pub async fn get_chat_messages(auth: Authenticated) -> Response {
    response::ok(
        auth.format,
        ChatMessagesBody {
            chat_messages: Empty {},
        },
    )
}

pub async fn get_shares(auth: Authenticated) -> Response {
    response::ok(auth.format, SharesBody { shares: Empty {} })
}

pub async fn get_podcasts(auth: Authenticated) -> Response {
    response::ok(auth.format, PodcastsBody { podcasts: Empty {} })
}

pub async fn get_internet_radio_stations(auth: Authenticated) -> Response {
    response::ok(
        auth.format,
        RadioBody {
            internet_radio_stations: Empty {},
        },
    )
}

/// For the calls that name one thing: that thing is not here.
///
/// 70 rather than 0, because the specification has no code for "not
/// implemented" and this is the closest true statement.
pub async fn not_found(auth: Authenticated) -> Response {
    ApiError::NotFound.in_format(auth.format).into_response()
}
