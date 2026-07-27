// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! One module per section of the panel.
//!
//! Only the overview is written. The rest announce themselves as unbuilt, which
//! is honest and takes three lines, and each will be filled in as its turn comes
//! — the scan next, because it is the one that needs the event stream.

pub mod overview;
pub mod scan;

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
