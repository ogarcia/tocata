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
    let then = js_sys::Date::parse(iso);
    if then.is_nan() {
        return iso.to_string();
    }

    let seconds = ((js_sys::Date::now() - then) / 1000.0).max(0.0);

    if seconds < 60.0 {
        t!("home.moments").to_string()
    } else if seconds < 3600.0 {
        t!("home.minutes", count = (seconds / 60.0).floor()).to_string()
    } else if seconds < 86_400.0 {
        t!("home.hours", count = (seconds / 3600.0).floor()).to_string()
    } else {
        t!("home.days", count = (seconds / 86_400.0).floor()).to_string()
    }
}

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
