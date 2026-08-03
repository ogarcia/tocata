// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What is sounding, and the queue it came out of.
//!
//! One of these exists for the whole panel, handed round in a context, because two
//! would be two things playing at once. The screens put music into it and the block
//! at the foot of the sidebar draws it; neither knows about the other.
//!
//! **The queue is identifiers.** That is what lets "play what you are looking at"
//! mean everything the filter matched rather than the fifty rows that happened to be
//! fetched — thousands of them cost nothing as strings. The price is that stepping
//! onto a track is the moment we learn what it is called, so each step asks the
//! server for that one track. One request per song is nothing next to the song.
//!
//! **The audio element is the clock.** Nothing here counts time: `timeupdate` says
//! where the browser has got to and the figures follow it. A player that kept its own
//! position would be a second opinion about the one thing the browser knows for
//! certain, and the two would drift apart over an hour of buffering and seeking.

use crate::api;
use leptos::prelude::*;
use leptos::task::spawn_local;
use tocata::types::Track;

/// Far enough into a track to count as having heard it.
///
/// Half of it, which is what every scrobbler settles on: enough that skipping
/// through a record does not count as listening to it, little enough that leaving
/// before the outro does not mean it never happened.
const HEARD: f64 = 0.5;

/// What is sounding, and what is queued behind it.
#[derive(Clone, Copy)]
pub struct Player {
    /// Everything to be played, as identifiers, in the order it will be played.
    pub queue: RwSignal<Vec<String>>,
    /// Where in that queue we are. Meaningless when the queue is empty.
    pub at: RwSignal<usize>,
    /// What the current track turns out to be, once the server has said. `None`
    /// while that answer is on its way, which is when the sidebar has an identifier
    /// and nothing to write.
    pub now: RwSignal<Option<Track>>,
    /// Whether it is sounding, as against paused. Set from the audio element's own
    /// events rather than by whoever pressed the button: the browser can refuse to
    /// start, and then this said it was playing and nothing was.
    pub playing: RwSignal<bool>,
    /// Seconds into the current track, and how many it holds. Both come from the
    /// element; the second overrides the length the listing reported, since the file
    /// is the thing being played.
    pub elapsed: RwSignal<f64>,
    pub duration: RwSignal<f64>,

    /// Whether this track has already been counted, so holding still at the halfway
    /// mark does not count it twice.
    counted: RwSignal<bool>,
}

impl Player {
    pub fn new() -> Self {
        Self {
            queue: RwSignal::new(Vec::new()),
            at: RwSignal::new(0),
            now: RwSignal::new(None),
            playing: RwSignal::new(false),
            elapsed: RwSignal::new(0.0),
            duration: RwSignal::new(0.0),
            counted: RwSignal::new(false),
        }
    }

    /// Whether there is anything to draw. The block in the sidebar is not there at
    /// all until something has been played — an idle player is furniture.
    pub fn loaded(&self) -> bool {
        !self.queue.with(Vec::is_empty)
    }

    /// The identifier of what should be sounding, if anything.
    pub fn current(&self) -> Option<String> {
        let at = self.at.get();
        self.queue.with(|queue| queue.get(at).cloned())
    }

    /// Takes a queue and starts at the top of it.
    ///
    /// `from` is where in that queue to begin, which is how pressing play on the
    /// fourth row plays the fourth row and keeps the three above it behind — going
    /// back is part of what a queue is for.
    pub fn play(&self, queue: Vec<String>, from: usize) {
        if queue.is_empty() {
            return;
        }

        self.queue.set(queue);
        self.step_to(from);
    }

    /// Pauses if it is sounding and starts it again if it is not. Does nothing at
    /// all with an empty queue, which is what the sidebar not being there means.
    pub fn toggle(&self) {
        if self.loaded() {
            self.playing.update(|on| *on = !*on);
        }
    }

    /// The next track, or a stop at the end.
    ///
    /// The end of a queue leaves the last track loaded and paused rather than
    /// clearing everything: somebody who has just heard a record is quite likely to
    /// want it again, and an empty sidebar would have thrown that away.
    pub fn next(&self) {
        let at = self.at.get_untracked();
        let last = self.queue.with_untracked(Vec::len).saturating_sub(1);

        if at < last {
            self.step_to(at + 1);
        } else {
            self.playing.set(false);
        }
    }

    /// The previous track, or the start of this one.
    ///
    /// Which is what the button does everywhere: from the middle of a song it goes
    /// back to its beginning, and only from the first few seconds does it reach the
    /// song before.
    pub fn previous(&self) {
        let at = self.at.get_untracked();

        if self.elapsed.get_untracked() > 3.0 || at == 0 {
            self.seek_to(0.0);
            self.replay();
        } else {
            self.step_to(at - 1);
        }
    }

    /// Moves to a position in the queue and asks what is there.
    fn step_to(&self, at: usize) {
        let player = *self;

        player.at.set(at);
        player.now.set(None);
        player.elapsed.set(0.0);
        player.duration.set(0.0);
        player.counted.set(false);
        player.playing.set(true);

        let Some(id) = player.current() else { return };

        spawn_local(async move {
            // A title is not worth interrupting the music for. If this fails the
            // sidebar stays quiet about what it is playing and goes on playing it.
            if let Ok(track) = api::track(&id).await {
                // Unless the queue has moved on while the answer was in flight, in
                // which case this is the name of the song before.
                if player.current().as_deref() == Some(track.id.as_str()) {
                    player
                        .duration
                        .set(track.duration.unwrap_or_default() as f64);
                    player.now.set(Some(track));
                }
            }
        });
    }

    /// Starts the current track again from the top, without asking for it afresh.
    fn replay(&self) {
        self.counted.set(false);
        self.playing.set(true);
    }

    /// Asks to be somewhere else in the current track.
    ///
    /// Written into `elapsed` rather than into a request of its own, because that is
    /// what `elapsed` is: where we are. The element watches it and, on finding itself
    /// somewhere else, moves — so a drag along the bar and the end of `previous`
    /// travel the same road, and the figures follow before the audio has caught up
    /// rather than lagging behind the thumb.
    pub fn seek_to(&self, seconds: f64) {
        let whole = self.duration.get_untracked();
        self.elapsed.set(seconds.clamp(0.0, whole.max(0.0)));
    }

    /// How many are still to come after the one sounding.
    pub fn ahead(&self) -> usize {
        self.queue.with(Vec::len).saturating_sub(self.at.get() + 1)
    }

    /// Takes a track out of what is coming, by where it sits in the queue.
    ///
    /// Only ever something ahead of us: removing what is sounding would be a stop
    /// dressed up as a tidy, and removing what is behind changes nothing anybody can
    /// hear. `at` therefore needs no adjusting, which is the whole reason for that
    /// rule.
    pub fn drop_at(&self, index: usize) {
        if index > self.at.get_untracked() {
            self.queue.update(|queue| {
                if index < queue.len() {
                    queue.remove(index);
                }
            });
        }
    }

    /// Moves a track in what is coming from one place to another.
    ///
    /// Both ends have to be ahead of what is sounding, for the same reason as
    /// dropping one: nothing here may disturb the track being played or where we are
    /// in the queue, so `at` never needs adjusting.
    pub fn move_in_queue(&self, from: usize, to: usize) {
        let now = self.at.get_untracked();

        if from <= now || to <= now || from == to {
            return;
        }

        self.queue.update(|queue| {
            if from < queue.len() && to < queue.len() {
                let moved = queue.remove(from);
                queue.insert(to, moved);
            }
        });
    }

    /// How far through, from nought to one. What the bar is filled with, and the one
    /// place that guards against the length being zero — which is what a track
    /// reports until its metadata has arrived.
    pub fn share(&self) -> f64 {
        let (elapsed, whole) = (self.elapsed.get(), self.duration.get());

        if whole > 0.0 {
            (elapsed / whole).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Where the browser has got to, which is the only clock this trusts.
    ///
    /// Also where a play gets counted, because this is the one place that knows how
    /// far in we are. It goes to the server once per track: `counted` is what stops
    /// a track held at its midpoint from being counted on every tick.
    pub fn ticked(&self, elapsed: f64, duration: f64) {
        self.elapsed.set(elapsed);

        // The file's own length, which can differ from what was scanned. Guarded
        // because an element with nothing loaded reports NaN.
        if duration.is_finite() && duration > 0.0 {
            self.duration.set(duration);

            if !self.counted.get_untracked() && elapsed / duration >= HEARD {
                self.counted.set(true);

                if let Some(id) = self.current() {
                    spawn_local(async move {
                        // Nothing to tell anybody if this fails. A play that was not
                        // counted is a figure that is one lower than it should be,
                        // and interrupting somebody's listening to say so would be
                        // worse than the figure.
                        let _ = api::count_play(&id).await;
                    });
                }
            }
        }
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

/// The one player, from wherever it is wanted.
///
/// Panics if it is not there, which it always is: it is provided above the router,
/// so anything drawn inside the panel has it. A screen that could not find it would
/// be a screen mounted outside the panel, which is a mistake in wiring rather than
/// something to handle.
pub fn player() -> Player {
    use_context::<Player>().expect("the player is provided above every screen")
}
