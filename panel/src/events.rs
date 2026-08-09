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
//!
//! One connection, however many kinds of news travel down it. The server names
//! each event and this listens for each name separately, so a screen reads the
//! signal it cares about and knows nothing about the rest.

use crate::api;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use serde::de::DeserializeOwned;
use tocata::types::{PortraitRun, Resources, Status};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

/// The name the server puts on a scan update.
const SCAN: &str = "scan";

/// And on the figures it takes every couple of seconds.
const RESOURCES: &str = "resources";

/// And on how the walk out for artist portraits is going. Nothing arrives under
/// this name while none is going, which is nearly always.
const PORTRAITS: &str = "portraits";

/// What the stream says, kept up to date.
///
/// The initial `None` in each means nothing has arrived yet, which a screen can
/// tell apart from a scan that is not running or a figure of zero.
#[derive(Clone, Copy)]
pub struct Live {
    pub scan: ReadSignal<Option<Status>>,
    pub resources: ReadSignal<Option<Resources>>,
    /// Only the half of it that moves. What a walk would find if it started —
    /// the setting, and how many artists are without a picture — is asked for
    /// rather than watched: it changes when a walk ends, not while it runs.
    pub portraits: ReadSignal<Option<PortraitRun>>,
}

/// Opens the stream and hands back what it says.
///
/// The stream is closed when whatever called this goes away. Without that a
/// screen left behind would keep a connection open and keep waking up to write
/// into signals nobody reads.
pub fn open() -> Live {
    let (scan, set_scan) = signal(None);
    let (resources, set_resources) = signal(None);
    let (portraits, set_portraits) = signal(None);

    let live = Live {
        scan,
        resources,
        portraits,
    };

    let Ok(source) = EventSource::new(api::EVENTS) else {
        // A URL we wrote ourselves does not fail to parse, and if it did there is
        // nothing a screen could do about it beyond showing no figures.
        return live;
    };

    let listeners = [
        listen::<Status>(&source, SCAN, set_scan),
        listen::<Resources>(&source, RESOURCES, set_resources),
        listen::<PortraitRun>(&source, PORTRAITS, set_portraits),
    ];

    // Kept alive until the cleanup, because a closure dropped while the browser
    // still holds a pointer to it is how a page starts panicking on the next
    // message. None of these is `Send` — they are handles into a JavaScript
    // runtime — and `on_cleanup` asks for one, so they travel wrapped. There is
    // one thread here, so the wrapper never has anything to refuse.
    let held = SendWrapper::new((source, listeners));

    on_cleanup(move || {
        let (source, listeners) = held.take();
        source.close();
        drop(listeners);
    });

    live
}

/// Feeds one named event into one signal, and hands back the closure to hold on
/// to.
fn listen<T: DeserializeOwned + Send + Sync + 'static>(
    source: &EventSource,
    name: &str,
    set: WriteSignal<Option<T>>,
) -> Closure<dyn Fn(MessageEvent)> {
    let listener = Closure::<dyn Fn(MessageEvent)>::new(move |message: MessageEvent| {
        let Some(text) = message.data().as_string() else {
            return;
        };
        // A payload we cannot read is one update skipped. The next one carries
        // the whole state again, so there is nothing to recover.
        if let Ok(update) = serde_json::from_str::<T>(&text) {
            set.set(Some(update));
        }
    });

    source
        .add_event_listener_with_callback(name, listener.as_ref().unchecked_ref())
        .ok();

    listener
}
