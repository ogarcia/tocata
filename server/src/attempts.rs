// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Making somebody wait after enough failed logins.
//!
//! A password anybody may guess at as fast as they can send requests is a
//! password with far fewer bits than it looks. Argon2 already makes each guess
//! expensive, which is most of the answer; this is the rest of it, and it is also
//! what keeps the machine from spending its whole processor hashing guesses.
//!
//! Counted by where the request came from rather than by which account was named.
//! Counting by account would turn this screen into a way of asking whether a name
//! exists — try it, and being told to wait means somebody is there — and it would
//! let anybody lock a known account out by getting its password wrong on purpose.
//! Neither is true of counting by origin.
//!
//! Nothing is said about how many tries are left, for the same reason. What the
//! panel shows is that there has been a wrong password, and separately that too
//! many have arrived from here and it is time to wait.
//!
//! In memory rather than in the database. These are worth nothing once the server
//! has stopped — restarting is something the administrator does, not something
//! whoever is guessing can ask for — and a table would mean a write per wrong
//! password, which is the one kind of request worth making cheap.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many may go wrong from one place before it has to wait.
///
/// Enough for somebody who genuinely cannot remember which of their passwords it
/// was, and few enough to make guessing pointless.
const ALLOWED: u32 = 5;

/// How long the wait lasts — and, the same thing said from the other side, how
/// long a run of failures is remembered. A place that stops trying is forgotten
/// rather than kept on a list to be punished tomorrow.
const PAUSE: Duration = Duration::from_secs(60);

/// What is known about one place that has been getting it wrong.
struct Record {
    failures: u32,
    /// When the last one arrived. Trying again during the wait moves this, so
    /// hammering the door keeps it shut.
    last: Instant,
}

#[derive(Default)]
pub struct Attempts {
    places: Mutex<HashMap<IpAddr, Record>>,
}

impl Attempts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this place has to wait before it may try again.
    pub fn barred(&self, who: IpAddr) -> bool {
        self.barred_at(who, Instant::now())
    }

    /// Notes that a login from here did not check out.
    pub fn failed(&self, who: IpAddr) {
        self.failed_at(who, Instant::now());
    }

    /// Forgets a place, because somebody there got in. A right password says
    /// whoever is at the keyboard is who they say they are, and the four wrong
    /// ones before it were the same person mistyping.
    pub fn succeeded(&self, who: IpAddr) {
        if let Ok(mut places) = self.places.lock() {
            places.remove(&who);
        }
    }

    fn barred_at(&self, who: IpAddr, now: Instant) -> bool {
        let Ok(mut places) = self.places.lock() else {
            // The lock is only ever held for the few lines below, so this means a
            // thread panicked mid-update. Letting the login through is the right
            // way to be wrong: the alternative is a server that has locked
            // everybody out until it restarts.
            return false;
        };

        forget_the_quiet_ones(&mut places, now);

        places
            .get(&who)
            .is_some_and(|record| record.failures >= ALLOWED)
    }

    fn failed_at(&self, who: IpAddr, now: Instant) {
        let Ok(mut places) = self.places.lock() else {
            return;
        };

        forget_the_quiet_ones(&mut places, now);

        places
            .entry(who)
            .and_modify(|record| {
                record.failures += 1;
                record.last = now;
            })
            .or_insert(Record {
                failures: 1,
                last: now,
            });
    }
}

/// Drops every place that has not tried again within the wait.
///
/// Done on the way past rather than on a timer of its own: the map is only ever
/// touched by a login, so a server nobody is logging into has nothing to tidy.
/// It also bounds the map to the places that have tried in the last minute, which
/// is what keeps a flood of addresses from being a way to fill memory.
fn forget_the_quiet_ones(places: &mut HashMap<IpAddr, Record>, now: Instant) {
    places.retain(|_, record| now.duration_since(record.last) < PAUSE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn somewhere() -> IpAddr {
        "203.0.113.7".parse().unwrap()
    }

    fn elsewhere() -> IpAddr {
        "203.0.113.8".parse().unwrap()
    }

    #[test]
    fn a_few_wrong_passwords_are_a_person_mistyping() {
        let attempts = Attempts::new();

        for _ in 0..ALLOWED - 1 {
            attempts.failed(somewhere());
            assert!(!attempts.barred(somewhere()));
        }
    }

    #[test]
    fn enough_of_them_and_it_has_to_wait() {
        let attempts = Attempts::new();

        for _ in 0..ALLOWED {
            attempts.failed(somewhere());
        }

        assert!(attempts.barred(somewhere()));
    }

    /// The whole reason it is counted by origin: one place getting it wrong must
    /// not keep anybody else out.
    #[test]
    fn one_place_waiting_does_not_bar_another() {
        let attempts = Attempts::new();

        for _ in 0..ALLOWED {
            attempts.failed(somewhere());
        }

        assert!(attempts.barred(somewhere()));
        assert!(!attempts.barred(elsewhere()));
    }

    /// Whoever finally remembers their password is not made to wait for the four
    /// tries it took them.
    #[test]
    fn getting_in_forgets_what_came_before() {
        let attempts = Attempts::new();

        for _ in 0..ALLOWED - 1 {
            attempts.failed(somewhere());
        }
        attempts.succeeded(somewhere());

        for _ in 0..ALLOWED - 1 {
            attempts.failed(somewhere());
        }

        assert!(!attempts.barred(somewhere()), "the count started over");
    }

    /// A place that gives up is forgotten rather than kept on a list.
    #[test]
    fn the_wait_runs_out() {
        let attempts = Attempts::new();
        let then = Instant::now();

        for _ in 0..ALLOWED {
            attempts.failed_at(somewhere(), then);
        }

        assert!(attempts.barred_at(somewhere(), then));
        assert!(!attempts.barred_at(somewhere(), then + PAUSE));
    }

    /// Trying again while barred keeps it barred, so hammering the door is not a
    /// way to run the clock down.
    #[test]
    fn trying_again_during_the_wait_starts_it_over() {
        let attempts = Attempts::new();
        let then = Instant::now();

        for _ in 0..ALLOWED {
            attempts.failed_at(somewhere(), then);
        }

        // Half way through the wait, one more guess.
        attempts.failed_at(somewhere(), then + PAUSE / 2);

        assert!(
            attempts.barred_at(somewhere(), then + PAUSE),
            "the wait runs from the last attempt, not the first"
        );
    }

    /// Nothing is kept about a place that has stopped, which is what stops a
    /// flood of addresses being a way to fill memory.
    #[test]
    fn nothing_is_remembered_about_a_place_that_stopped() {
        let attempts = Attempts::new();
        let then = Instant::now();

        for address in 0..50u8 {
            let who: IpAddr = format!("203.0.113.{address}").parse().unwrap();
            attempts.failed_at(who, then);
        }

        assert_eq!(attempts.places.lock().unwrap().len(), 50);

        attempts.failed_at(somewhere(), then + PAUSE);

        assert_eq!(
            attempts.places.lock().unwrap().len(),
            1,
            "only the one still trying"
        );
    }
}
