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

pub mod home;
pub mod libraries;

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
