// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Following a finger, or a pointer, from where it went down to where it lets go.
//!
//! Three gestures in this panel are the same three events with different arithmetic
//! after them — reordering the queue, swiping a track out of it, and pushing the
//! player's sheet back down — so what they share is here and each of them keeps only
//! its own sums.
//!
//! **Pointer events and not touch events.** One set for a finger, a mouse and a pen,
//! so none of this is written twice and reordering a queue works on a laptop as well
//! as on a phone.
//!
//! **Capture is what makes it reliable.** Without it a gesture ends the moment the
//! pointer leaves the element it started on — which for a row being dragged up a list
//! is immediately. With it, every move and the release come back to where the gesture
//! began, however far away the finger has gone.
//!
//! **`touch-action` decides who gets the gesture, not JavaScript.** A row that can be
//! swiped sideways says `pan-y`, which hands the browser the vertical scroll and keeps
//! the horizontal for us; a handle that reorders says `none` and keeps both. Preventing
//! a default that the browser has already committed to is what makes a list feel like
//! it is fighting back.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// A gesture in progress: where it started, and how far it has come.
#[derive(Clone, Copy)]
pub struct Drag {
    /// How far from where it went down, in pixels. Positive is right and down.
    pub by: RwSignal<(f64, f64)>,
    /// Whether a gesture is happening at all. What the styles hang off, so a row
    /// knows to follow the finger rather than sit where the list put it.
    pub going: RwSignal<bool>,
    /// Where it went down, which is only ever subtracted from where it is now.
    at: RwSignal<(f64, f64)>,
}

impl Default for Drag {
    fn default() -> Self {
        Self::new()
    }
}

impl Drag {
    pub fn new() -> Self {
        Self {
            by: RwSignal::new((0.0, 0.0)),
            going: RwSignal::new(false),
            at: RwSignal::new((0.0, 0.0)),
        }
    }

    /// Takes the gesture, if it is one worth taking.
    ///
    /// Only the primary button, so a right click does not start dragging a row. And
    /// never one that went down on a control: the progress bar and the buttons inside
    /// the player's sheet have their own answer to being dragged, and a sheet that
    /// slid away while somebody moved the progress bar would be the sheet stealing it.
    pub fn begin(&self, event: &web_sys::PointerEvent) -> bool {
        if event.button() != 0 || on_a_control(event) {
            return false;
        }

        // Every move and the release come back here even once the pointer has left
        // this element, which for a row dragged up a list is straight away.
        if let Some(element) = current(event) {
            let _ = element.set_pointer_capture(event.pointer_id());
        }

        self.at
            .set((event.client_x() as f64, event.client_y() as f64));
        self.by.set((0.0, 0.0));
        self.going.set(true);
        true
    }

    /// Where the pointer has got to. Does nothing at all when no gesture is running,
    /// which is what makes it safe to bind to every move over the element.
    pub fn moved(&self, event: &web_sys::PointerEvent) {
        if !self.going.get_untracked() {
            return;
        }

        let (from_x, from_y) = self.at.get_untracked();
        self.by.set((
            event.client_x() as f64 - from_x,
            event.client_y() as f64 - from_y,
        ));
    }

    /// Ends it, and hands back how far it had come so the caller can decide what that
    /// meant. `None` when there was no gesture, so a stray release does nothing.
    ///
    /// The offset goes back to nought here rather than being left where the finger
    /// stopped, and that is what makes a gesture that fell short spring back instead
    /// of staying half done. Which is also why the distance is returned rather than
    /// read again afterwards: by then it is nought.
    pub fn end(&self) -> Option<(f64, f64)> {
        if !self.going.get_untracked() {
            return None;
        }

        let came = self.by.get_untracked();
        self.going.set(false);
        self.by.set((0.0, 0.0));
        Some(came)
    }

    /// How far it has come sideways, for a style to follow.
    pub fn across(&self) -> f64 {
        self.by.get().0
    }

    /// And downwards.
    pub fn down(&self) -> f64 {
        self.by.get().1
    }
}

/// The element the handler is bound to, which is what captures the pointer — not the
/// one under the finger, which for a row is whichever span it happened to land on.
fn current(event: &web_sys::PointerEvent) -> Option<web_sys::Element> {
    event
        .current_target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
}

/// Whether the gesture went down on something that has its own idea about being
/// dragged.
fn on_a_control(event: &web_sys::PointerEvent) -> bool {
    let Some(element) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    else {
        return false;
    };

    // `closest` rather than the tag of what was hit: a press lands on the span inside
    // a button as often as on the button.
    element
        .closest("button, input, select, a")
        .ok()
        .flatten()
        .is_some()
}
