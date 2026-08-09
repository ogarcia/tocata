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

    /// The queue as it stood before it was mixed, and nothing at all while it is in
    /// the order it was given.
    ///
    /// One field doing both halves of the shuffle: it is the way back, and its being
    /// there is what the lit button means. Nothing about the mix itself is kept —
    /// mixing, ordering and mixing again gives a second mix with no relation to the
    /// first, which is what a shuffle is.
    ordered: RwSignal<Option<Vec<String>>>,
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
            ordered: RwSignal::new(None),
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

    /// What comes after it, which is what the second audio element loads while the
    /// first one is still playing.
    ///
    /// Nothing at the end of the queue: there is nothing to hand over to, and the
    /// element that would have held it stays empty rather than holding the track
    /// that is already sounding.
    pub fn upcoming(&self) -> Option<String> {
        let next = self.at.get() + 1;
        self.queue.with(|queue| queue.get(next).cloned())
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

        // A new queue is a new order, so there is nothing to go back to any more and
        // the button is not lit over the record that has just been put on.
        self.ordered.set(None);
        self.queue.set(queue);
        self.step_to(from);
    }

    /// Puts a track at the end of what is coming.
    ///
    /// Nothing else moves: not `at`, not what is sounding, not a track already
    /// waiting. Adding to a queue is the one way of putting music on that promises
    /// not to interrupt the music, which is the whole of why it exists beside
    /// `play`.
    pub fn queue_up(&self, id: String) {
        // While the queue is mixed there are two orders to keep. A track added to
        // only the mixed one would vanish the moment the shuffle was put away:
        // ordering walks the order it was given and keeps nothing that is not named
        // in it.
        self.ordered.update(|ordered| {
            if let Some(ordered) = ordered {
                ordered.push(id.clone());
            }
        });

        self.queue.update(|queue| queue.push(id));
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

        // That a song has started, which is a different claim from having heard it and
        // goes out at a different moment: this one now, and the play once it is mostly
        // over. It puts the panel in what is playing now beside every other client,
        // and tells whoever this account scrobbles to.
        //
        // Nothing is done with the answer. A song that starts is a fact about this
        // browser, not a request that can be refused, and the music does not wait on
        // it.
        {
            let id = id.clone();
            spawn_local(async move {
                let _ = api::announce_play(&id).await;
            });
        }

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

    /// Whether what is coming has been mixed, which is what lights the button.
    pub fn shuffled(&self) -> bool {
        self.ordered.with(Option::is_some)
    }

    /// Mixes what is coming, or puts it back the way it came.
    ///
    /// Both halves leave what is sounding and everything behind it exactly where they
    /// are — the same rule that governs dropping a track and moving one, and for the
    /// same reason: `at` never needs adjusting and the music never jumps.
    pub fn toggle_shuffle(&self) {
        let at = self.at.get_untracked();

        match self.ordered.get_untracked() {
            Some(ordered) => {
                self.queue
                    .update(|queue| *queue = ordered_again(&ordered, queue, at));
                self.ordered.set(None);
            }
            None => {
                self.ordered.set(Some(self.queue.get_untracked()));
                self.queue.update(|queue| mix(queue, at + 1, roll));
            }
        }
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
    /// The track has to be one ahead of what is sounding, for the same reason as
    /// dropping one: nothing here may disturb the track being played or where we are
    /// in the queue, so `at` never needs adjusting.
    ///
    /// **Where it lands is brought inside the queue rather than refused.** Dragging a
    /// row past the end of the list means putting it last, and past the start means
    /// putting it first — those are answers, not mistakes. Refusing them is what made
    /// the two ends behave differently: the near end was already clamped, so dragging
    /// to the top worked, while a drop past the bottom fell outside the list, was
    /// discarded, and sprang back as though nothing had been asked.
    pub fn move_in_queue(&self, from: usize, to: usize) {
        let now = self.at.get_untracked();

        if from <= now {
            return;
        }

        self.queue.update(|queue| {
            if let Some(to) = settled(from, to, now, queue.len()) {
                let moved = queue.remove(from);
                queue.insert(to, moved);
            }
        });
    }

    /// Where a track would land if it were dropped there, without moving anything.
    ///
    /// What the gap follows while a row is being held. It answers through the same
    /// function the move itself goes through, so the space that opens up is where the
    /// row will actually go and not a second guess at it.
    pub fn would_land(&self, from: usize, to: usize) -> Option<usize> {
        settled(from, to, self.at.get(), self.queue.with(Vec::len))
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

/// Where a track being moved actually ends up, or `None` when there is nothing to do.
///
/// Pulled out of the queue itself because it is the whole of the thinking and none of
/// the state — and because it is where the two ends of the list stopped agreeing. A
/// drop past the bottom used to fall outside the list and be discarded, so the row
/// sprang back as though nothing had been asked, while the top end was clamped and
/// worked. Both ends clamp now, and this says so in a form a test can ask about.
fn settled(from: usize, to: usize, now: usize, len: usize) -> Option<usize> {
    // Never the track sounding or anything behind it, and never a queue with nothing
    // waiting in it but that one.
    if from <= now || from >= len || len < now + 2 {
        return None;
    }

    let to = to.clamp(now + 1, len - 1);

    (to != from).then_some(to)
}

/// Mixes everything from `from` onwards and leaves what is before it alone.
///
/// Fisher and Yates: walk the tail backwards and swap each track with one drawn from
/// the part not yet walked. Every ordering comes out as often as every other, which
/// the obvious "sort by a random number" does not manage and the equally obvious
/// "swap each with any" does not either.
///
/// Where the numbers come from is handed in. It is the one thing here that is not
/// about order, and a test cannot ask what a mix will be unless it can say.
fn mix(queue: &mut [String], from: usize, mut roll: impl FnMut(usize) -> usize) {
    for nth in (from + 1..queue.len()).rev() {
        queue.swap(nth, from + roll(nth - from + 1));
    }
}

/// A number under `n`, from the browser's own randomness.
///
/// Clamped because the multiplication is in floating point: `random` promises to stay
/// under one, and one rounding away from that promise would be an index past the end
/// of the queue.
fn roll(n: usize) -> usize {
    ((js_sys::Math::random() * n as f64) as usize).min(n.saturating_sub(1))
}

/// The queue back in the order it was given.
///
/// What is sounding and everything played stay exactly where the mix left them —
/// putting a shuffle away is a claim about what is *coming*, and rewriting the last
/// half hour of listening would be a different, stranger thing to do.
///
/// The old order is filtered against what is actually waiting rather than taken
/// whole, which is what stops a track dropped out of the mix, or already heard in it,
/// from walking back in with the order. Everything waiting is in there somewhere: a
/// queue is only ever set whole, and setting one forgets the order it replaced.
fn ordered_again(ordered: &[String], queue: &[String], at: usize) -> Vec<String> {
    let mut waiting: Vec<&String> = queue.iter().skip(at + 1).collect();
    let mut back: Vec<String> = queue.iter().take(at + 1).cloned().collect();

    for id in ordered {
        // By position rather than by containment, so the same track queued twice
        // comes back twice and not four times.
        if let Some(nth) = waiting.iter().position(|waits| *waits == id) {
            waiting.remove(nth);
            back.push(id.clone());
        }
    }

    back
}

#[cfg(test)]
mod tests {
    use super::{mix, ordered_again, settled};

    /// A queue of single letters, which is all either function looks at.
    fn queue(letters: &str) -> Vec<String> {
        letters.chars().map(|one| one.to_string()).collect()
    }

    /// What a queue reads as, so a test can say the whole answer in one word.
    fn spelt(queue: &[String]) -> String {
        queue.concat()
    }

    /// Always the first of what is left to draw from, which is a mix a test can write
    /// down and one that moves every waiting track — a roll that drew the last would
    /// swap each track with itself and prove nothing.
    fn first(_: usize) -> usize {
        0
    }

    /// A track dropped past either end of the queue goes to that end.
    ///
    /// Which is the asymmetry this function exists for: the far end was refused rather
    /// than clamped, so dragging a row to the bottom of the list quietly did nothing
    /// while dragging it to the top worked.
    #[test]
    fn a_drop_past_either_end_lands_on_that_end() {
        // Five queued, the first sounding, so four are waiting.
        assert_eq!(settled(1, 99, 0, 5), Some(4), "past the bottom is last");
        assert_eq!(settled(4, 0, 0, 5), Some(1), "past the top is first");
        assert_eq!(settled(3, 1, 0, 5), Some(1), "and in between is itself");
    }

    /// Nothing may disturb what is sounding, or anything already played.
    #[test]
    fn nothing_moves_what_is_sounding_or_what_is_behind_it() {
        // The third of five is playing, so only the fourth and fifth may move.
        assert_eq!(settled(2, 4, 2, 5), None, "the one sounding stays put");
        assert_eq!(settled(1, 4, 2, 5), None, "and so does one already played");
        assert_eq!(
            settled(4, 0, 2, 5),
            Some(3),
            "a waiting one goes no further back"
        );
    }

    /// A move that changes nothing is not a move.
    #[test]
    fn a_move_that_lands_where_it_started_is_no_move() {
        assert_eq!(settled(2, 2, 0, 5), None);
        // Clamped onto its own position, which is what dragging the last row further
        // down amounts to.
        assert_eq!(settled(4, 9, 0, 5), None);
    }

    /// Nothing to reorder in a queue with only the track that is playing.
    #[test]
    fn one_track_playing_is_not_a_queue_to_reorder() {
        assert_eq!(settled(1, 1, 0, 1), None);
        assert_eq!(settled(0, 0, 0, 0), None);
        // And an index past the end of it, which is what a stale row would ask for.
        assert_eq!(settled(7, 1, 0, 3), None);
    }

    /// A mix touches what is coming and nothing else.
    ///
    /// Which is the rule the whole queue is built on: the track sounding stays
    /// sounding and what has been heard stays where it was heard, so pressing shuffle
    /// never interrupts the music.
    #[test]
    fn a_mix_leaves_what_is_sounding_and_what_is_behind_it_alone() {
        // Five queued with the third sounding, so only the last two may move.
        let mut queued = queue("ABCDE");
        mix(&mut queued, 3, first);

        assert_eq!(spelt(&queued), "ABCED");

        // And from the top of the queue, where everything but the first is waiting.
        let mut queued = queue("ABCDE");
        mix(&mut queued, 1, first);

        assert_eq!(spelt(&queued), "ACDEB");
    }

    /// Nothing to mix at the end of a queue, and nothing that falls over there either.
    #[test]
    fn a_queue_with_nothing_waiting_comes_out_as_it_went_in() {
        let mut queued = queue("ABC");
        mix(&mut queued, 2, first);
        assert_eq!(spelt(&queued), "ABC", "one waiting is one ordering");

        mix(&mut queued, 3, first);
        assert_eq!(spelt(&queued), "ABC", "the last track is sounding");

        // Past the end of it, which is what the last track of a queue asks for.
        mix(&mut queued, 9, first);
        assert_eq!(spelt(&queued), "ABC");
    }

    /// Putting the shuffle away gives back the order it was given.
    #[test]
    fn what_is_waiting_goes_back_into_the_order_it_came_in() {
        let ordered = queue("ABCDE");
        let mut queued = ordered.clone();
        mix(&mut queued, 1, first);

        assert_eq!(spelt(&ordered_again(&ordered, &queued, 0)), "ABCDE");
    }

    /// What has been heard in a shuffle stays heard, in the order it was heard in.
    ///
    /// The queue has moved on inside the mix, so the first three of `ACDEB` are what
    /// this listening actually was. Ordering what is coming may not rewrite it.
    #[test]
    fn putting_it_back_does_not_reorder_what_has_already_sounded() {
        let ordered = queue("ABCDE");
        let queued = queue("ACDEB");

        assert_eq!(spelt(&ordered_again(&ordered, &queued, 2)), "ACDBE");
    }

    /// A track taken out of the mix does not come back with the order.
    #[test]
    fn a_track_dropped_from_the_mix_stays_dropped() {
        let ordered = queue("ABCDE");
        // Mixed, and then D swiped out of what was coming.
        let queued = queue("ACEB");

        assert_eq!(spelt(&ordered_again(&ordered, &queued, 0)), "ABCE");
    }

    /// A track queued while the mix is on is still there when the mix is put away.
    ///
    /// Which is what `queue_up` keeps two orders for: `ABCDE` mixed into `ACDEB` and
    /// then `F` added goes on the end of both, and this is the half that says why —
    /// anything waiting that the old order does not name is dropped on the way back.
    #[test]
    fn a_track_queued_during_a_mix_is_still_there_after_ordering() {
        assert_eq!(
            spelt(&ordered_again(&queue("ABCDEF"), &queue("ACDEBF"), 0)),
            "ABCDEF"
        );

        // And the same track without it, which is what forgetting the second order
        // would leave behind.
        assert_eq!(
            spelt(&ordered_again(&queue("ABCDE"), &queue("ACDEBF"), 0)),
            "ABCDE"
        );
    }

    /// The same track queued twice comes back twice, and comes back once when one of
    /// the two has gone.
    ///
    /// Which is why the old order is walked against a list that is consumed rather
    /// than merely asked whether it holds something: a track named twice up there and
    /// waiting once down here would otherwise be written back out twice, and one of
    /// them is a copy the listener took out.
    #[test]
    fn a_track_queued_twice_comes_back_as_often_as_it_is_still_waiting() {
        let ordered = queue("ABCB");

        assert_eq!(spelt(&ordered_again(&ordered, &queue("ABBC"), 0)), "ABCB");
        assert_eq!(spelt(&ordered_again(&ordered, &queue("ABC"), 0)), "ABC");
    }
}
