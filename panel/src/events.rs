// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The one stream the panel keeps open.
//!
//! `EventSource` rather than a socket or a timer. It reconnects on its own after
//! a dropped connection or a restarted server, which is the whole reason the
//! server chose to send events this way, and it carries the session cookie
//! because it is a same origin request like any other.
//!
//! Every message holds the whole state rather than a change to it, so a missed
//! one costs nothing and there is no sequence to keep in order.

use crate::api;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use tocata::types::Status;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

/// The name the server puts on a scan update.
const SCAN: &str = "scan";

/// Opens the stream and hands back what it says, kept up to date.
///
/// The initial `None` means nothing has arrived yet, which a screen can tell
/// apart from a scan that is not running.
///
/// The stream is closed when whatever called this goes away. Without that a
/// screen left behind would keep a connection open and keep waking up to write
/// into signals nobody reads.
pub fn scan_status() -> ReadSignal<Option<Status>> {
    let (status, set_status) = signal(None);

    let Ok(source) = EventSource::new(api::EVENTS) else {
        // A URL we wrote ourselves does not fail to parse, and if it did there is
        // nothing a screen could do about it beyond showing no progress.
        return status;
    };

    let listener = Closure::<dyn Fn(MessageEvent)>::new(move |message: MessageEvent| {
        let Some(text) = message.data().as_string() else {
            return;
        };
        // A payload we cannot read is one update skipped. The next one carries
        // the whole state again, so there is nothing to recover.
        if let Ok(update) = serde_json::from_str::<Status>(&text) {
            set_status.set(Some(update));
        }
    });

    source
        .add_event_listener_with_callback(SCAN, listener.as_ref().unchecked_ref())
        .ok();

    // Kept alive until the cleanup, because a closure dropped while the browser
    // still holds a pointer to it is how a page starts panicking on the next
    // message. Neither of these is `Send` — they are handles into a JavaScript
    // runtime — and `on_cleanup` asks for one, so they travel wrapped. There is
    // one thread here, so the wrapper never has anything to refuse.
    let held = SendWrapper::new((source, listener));

    on_cleanup(move || {
        let (source, listener) = held.take();
        source.close();
        drop(listener);
    });

    status
}
