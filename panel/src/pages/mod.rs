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
pub mod home;
pub mod libraries;

use crate::api::Failure;
use leptos::prelude::*;
use rust_i18n::t;

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
