// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Tocata's administration panel.
//!
//! A client of `/api/v1` and nothing else. It holds no state the server does not
//! already hold, which is what keeps a reload from being a way to lose anything.
//!
//! The shapes it exchanges come from the server's own crate, with everything that
//! needs a database or a socket switched off by a feature. Rename a field there
//! and this stops compiling, which is the whole reason the panel is in Rust.

mod accent;
mod api;
mod events;
mod icon;
mod layout;
mod locale;
mod login;
mod pages;
mod theme;

// Compiles the translations in. `fallback` is what a key missing from a
// translation falls back to, so a half translated language shows English rather
// than the name of the key.
//
// It reads the files without telling cargo, which is what build.rs is there to
// fix: without it, editing a translation rebuilds nothing.
rust_i18n::i18n!("locales", fallback = "en");

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;
use rust_i18n::t;
use tocata::types::Identity;

/// Who is logged in, or nobody, or not asked yet.
///
/// The third state matters: without it the form flashes up for a moment on every
/// reload before the answer to "who am I" arrives, which looks like being logged
/// out and is not.
#[derive(Clone, PartialEq, Eq)]
enum Who {
    Asking,
    Nobody,
    Somebody(Identity),
}

#[component]
fn Panel() -> impl IntoView {
    let (who, set_who) = signal(Who::Asking);

    // One question on load. A live cookie lands straight on the panel; anything
    // else puts the form up.
    //
    // The answer carries how the panel should look and speak, so it is applied
    // before the panel is built rather than after: the language especially, since
    // rust-i18n reads it as each string is rendered and nothing already on screen
    // would be rendered again.
    spawn_local(async move {
        set_who.set(match api::whoami().await {
            Ok(identity) => Who::Somebody(identity),
            Err(_) => Who::Nobody,
        });
    });

    let forget = Callback::new(move |()| set_who.set(Who::Nobody));

    view! {
        {move || match who.get() {
            Who::Asking => {
                view! {
                    <main class="entry">
                        <p class="quiet">{t!("common.loading")}</p>
                    </main>
                }
                    .into_any()
            }
            Who::Nobody => {
                view! {
                    <login::LogIn on_in=Callback::new(move |identity| {
                        set_who.set(Who::Somebody(identity))
                    }) />
                }
                    .into_any()
            }
            Who::Somebody(identity) => view! { <Inside identity forget /> }.into_any(),
        }}
    }
}

/// The panel proper, once there is somebody to show it to.
#[component]
fn Inside(identity: Identity, forget: Callback<()>) -> impl IntoView {
    let admin = identity.admin;
    let who = identity.clone();

    // What the account chose, over what this browser had cached from last time.
    // Both were already applied before the first paint; this is where they are
    // corrected, and where somebody logging in on a borrowed machine stops seeing
    // its owner's colours.
    let theme = theme::settle();
    let accent = accent::settle();

    theme::adopt(theme, identity.preferences.theme.as_deref());
    accent::adopt(&accent, identity.preferences.accent.as_deref());
    locale::adopt(identity.preferences.locale.as_deref());

    // Reached by the one screen that offers them, rather than threaded through
    // every route on the way there.
    provide_context(theme);
    provide_context(accent::Accent(accent));

    // One stream for the whole panel, opened here and read wherever it is wanted:
    // the header says a scan is running, and where you land shows that in full
    // alongside what the server is costing the machine. Two connections would be
    // two of everything for the same news.
    let live = events::open();
    let scan = live.scan;

    view! {
        <Router>
            <layout::Shell identity on_out=forget scan>
                <Routes fallback=move || {
                    view! { <pages::Unbuilt heading=t!("nav.home").to_string() /> }
                }>
                    <Route
                        path=path!("/")
                        view={
                            let who = who.clone();
                            move || {
                                view! {
                                    <pages::home::Home
                                        identity=who.clone()
                                        scan
                                        resources=live.resources
                                        admin
                                        on_expired=forget
                                    />
                                }
                            }
                        }
                    />
                    // Your own account, in three: who you are, what opens it, and
                    // how the panel looks to you. Not the screen an administrator
                    // sees on somebody else, because none of this is administration.
                    <Route
                        path=path!("/account")
                        view={
                            let who = who.clone();
                            move || {
                                view! {
                                    <pages::account::Profile who=who.clone() on_expired=forget />
                                }
                            }
                        }
                    />
                    <Route
                        path=path!("/account/access")
                        view={
                            let who = who.clone();
                            move || {
                                view! {
                                    <pages::account::Access who=who.clone() on_expired=forget />
                                }
                            }
                        }
                    />
                    <Route
                        path=path!("/account/preferences")
                        view=move || {
                            view! { <pages::account::Preferences on_expired=forget /> }
                        }
                    />

                    // The collection, four ways in and none of them built. They are
                    // in the menu already because the shape of it is a decision of
                    // its own, and an entry that says so is more honest than a menu
                    // that grows a section later.
                    <Route
                        path=path!("/tracks")
                        view=move || view! { <pages::Unbuilt heading=t!("nav.tracks").to_string() /> }
                    />
                    <Route
                        path=path!("/albums")
                        view=move || view! { <pages::Unbuilt heading=t!("nav.albums").to_string() /> }
                    />
                    <Route
                        path=path!("/artists")
                        view=move || {
                            view! { <pages::Unbuilt heading=t!("nav.artists").to_string() /> }
                        }
                    />
                    <Route
                        path=path!("/genres")
                        view=move || view! { <pages::Unbuilt heading=t!("nav.genres").to_string() /> }
                    />

                    // The administration sections. The menu does not offer these
                    // to anybody else and the server refuses them regardless, so
                    // what is left to handle here is a URL typed by hand.
                    <Route
                        path=path!("/libraries")
                        view=move || {
                            if admin {
                                view! { <pages::libraries::Libraries on_expired=forget /> }
                                    .into_any()
                            } else {
                                view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
                            }
                        }
                    />
                    <Route
                        path=path!("/accounts")
                        view={
                            let who = who.clone();
                            move || {
                                if admin {
                                    view! {
                                        <pages::accounts::Accounts
                                            who=who.clone()
                                            on_expired=forget
                                        />
                                    }
                                        .into_any()
                                } else {
                                    view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
                                }
                            }
                        }
                    />
                    <Route
                        path=path!("/accounts/:username")
                        view={
                            let who = who.clone();
                            move || {
                                if admin {
                                    view! {
                                        <pages::accounts::Detail
                                            who=who.clone()
                                            on_expired=forget
                                        />
                                    }
                                        .into_any()
                                } else {
                                    view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
                                }
                            }
                        }
                    />
                    <Route
                        path=path!("/settings")
                        view=move || {
                            view! { <Restricted admin heading=t!("nav.settings").to_string() /> }
                        }
                    />
                    <Route
                        path=path!("/maintenance")
                        view=move || {
                            view! { <Restricted admin heading=t!("nav.maintenance").to_string() /> }
                        }
                    />
                </Routes>
            </layout::Shell>
        </Router>
    }
}

/// Keeps the rights check in one place rather than repeated at every route.
#[component]
fn Restricted(admin: bool, heading: String) -> impl IntoView {
    if admin {
        view! { <pages::Unbuilt heading /> }.into_any()
    } else {
        view! { <p class="failure">{t!("login.failed")}</p> }.into_any()
    }
}

fn main() {
    console_error_panic_hook::set_once();
    locale::settle();
    leptos::mount::mount_to_body(Panel);
}

#[cfg(test)]
mod tests {
    use rust_i18n::t;

    /// The stylesheet, read at compile time so a test can be told about it.
    const STYLESHEET: &str = include_str!("../panel.css");

    /// Every file that draws something. Read rather than listed, so a screen added
    /// tomorrow is checked without anybody remembering to add it here — which is the
    /// same reason the translations are read from the source.
    const SOURCES: [&str; 9] = [
        include_str!("main.rs"),
        include_str!("icon.rs"),
        include_str!("layout.rs"),
        include_str!("login.rs"),
        include_str!("pages/mod.rs"),
        include_str!("pages/home.rs"),
        include_str!("pages/libraries.rs"),
        include_str!("pages/accounts.rs"),
        include_str!("pages/account.rs"),
    ];

    /// The stylesheet and the panel agree on which accent is the default.
    ///
    /// They have to, and nothing else can tell: choosing the default takes the
    /// attribute off the root element and leaves the stylesheet's own `--accent` to
    /// answer. When the two disagreed — green in the stylesheet, blue here — blue
    /// was unpickable, because picking it removed the attribute and handed back
    /// green, and green's own swatch did nothing, because green was already what you
    /// were looking at. Neither of those is a compile error and neither shows up in
    /// any other test.
    #[test]
    fn the_stylesheet_and_the_panel_default_to_the_same_accent() {
        let stylesheet = STYLESHEET;
        let expected = format!("--accent: var(--accent-{});", crate::accent::DEFAULT);

        assert!(
            stylesheet.contains(&expected),
            "the stylesheet does not fall back to {}: looked for {expected:?}",
            crate::accent::DEFAULT
        );

        assert_eq!(
            crate::accent::AVAILABLE[0],
            crate::accent::DEFAULT,
            "the default is offered first, so that the swatch that stores nothing is \
             the one at the start of the row"
        );

        for accent in crate::accent::AVAILABLE {
            let rule =
                format!("[data-accent=\"{accent}\"] {{ --accent: var(--accent-{accent}); }}");
            assert!(
                stylesheet.contains(&rule),
                "{accent} is offered and the stylesheet cannot colour it: looked for {rule:?}"
            );
        }
    }

    /// Nothing in the stylesheet styles a class the markup never wears.
    ///
    /// This does not catch a collision. It catches what every collision in the restyle
    /// needed first: a rule that outlived the screen it was written for. `.state` was
    /// the clearest — a margin from a screen that had been redrawn, sitting there
    /// until a new row happened to pick the same word, and then adding sixteen pixels
    /// under it. The rule was not wrong when it was written and nothing on screen said
    /// it was still there. Swept when its own screen went, it would not have been
    /// waiting.
    ///
    /// So the test is deliberately about the sheet's leftovers rather than about names
    /// meaning two things. Two screens sharing a name is how `quiet` and `lead` and
    /// `titled` are meant to work, and how `named` and `counts` were not, and telling
    /// those apart needs a list of blessed names — which somebody has to keep, and a
    /// list nobody keeps is a test that lies.
    #[test]
    fn the_stylesheet_styles_nothing_the_markup_does_not_wear() {
        let worn = classes_in_markup();
        assert!(
            worn.len() > 40,
            "the classes are read from the source, so a parse that finds none would pass \
             everything"
        );

        let mut unworn: Vec<&str> = styled_classes()
            .into_iter()
            .filter(|class| !worn.contains(class))
            .collect();
        unworn.sort_unstable();

        assert!(
            unworn.is_empty(),
            "the stylesheet styles classes nothing wears: {}",
            unworn.join(", ")
        );
    }

    /// No class is given the same property twice, in two rules, with two values.
    ///
    /// This is the shape of the `bar` breakage, which was the sharpest of them: the
    /// strip across the top of a narrow screen and the track under a figure were both
    /// `.bar`, one saying `display: none` and the other `display: block`. Same
    /// specificity, so the later one won, the top bar became two pixels of grey — and
    /// being displayed it still took a cell of the frame's grid, which put the whole
    /// screen in the column under the sections.
    ///
    /// Only bare single-class rules outside a media query, because that is the only
    /// case where two declarations of one property are always a mistake. Everywhere
    /// else it is how the sheet is written: `.pill` and then `.pill.solid`, `.link`
    /// and then `.link:hover`, a rule and then the same rule narrower inside an
    /// `@media`.
    #[test]
    fn no_class_is_told_the_same_property_twice() {
        let mut said: Vec<(String, String, String)> = Vec::new();

        for rule in rules() {
            if rule.inside_media {
                continue;
            }

            for selector in rule.selectors {
                let Some(class) = bare_class(selector) else {
                    continue;
                };

                for (property, value) in declarations(rule.body) {
                    said.push((class.to_string(), property, value));
                }
            }
        }

        for (index, (class, property, value)) in said.iter().enumerate() {
            for (other_class, other_property, other_value) in &said[index + 1..] {
                assert!(
                    !(class == other_class && property == other_property && value != other_value),
                    ".{class} is told {property} twice and differently: {value:?} and \
                     {other_value:?}. One name, two meanings — give the second one a name of \
                     its own rather than leaving the cascade to decide."
                );
            }
        }
    }

    /// Every class the stylesheet mentions in a selector.
    fn styled_classes() -> Vec<&'static str> {
        let mut found = Vec::new();

        for rule in rules() {
            for selector in rule.selectors {
                let mut rest = selector;
                while let Some(at) = rest.find('.') {
                    rest = &rest[at + 1..];
                    let end = rest
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                        .unwrap_or(rest.len());
                    let (name, after) = rest.split_at(end);
                    rest = after;

                    // A decimal in a value is not a class, and neither is anything
                    // that starts like one.
                    if !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) {
                        found.push(name);
                    }
                }
            }
        }

        found.sort_unstable();
        found.dedup();
        found
    }

    /// Every class the markup puts on an element, whether as text in a `class`
    /// attribute or as the name of a `class:` toggle.
    ///
    /// Read from near the word `class` rather than from every quoted string in the
    /// file, which is what this did first and why it missed `.elsewhere`: taking
    /// every pair of quotes in a source turns prose into class names, because a
    /// comment holding a single quotation mark puts the pairing out of step for
    /// everything after it. A class then counted as worn because some paragraph
    /// happened to use the word.
    ///
    /// A window rather than a strict pattern, because a class is not always a plain
    /// string: `class=if enabled { "state on" } else { "state" }` is two of them in
    /// one attribute, and a rule that only read `class="…"` would call both dead.
    fn classes_in_markup() -> Vec<&'static str> {
        /// Enough to cover the longest `class=` in the panel and not enough to reach
        /// the next attribute.
        const WINDOW: usize = 160;

        let mut found = Vec::new();

        for source in SOURCES {
            let mut at = 0;

            while let Some(next) = source[at..].find("class") {
                at += next + "class".len();

                // `classes_in_markup`, `.class` in a comment, and anything else that
                // merely contains the word.
                let follows = source[at..].chars().next().unwrap_or(' ');
                if follows != '=' && follows != ':' {
                    continue;
                }

                if follows == ':' {
                    // `class:chosen=…`, whose name is never quoted.
                    let rest = &source[at + 1..];
                    let end = rest
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                        .unwrap_or(rest.len());
                    found.push(&rest[..end]);
                    continue;
                }

                let window = &source[at..source.len().min(at + WINDOW)];
                let mut rest = window;

                while let Some(open) = rest.find('"') {
                    rest = &rest[open + 1..];
                    let Some(close) = rest.find('"') else { break };
                    let (quoted, after) = rest.split_at(close);
                    rest = after;

                    found.extend(quoted.split_whitespace());
                }
            }
        }

        found.sort_unstable();
        found.dedup();
        found
    }

    /// One rule of the stylesheet: what it selects, what it says, and whether it sits
    /// inside an `@media`.
    struct Rule {
        selectors: Vec<&'static str>,
        body: &'static str,
        inside_media: bool,
    }

    /// The stylesheet, comments stripped, as rules.
    ///
    /// Enough of a parser for a hand written sheet with no nesting beyond one level of
    /// `@media`, which is what this one is and what the file's own header says it will
    /// stay.
    fn rules() -> Vec<Rule> {
        let sheet = STYLESHEET;
        let mut found = Vec::new();
        let mut at = 0;
        let mut inside_media = false;

        while at < sheet.len() {
            let rest = &sheet[at..];

            // Comments are skipped rather than removed, so every slice below is still
            // a slice of the file itself.
            if let Some(after) = rest.strip_prefix("/*") {
                at += 2 + after.find("*/").map_or(after.len(), |end| end + 2);
                continue;
            }

            let next = rest.chars().next().unwrap_or(' ');
            if next.is_whitespace() {
                at += next.len_utf8();
                continue;
            }

            if next == '}' {
                inside_media = false;
                at += 1;
                continue;
            }

            let Some(open) = rest.find('{') else { break };

            if next == '@' {
                inside_media = true;
                at += open + 1;
                continue;
            }

            let Some(close) = rest[open..].find('}') else {
                break;
            };

            // The loop above has already walked past any comment before this, so what
            // is left of the head is the selector itself.
            let head = &rest[..open];

            found.push(Rule {
                selectors: head
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect(),
                body: &rest[open + 1..open + close],
                inside_media,
            });

            at += open + close + 1;
        }

        found
    }

    /// A selector that is one class and nothing else, which is the only shape where a
    /// second opinion about a property is always a mistake.
    fn bare_class(selector: &str) -> Option<&str> {
        let name = selector.strip_prefix('.')?;

        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            .then_some(name)
    }

    /// The properties a body sets, ignoring the custom ones: those are variables, and
    /// redefining a variable per selector is the whole mechanism the accents run on.
    fn declarations(body: &str) -> Vec<(String, String)> {
        strip_comments(body)
            .split(';')
            .filter_map(|line| line.split_once(':'))
            .map(|(property, value)| (property.trim().to_string(), value.trim().to_string()))
            .filter(|(property, _)| !property.starts_with("--") && !property.is_empty())
            .collect()
    }

    /// Comments out of a fragment, for the two places that need the text rather than
    /// the slice.
    fn strip_comments(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;

        while let Some(at) = rest.find("/*") {
            out.push_str(&rest[..at]);
            rest = match rest[at..].find("*/") {
                Some(end) => &rest[at + end + 2..],
                None => "",
            };
        }

        out.push_str(rest);
        out
    }

    /// Every key the code asks for, in both languages, resolving to something
    /// other than its own name.
    ///
    /// The check that exists because of `no`: YAML reads it as a boolean, so an
    /// unquoted `no:` becomes the key `false` and `t!("common.no")` quietly
    /// returns nothing useful. Comparing the answer against the key is what
    /// catches that, and it would have caught it before anybody saw "false" in a
    /// table.
    #[test]
    fn every_key_resolves_in_every_language() {
        let keys = collect_keys();
        assert!(
            keys.len() > 100,
            "the keys are read from the source, so a parse that finds none would pass everything"
        );

        for locale in ["en", "es"] {
            rust_i18n::set_locale(locale);

            for key in &keys {
                let key = key.as_str();
                let said = t!(key);
                assert_ne!(
                    said, key,
                    "{key} does not resolve in {locale}: rust-i18n hands back the key when it \
                     cannot find one"
                );
                assert!(!said.is_empty(), "{key} resolves to nothing in {locale}");

                // The check that `no` needed and the one above missed. YAML reads
                // yes, no, on and off as booleans in values too, so an unquoted
                // `no: No` resolves to the string "false" — which is not the key,
                // so comparing against the key said nothing was wrong, and what
                // ended up on screen was the word "false".
                assert!(
                    !matches!(said.as_ref(), "true" | "false"),
                    "{key} resolves to {said:?} in {locale}: YAML read the value as a boolean, \
                     which is what unquoted yes, no, on and off do"
                );
            }
        }
    }

    /// Read from the source rather than listed here, so a key added tomorrow is
    /// checked without anybody remembering to add it.
    fn collect_keys() -> Vec<String> {
        let mut keys = Vec::new();

        for source in SOURCES {
            let mut rest = source;

            while let Some(at) = rest.find("t!(") {
                // A macro whose name merely ends in a t is not this one. Without
                // this, `format!("{share:.1} %")` is read as a call to translate
                // `{share:.1}` — which has a dot in it, no space, and looks
                // exactly like one of ours.
                let ours = rest[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !before.is_alphanumeric() && before != '_');

                rest = &rest[at + 3..];

                if !ours {
                    continue;
                }

                let trimmed = rest.trim_start();

                if let Some(key) = trimmed
                    .strip_prefix('"')
                    .and_then(|quoted| quoted.split('"').next())
                    // Only what looks like one of ours, so the pattern in this
                    // very function does not count itself.
                    .filter(|key| key.contains('.') && !key.contains(' '))
                {
                    keys.push(key.to_string());
                }
            }
        }

        keys.sort();
        keys.dedup();
        keys
    }
}
