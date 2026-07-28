// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What this process is costing the machine it runs on.
//!
//! Read straight out of `/proc`, which is where the kernel already keeps it. A
//! crate could do this on more systems than one, and would bring a dependency,
//! a build of its own for every target and a set of platform backends for
//! platforms this does not ship to, all to read two small files. What is here is
//! those two reads.
//!
//! Linux only, which is the whole of what a static musl binary is for. Nothing
//! guards against `/proc` being absent: on the systems this runs on it is there,
//! and a figure that cannot be read comes back as an error rather than as a
//! plausible zero.
//!
//! Processor time is a counter, not a level, so a share of the machine only means
//! anything between two readings of it. This keeps the last one and reports what
//! was used since — which makes the answer as instantaneous as the asking is
//! frequent, and never a snapshot of an instant that nothing could measure.

use crate::types::Resources;
use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Units the kernel reports processor time in, per second.
///
/// Fixed at 100 by the interface rather than by the kernel's own tick rate,
/// which is configurable and is not this. `sysconf(_SC_CLK_TCK)` is the portable
/// way to ask and it needs libc to call it; this is the answer on every
/// architecture the binary is built for.
const TICKS_PER_SECOND: u64 = 100;

/// Below this, two readings are too close together for the difference between
/// them to say anything: a few milliseconds of processor time either side of a
/// rounding boundary would swing the share wildly. The previous answer is given
/// again instead, which is what a second panel asking right after the first one
/// should see anyway.
const TOO_SOON: Duration = Duration::from_millis(500);

/// A reading, and what was made of it.
struct Reading {
    at: Instant,
    /// Processor time used by this process since it started.
    used: Duration,
    /// The share worked out when this reading was taken, kept so an asker who
    /// arrives too soon after gets an answer rather than a gap.
    share: f64,
}

/// Holds on to the last reading, which is what turns a counter into a rate.
pub struct Meter {
    previous: Mutex<Reading>,
    /// What full use of the machine means, so the share has a ceiling of a
    /// hundred however many threads the process spreads over.
    cores: f64,
}

impl Meter {
    /// Takes the first reading, so the first answer has something to be measured
    /// against.
    pub fn new() -> io::Result<Self> {
        let cores = std::thread::available_parallelism()
            .map(|cores| cores.get())
            .unwrap_or(1);

        Ok(Self {
            previous: Mutex::new(Reading {
                at: Instant::now(),
                used: processor_time()?,
                share: 0.0,
            }),
            cores: cores as f64,
        })
    }

    /// How much of the machine this process is using, and how much memory it is
    /// holding.
    ///
    /// The share covers the time since this was last called, so a caller asking
    /// on a timer gets its own interval and a caller asking once gets the average
    /// over however long it left between.
    pub fn read(&self) -> io::Result<Resources> {
        let used = processor_time()?;
        let now = Instant::now();

        // Poisoning would mean a panic between the two lines below, which do not
        // panic. Recovering the reading is better than propagating a failure
        // nothing can act on.
        let mut previous = self
            .previous
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let elapsed = now.duration_since(previous.at);

        let share = if elapsed < TOO_SOON {
            previous.share
        } else {
            let spent = used.saturating_sub(previous.used).as_secs_f64();
            let percentage = 100.0 * spent / (elapsed.as_secs_f64() * self.cores);

            // A process cannot use more of the machine than there is, but a
            // reading taken as threads are being accounted for can look like it
            // did.
            let share = percentage.clamp(0.0, 100.0);

            *previous = Reading {
                at: now,
                used,
                share,
            };

            share
        };

        Ok(Resources {
            // Two decimals: a figure that redraws every couple of seconds and
            // shows every last digit is a figure nobody can read.
            cpu: (share * 100.0).round() / 100.0,
            cores: self.cores as i64,
            memory: resident_memory()?,
            memory_total: total_memory(),
        })
    }
}

/// Processor time this process has used, user and kernel side together.
///
/// The second field of the line is the executable's name in brackets and may
/// contain spaces and brackets of its own, so the fields are counted from the
/// last bracket rather than from the start.
fn processor_time() -> io::Result<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat")?;

    let after_name = stat
        .rfind(')')
        .map(|end| &stat[end + 1..])
        .ok_or_else(|| malformed("/proc/self/stat"))?;

    // The field after the name is the third of the line, so the eleventh and
    // twelfth from here are utime and stime.
    let mut fields = after_name.split_whitespace().skip(11);
    let user = ticks(fields.next())?;
    let kernel = ticks(fields.next())?;

    Ok(Duration::from_secs_f64(
        (user + kernel) as f64 / TICKS_PER_SECOND as f64,
    ))
}

fn ticks(field: Option<&str>) -> io::Result<u64> {
    field
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| malformed("/proc/self/stat"))
}

/// Memory this process is actually holding, in bytes.
///
/// The resident size rather than the virtual one: what a process has mapped says
/// almost nothing, and an allocator that reserves generously would show as a
/// server eating the machine.
fn resident_memory() -> io::Result<i64> {
    let status = std::fs::read_to_string("/proc/self/status")?;

    kilobytes(&status, "VmRSS:").ok_or_else(|| malformed("/proc/self/status"))
}

/// What the machine has, so the figure above has something to be a share of.
///
/// The one thing here that is about the machine rather than about us, and the one
/// that is allowed to be missing: without it there is a number to show and no
/// scale to show it against, which is worse than the other way round but not
/// worth failing a request over.
fn total_memory() -> Option<i64> {
    kilobytes(&std::fs::read_to_string("/proc/meminfo").ok()?, "MemTotal:")
}

/// Finds a `Name: 1234 kB` line and returns the value in bytes.
fn kilobytes(text: &str, name: &str) -> Option<i64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<i64>().ok())
        .map(|value| value * 1024)
}

fn malformed(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("cannot read {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the real file, name and all: a process called something with
    /// spaces and brackets in it is exactly what the counting has to survive.
    const STAT: &str = "42 (tocata (test) x) S 1 42 42 0 -1 4194560 1234 0 0 0 \
                        700 300 0 0 20 0 9 0 187 0 0 0";

    #[test]
    fn processor_time_is_counted_from_the_end_of_the_name() {
        let after_name = STAT.rfind(')').map(|end| &STAT[end + 1..]).unwrap();
        let mut fields = after_name.split_whitespace().skip(11);

        assert_eq!(ticks(fields.next()).unwrap(), 700, "utime");
        assert_eq!(ticks(fields.next()).unwrap(), 300, "stime");
    }

    #[test]
    fn a_value_in_kilobytes_comes_back_in_bytes() {
        let status = "Name:\ttocata\nVmPeak:\t  900 kB\nVmRSS:\t  4096 kB\nThreads:\t9\n";

        assert_eq!(kilobytes(status, "VmRSS:"), Some(4_194_304));
        assert_eq!(kilobytes(status, "VmSwap:"), None, "not there");
    }

    /// The name has to match at the start of the line. `VmRSS` and `VmHWM` are
    /// different figures and a substring search would confuse similar ones.
    #[test]
    fn a_name_is_not_confused_with_another() {
        let status = "VmHWM:\t  8192 kB\nVmRSS:\t  4096 kB\n";

        assert_eq!(kilobytes(status, "VmRSS:"), Some(4_194_304));
        assert_eq!(kilobytes(status, "VmHWM:"), Some(8_388_608));
    }

    /// Reading it twice in a row is the case that would divide by almost nothing.
    #[test]
    fn two_readings_at_once_do_not_divide_by_nothing() {
        let meter = Meter::new().unwrap();

        let first = meter.read().unwrap();
        let second = meter.read().unwrap();

        assert_eq!(first.cpu, second.cpu, "the same answer, not a wild one");
        assert!((0.0..=100.0).contains(&second.cpu));
    }

    /// A share of the machine, not of a core: whatever the process does, a meter
    /// that goes past its own ceiling is a broken meter.
    #[test]
    fn the_share_stays_within_the_machine() {
        let meter = Meter::new().unwrap();

        // Something to actually account for, so the reading is not all zeroes.
        let mut spun = 0u64;
        let until = Instant::now() + TOO_SOON * 2;
        while Instant::now() < until {
            spun = spun.wrapping_add(1);
        }

        let read = meter.read().unwrap();

        assert!(
            (0.0..=100.0).contains(&read.cpu),
            "a share of {} is not a share",
            read.cpu
        );
        assert!(read.memory > 0, "a running process holds memory");
        assert_eq!(read.cores, meter.cores as i64);
    }
}
