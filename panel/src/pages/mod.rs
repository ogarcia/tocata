// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! One module per section of the panel.
//!
//! Only home is written. The rest announce themselves as unbuilt, which is honest
//! and takes three lines, and each will be filled in as its turn comes.
//!
//! There is no screen for the scan. A scan is not a place, it is something the
//! server is doing, so it shows up on the screen you are already looking at and
//! goes away when there is nothing to report.
//!
//! Your own account and the accounts are two modules, and the line between them is
//! whose account it is. Administering somebody is not the same job as looking after
//! yourself, even where the calls underneath are the same ones.

pub mod account;
pub mod accounts;
pub mod albums;
pub mod artists;
pub mod endless;
pub mod genres;
pub mod home;
pub mod libraries;
pub mod maintenance;
pub mod settings;
pub mod tracks;

use crate::api::Failure;
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use rust_i18n::t;
use wasm_bindgen::JsCast;

/// A section that exists in the menu and nowhere else yet.
///
/// Said twice on purpose: once as the lead, where every screen says what it is, and
/// once in the space the screen would have filled. The empty band is what keeps it
/// from reading as a screen that failed to load.
#[component]
pub fn Unbuilt(heading: String) -> impl IntoView {
    view! {
        <header class="titled">
            <div>
                <h1>{heading}</h1>
                <p class="quiet lead">{t!("common.not_yet")}</p>
            </div>
        </header>

        <p class="nothing">{t!("common.nothing_here")}</p>
    }
}

/// The three dots at the end of a row, and the menu they open.
///
/// Two screens have rows with more than one thing to do about them, and this is the
/// shape both use. Which is worth one component rather than two copies mostly for
/// the awkward part: where the menu goes.
///
/// It is fixed to the viewport rather than hung off the row. A menu is taller than
/// the row it belongs to and both of these rows sit in something that clips —
/// a box that scrolls sideways, or a column half a screen wide — and anything
/// overflowing a scrolling box is cut off by it. Fixed escapes that; the price is
/// working out where to put it, which is one rectangle read off the button and the
/// height of the window.
///
/// It opens downward where there is room and upward where there is not, and what
/// still does not fit scrolls inside it. Fixed to the viewport means a menu running
/// off the bottom cannot be scrolled to: the last rows of a long list would offer
/// items nobody could reach.
///
/// Any click inside the menu closes it, after whatever was clicked has run. So an
/// item is written as the thing it does and nothing has to remember to shut the
/// menu afterwards.
#[component]
pub fn Dots(
    /// What the button is for, for whatever reads the page aloud.
    title: String,
    /// Held shut while something is already being done to the row.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    children: ChildrenFn,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    // Where the menu goes, as the declarations that put it there: which edge it hangs
    // from depends on the room around the button, so it is worked out here rather
    // than written in the stylesheet.
    let (at, set_at) = signal(String::new());

    let toggle = move |event: web_sys::MouseEvent| {
        if let Some(button) = event
            .current_target()
            .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let rect = button.get_bounding_client_rect();

            let window = web_sys::window()
                .and_then(|window| window.inner_height().ok())
                .and_then(|height| height.as_f64())
                .unwrap_or(0.0);

            let below = window - rect.bottom();
            let above = rect.top();

            // Whichever side has more room, and never taller than that room: a menu
            // that would run past the edge scrolls instead.
            let side = if below >= above {
                format!(
                    "top: {}px; max-height: {}px",
                    rect.bottom() + 4.0,
                    room(below)
                )
            } else {
                format!(
                    "bottom: {}px; max-height: {}px",
                    window - rect.top() + 4.0,
                    room(above)
                )
            };

            set_at.set(format!("{side}; right: calc(100vw - {}px)", rect.right()));
        }

        set_open.update(|shown| *shown = !*shown);
    };

    view! {
        <button
            class="dots"
            title=title
            disabled=move || disabled.get()
            aria-expanded=move || open.get().to_string()
            on:click=toggle
        >
            <Glyph icon=Icon::More />
        </button>

        <Show when=move || open.get()>
            // Under the menu and over the page, so it catches every click except the
            // ones meant for the menu itself.
            <div class="veil" on:click=move |_| set_open.set(false)></div>
            <div class="menu afloat" style=at on:click=move |_| set_open.set(false)>
                {children()}
            </div>
        </Show>
    }
}

/// A label, what it is for, and whatever sets it.
///
/// Two children exactly: the stylesheet gives the first the label column and the
/// second the control, and lets the second drop under the first when the row runs
/// out of width.
#[component]
pub fn Setting(
    label: String,
    /// What the row is for. A signal rather than a string because one row's answer
    /// changes what the row means — the quarantine on Settings says how long
    /// something stays marked, and the number is in the row itself — while every
    /// other caller hands over a constant, which becomes a signal that never
    /// fires.
    #[prop(optional, into)]
    why: Signal<String>,
    /// The row that answers for all the others, said in the accent.
    #[prop(optional)]
    asked: bool,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="setting" class:asked=asked>
            <div>
                <span>{label}</span>
                <Show when=move || !why.get().is_empty()>
                    <span class="why">{move || why.get()}</span>
                </Show>
            </div>
            <div>{children()}</div>
        </div>
    }
}

/// Grouped with a space, which every language this speaks agrees on and no
/// language mistakes for a decimal point.
pub fn thousands(count: i64) -> String {
    let digits = count.abs().to_string();
    let mut out = String::new();

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }

    if count < 0 { format!("-{out}") } else { out }
}

/// How long one track lasts.
///
/// Zero-padded from the minutes down but never at the front: "3:44" rather than
/// "03:44", which is how a length is written everywhere it is read as one. Hours
/// appear only when there are any — a recording of a whole concert is one track,
/// and "97:20" is not how anybody says an hour and a half.
pub fn length(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// How long a record runs, in minutes and seconds however many minutes that is.
///
/// Never hours, unlike a single track. A record sits on a line with two other
/// figures beside it, and a third group of digits appearing on the long ones alone
/// is what makes that line ragged down a shelf — where "78:45" reads as a record's
/// length to anybody who has ever looked at the back of a sleeve.
pub fn runs(seconds: i64) -> String {
    let seconds = seconds.max(0);

    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Powers of two, and the unit spelled the way the standard spells it. Not
/// translated: a unit symbol is a symbol.
pub fn bytes(count: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut size = count as f64;
    let mut unit = 0;

    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{count} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// What is left of a span of room once the menu has been given air at both ends.
fn room(space: f64) -> f64 {
    (space - 16.0).max(0.0)
}

/// How long ago a moment was, said the way somebody would say it.
///
/// Relative rather than absolute wherever the point is recency: "2 h ago" is what
/// somebody wants to know about the last scan, and it fits a column where
/// "28/07/2026, 20:08:27" does not — it was that timestamp, wrapping onto a second
/// line, that made a library's row look like it had the date underneath the rest
/// rather than beside it.
///
/// Abbreviated on purpose: "3 min" needs no plural, and rust-i18n has no
/// pluralisation to lean on even if it did.
pub fn since(iso: &str) -> String {
    match elapsed(iso) {
        None => iso.to_string(),
        Some(seconds) if seconds < 60.0 => t!("ago.moments").to_string(),
        Some(seconds) if seconds < 3600.0 => {
            t!("ago.minutes", count = (seconds / 60.0).floor()).to_string()
        }
        Some(seconds) if seconds < 86_400.0 => {
            t!("ago.hours", count = (seconds / 3600.0).floor()).to_string()
        }
        Some(seconds) => t!("ago.days", count = (seconds / 86_400.0).floor()).to_string(),
    }
}

/// The same span with the "ago" left off, for a figure standing over a label that
/// has already said what happened.
///
/// "Hace 5 h" set at twenty eight pixels reads as a sentence somebody shouted. The
/// figures on a screen are quantities, and the quantity here is five hours.
pub fn lapse(iso: &str) -> String {
    match elapsed(iso) {
        None => MISSING.to_string(),
        Some(seconds) if seconds < 60.0 => t!("ago.just_now").to_string(),
        Some(seconds) if seconds < 3600.0 => {
            t!("ago.short_minutes", count = (seconds / 60.0).floor()).to_string()
        }
        Some(seconds) if seconds < 86_400.0 => {
            t!("ago.short_hours", count = (seconds / 3600.0).floor()).to_string()
        }
        Some(seconds) => t!("ago.short_days", count = (seconds / 86_400.0).floor()).to_string(),
    }
}

/// Seconds between then and now, or `None` if that was not a moment.
///
/// Never negative. A clock a few seconds ahead of the server's would otherwise turn
/// "just now" into a span into the future, which reads as a mistake because it is
/// one — just not the reader's.
fn elapsed(iso: &str) -> Option<f64> {
    let then = js_sys::Date::parse(iso);

    (!then.is_nan()).then(|| ((js_sys::Date::now() - then) / 1000.0).max(0.0))
}

/// Stands in for a value nothing has reported yet, or a moment that cannot be
/// worked out. A dash rather than a zero: none of these is a quantity, and zero is
/// an answer where this is the absence of one.
pub const MISSING: &str = "—";

/// A timestamp the way the reader's own machine writes one.
pub fn when(iso: &str) -> String {
    let moment = js_sys::Date::new(&iso.into());

    if moment.get_time().is_nan() {
        return iso.to_string();
    }

    moment
        .to_locale_string(&crate::locale::current(), &js_sys::Object::new())
        .into()
}

/// The day, without the hour it fell on.
///
/// What an expiry is: a key is given until a date, and a client that stops working
/// at midnight has not been told anything useful by "23:59:59". The same goes for a
/// session, whose thirty days end on a day.
pub fn on_day(iso: &str) -> String {
    let moment = js_sys::Date::new(&iso.into());

    if moment.get_time().is_nan() {
        return iso.to_string();
    }

    moment
        .to_locale_date_string(&crate::locale::current(), &js_sys::Object::new())
        .into()
}

/// What to say about a refusal. The codes are the server's own and stable, so this
/// can branch on them and say something useful in the reader's language.
pub fn said(why: &Failure) -> String {
    match why {
        Failure::Unreachable => t!("login.unreachable").to_string(),
        Failure::Refused(code) => match code.as_str() {
            "conflict" => t!("accounts.taken").to_string(),
            "invalidRequest" => t!("accounts.invalid").to_string(),
            "notAuthorized" => t!("accounts.not_allowed").to_string(),
            "wrongPassword" => t!("accounts.wrong_password").to_string(),
            _ => t!("common.refused").to_string(),
        },
        Failure::Unauthenticated => t!("common.refused").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{length, runs};

    #[test]
    fn a_length_is_written_the_way_a_length_is_read() {
        assert_eq!(length(224), "3:44");
        assert_eq!(length(0), "0:00");
        assert_eq!(length(9), "0:09");
        assert_eq!(length(60), "1:00");
        // The minutes are padded once there are hours in front of them, and not
        // before: "1:2:03" is not a time.
        assert_eq!(length(3723), "1:02:03");
        assert_eq!(length(36_000), "10:00:00");
        // Nothing sends a negative length. If something did, the row would say zero
        // rather than an hour with a minus in the middle of it.
        assert_eq!(length(-5), "0:00");
    }

    /// A record's length never grows a third group of digits, however long it is.
    #[test]
    fn a_record_runs_for_minutes_however_many_there_are() {
        assert_eq!(runs(2796), "46:36");
        assert_eq!(runs(0), "0:00");
        assert_eq!(runs(59), "0:59");
        // An hour and a quarter, which a track would call 1:15:00 and a record
        // calls what it says on the sleeve.
        assert_eq!(runs(4500), "75:00");
        // A boxed set. Still minutes, because the alternative is one shelf where
        // some records carry hours and the rest do not.
        assert_eq!(runs(38_524), "642:04");
        assert_eq!(runs(-5), "0:00");
    }
}
