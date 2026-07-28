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
#[component]
pub fn Unbuilt(heading: String) -> impl IntoView {
    view! {
        <h1>{heading}</h1>
        <p class="quiet">{t!("common.not_yet")}</p>
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
