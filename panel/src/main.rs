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
mod drag;
mod drawer;
mod events;
mod icon;
mod layout;
mod locale;
mod login;
mod pages;
mod player;
mod queue;
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
                    <main class="awaiting">
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

    // One player for the whole panel, above the router so it outlives every screen:
    // walking from Tracks to Albums must not stop the music, and two of these would
    // be two things sounding at once. The screens put music into it and the sidebar
    // draws it, neither knowing about the other.
    provide_context(player::Player::new());

    // One stream for the whole panel, opened here and read wherever it is wanted:
    // the header says a scan is running, and where you land shows that in full
    // alongside what the server is costing the machine. Two connections would be
    // two of everything for the same news.
    let live = events::open();
    let scan = live.scan;

    // Also as a context, so that anything which cares whether a scan is running can
    // read it without being handed it through every screen on the way. What cares is
    // every collection listing: a scan changes what there is, so a list looking at the
    // answer from before one finished is a list that is wrong.
    provide_context(scan);

    // And how the walk for artist portraits is going, for the one screen that
    // draws it. A context for the same reason: it arrives on the stream that is
    // already open, and threading it down through the router would be handing
    // every screen something one of them reads.
    provide_context(live.portraits);

    // Who is logged in, held apart from the identity that arrived with the session so
    // that changing either of the two names takes effect where they are read.
    //
    // A context rather than a signal threaded down, and above all not a new identity:
    // the identity is what `Inside` was built from, so replacing it would rebuild the
    // whole panel and stop the music on the way past. What reads these repaints and
    // nothing else does.
    provide_context(layout::Me {
        username: RwSignal::new(identity.username.clone()),
        called: RwSignal::new(identity.display_name.clone()),
    });

    view! {
        <Router>
            <layout::Shell admin on_out=forget scan>
                <Routes fallback=move || {
                    view! { <pages::Unbuilt heading=t!("nav.home").to_string() /> }
                }>
                    <Route
                        path=path!("/")
                        view=move || {
                            view! {
                                <pages::home::Home
                                    scan
                                    resources=live.resources
                                    admin
                                    on_expired=forget
                                />
                            }
                        }
                    />
                    // Your own account, in three: who you are, what opens it, and
                    // how the panel looks to you. Not the screen an administrator
                    // sees on somebody else, because none of this is administration.
                    <Route
                        path=path!("/account")
                        view=move || view! { <pages::account::Profile on_expired=forget /> }
                    />
                    <Route
                        path=path!("/account/access")
                        view=move || view! { <pages::account::Access on_expired=forget /> }
                    />
                    <Route
                        path=path!("/account/preferences")
                        view=move || {
                            view! { <pages::account::Preferences on_expired=forget /> }
                        }
                    />

                    // The collection, four ways into the same music. Everybody
                    // reaches these: a listener is here for them, and an
                    // administrator wants to see what they are administering.
                    <Route
                        path=path!("/tracks")
                        view=move || view! { <pages::tracks::Tracks admin on_expired=forget /> }
                    />
                    <Route
                        path=path!("/albums")
                        view=move || view! { <pages::albums::Albums admin on_expired=forget /> }
                    />
                    <Route
                        path=path!("/artists")
                        view=move || view! { <pages::artists::Artists admin on_expired=forget /> }
                    />
                    <Route
                        path=path!("/genres")
                        view=move || view! { <pages::genres::Genres admin on_expired=forget /> }
                    />

                    // What is the account's own. Not administration and not the
                    // collection either — the same music, narrowed to what whoever is
                    // asking has marked.
                    <Route
                        path=path!("/favourites")
                        view=move || {
                            view! { <pages::favourites::Favourites on_expired=forget /> }
                        }
                    />

                    // The administration sections. The menu does not offer these
                    // to anybody else and the server refuses them regardless, so
                    // what is left to handle here is a URL typed by hand — and what
                    // to say to whoever typed it, which for a while was that their
                    // username and password did not go together.
                    <Route
                        path=path!("/libraries")
                        view=move || {
                            if admin {
                                view! { <pages::libraries::Libraries on_expired=forget /> }
                                    .into_any()
                            } else {
                                view! { <pages::NotForYou /> }.into_any()
                            }
                        }
                    />
                    <Route
                        path=path!("/accounts")
                        view=move || {
                            if admin {
                                view! { <pages::accounts::Accounts on_expired=forget /> }.into_any()
                            } else {
                                view! { <pages::NotForYou /> }.into_any()
                            }
                        }
                    />
                    <Route
                        path=path!("/accounts/:username")
                        view=move || {
                            if admin {
                                view! { <pages::accounts::Detail on_expired=forget /> }.into_any()
                            } else {
                                view! { <pages::NotForYou /> }.into_any()
                            }
                        }
                    />
                    <Route
                        path=path!("/settings")
                        view=move || {
                            if admin {
                                view! { <pages::settings::Settings on_expired=forget /> }.into_any()
                            } else {
                                view! { <pages::NotForYou /> }.into_any()
                            }
                        }
                    />
                    <Route
                        path=path!("/maintenance")
                        view=move || {
                            if admin {
                                view! { <pages::maintenance::Maintenance on_expired=forget /> }
                                    .into_any()
                            } else {
                                view! { <pages::NotForYou /> }.into_any()
                            }
                        }
                    />
                </Routes>
            </layout::Shell>
        </Router>
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
    /// Named as well as read, so a test that finds one word on two screens can say
    /// which two.
    const SOURCES: [(&str, &str); 24] = [
        ("main.rs", include_str!("main.rs")),
        ("icon.rs", include_str!("icon.rs")),
        ("layout.rs", include_str!("layout.rs")),
        ("drawer/mod.rs", include_str!("drawer/mod.rs")),
        ("drawer/album.rs", include_str!("drawer/album.rs")),
        ("drawer/artist.rs", include_str!("drawer/artist.rs")),
        ("drawer/genre.rs", include_str!("drawer/genre.rs")),
        ("drawer/track.rs", include_str!("drawer/track.rs")),
        ("login.rs", include_str!("login.rs")),
        ("pages/mod.rs", include_str!("pages/mod.rs")),
        ("pages/home.rs", include_str!("pages/home.rs")),
        ("pages/libraries.rs", include_str!("pages/libraries.rs")),
        ("pages/accounts.rs", include_str!("pages/accounts.rs")),
        ("pages/account.rs", include_str!("pages/account.rs")),
        ("pages/settings.rs", include_str!("pages/settings.rs")),
        ("pages/maintenance.rs", include_str!("pages/maintenance.rs")),
        ("pages/tracks.rs", include_str!("pages/tracks.rs")),
        ("pages/albums.rs", include_str!("pages/albums.rs")),
        ("pages/artists.rs", include_str!("pages/artists.rs")),
        ("pages/genres.rs", include_str!("pages/genres.rs")),
        ("pages/favourites.rs", include_str!("pages/favourites.rs")),
        ("pages/endless.rs", include_str!("pages/endless.rs")),
        ("player.rs", include_str!("player.rs")),
        ("queue.rs", include_str!("queue.rs")),
    ];

    /// The words more than one screen wears on purpose.
    ///
    /// The written half of the note over "the shared words" in the sheet, and the thing
    /// whose absence made two earlier attempts at a test impossible. Nothing mechanical
    /// can tell a word two screens share deliberately from two screens that reached for
    /// the same English word for different things — so the deliberate ones are listed,
    /// and everything else must be one screen's alone.
    ///
    /// Adding to it is a decision, not a formality. A word belongs here when it is the
    /// same thing on every screen that wears it: `quiet` is one grey, `pill` is one
    /// button, `sheet` is the dialogue wherever it opens from. It does not belong here
    /// when two screens happen to have picked the same word — `reading` was a figure on
    /// the Overview and a scrolling body in a panel, `acting` was a row of buttons on a
    /// library and the play glyph over a track number, `named` was a link in the roster
    /// and the title block of the opened player. The way out of those is a different
    /// word, never a line here.
    const SHARED: [&str; 75] = [
        // How something reads, wherever it is.
        "quiet",
        "lead",
        "part",
        "note",
        "hint",
        "lettering",
        "path",
        "failure",
        "figure",
        "what",
        "by",
        "who",
        "why",
        "wrong",
        "mark",
        // The state something is in, said the same way everywhere.
        "off",
        "away",
        "chosen",
        "sounding",
        "doing",
        "found",
        "exact",
        "narrow",
        "instead",
        // Furniture every screen has.
        "titled",
        "nothing",
        "finding",
        "search",
        "facts",
        "figures",
        "counts",
        "count",
        "named-figure",
        "bled",
        "two",
        "rail",
        "settings",
        "setting",
        "saving",
        "after",
        "option",
        "options",
        "forms",
        "pane",
        // A box that takes its own sideways scrolling rather than handing it to the
        // page. One rule, `overflow-x: auto`, and it means exactly that wherever it
        // is worn — the roster of accounts and the list of files that have gone
        // astray are both too wide to narrow any further and neither may push the
        // whole panel sideways.
        "scrolls",
        // The chrome of a drawer, which is one shape whichever thing it is about: the
        // body that scrolls, a name against a value, a note under what it explains,
        // the actions at the foot and the one of them that is the point, and the block
        // that says something is not where it was. Two of these panels agreeing about
        // them is the whole reason they read as one thing.
        "leafing",
        "spelt",
        "remark",
        "deeds",
        "leading",
        "absent",
        // A row whose file has gone: quiet rather than removed, in a listing and in a
        // running order alike.
        "gone",
        // One thing wherever it appears.
        "pill",
        "solid",
        "risky",
        "undoing",
        "link",
        "checkbox",
        "chevron",
        "art",
        "avatar",
        "badge",
        // A password typed once and never repeated, and the word that shows it:
        // the way in, and an administrator making somebody else's account. The
        // same shape and the same two words, because they are the same thing.
        "secret",
        "reveal",
        "menu",
        "menu-item",
        "dropdown",
        "tap",
        "veil",
        "scrim",
        "sheet",
        "sheet-lead",
        "sheet-body",
        "sheet-content",
        "sheet-foot",
    ];

    /// A class means one thing, and the ones that mean it everywhere are written down.
    ///
    /// The hole the rest of these tests cannot see. They compare the sheet against
    /// itself — two rules for one name, a selector written twice — and this is about a
    /// sheet perfectly consistent with itself and a screen broken anyway, because a word
    /// was already taken.
    ///
    /// `.reading` was it. The Overview has said `class="reading"` for the processor
    /// figure since it was written, styled through `.gauge .reading` because that is the
    /// only place the word means a reading; a panel elsewhere took `.reading` for its
    /// scrolling body and the Overview grew a horizontal scrollbar. `.acting` and
    /// `.named` had been leaking for longer — the first putting `margin-top` on a play
    /// glyph pinned with `inset: 0`, which is why that glyph sat below the centre of its
    /// own box through three attempts at finding out why.
    ///
    /// Two cleverer tests were tried first and both were wrong. "A rule that lays things
    /// out belongs to one screen" flagged twenty-four names, nearly all deliberate.
    /// "A scoped name does not get a bare rule later" flagged twenty more, because this
    /// sheet is ordered by screen rather than base-then-variant. Neither could work,
    /// and the reason is the same: they were trying to infer intent that nobody had
    /// written down.
    ///
    /// This asks one thing of a new class, once: is it a word the whole panel shares, or
    /// this screen's own? Both answers are cheap. The dishonest third answer — adding a
    /// word to [`SHARED`] to make the test quiet — hands the next person the collision
    /// this exists to prevent.
    #[test]
    fn a_class_worn_by_two_screens_is_one_the_panel_shares() {
        let styled = styled_classes();
        let mut taken = Vec::new();

        for class in styled {
            if SHARED.contains(&class) {
                continue;
            }

            let wearers: Vec<&str> = SOURCES
                .iter()
                .filter(|(_, source)| classes_in(source).contains(&class))
                .map(|(name, _)| *name)
                .collect();

            if wearers.len() > 1 {
                taken.push(format!(".{class} — {}", wearers.join(", ")));
            }
        }

        assert!(
            taken.is_empty(),
            "one word, two screens, and no note anywhere saying that was meant:\n  {}\n\
             Either it is the same thing in both places — then add it to SHARED, \
             deliberately — or the second one needs a word of its own.",
            taken.join("\n  ")
        );

        // And the list cannot rot into something that passes without meaning anything: a
        // word nothing shares any more is a word the next person will reuse thinking it
        // is free, which is exactly how this started.
        let mut idle: Vec<&str> = SHARED
            .iter()
            .copied()
            .filter(|word| {
                SOURCES
                    .iter()
                    .filter(|(_, source)| classes_in(source).contains(word))
                    .count()
                    < 2
            })
            .collect();
        idle.sort_unstable();

        assert!(
            idle.is_empty(),
            "on the shared list and shared by nobody: {}.\nTake it off, so the next \
             person reaching for the word finds it free or finds it taken, and not both.",
            idle.join(", ")
        );
    }

    /// Nothing but the panel's own root is built from the identity the session
    /// arrived with.
    ///
    /// That identity is a photograph: taken when the session was opened and never
    /// taken again, because taking it again means rebuilding everything under it and
    /// stopping the music on the way past. Every screen that wanted a name out of it
    /// therefore held the name you had when you logged in — and renaming an account
    /// is precisely the one thing an administrator may do to their own. One save
    /// later, Profile and Access and the roster were all asking the server about
    /// somebody who no longer exists, and the greeting was still saying the old name,
    /// which reads as a change that did not take.
    ///
    /// Both names live in [`layout::Me`] now, where they can change. This is what
    /// stops the photograph from being handed round again: the form takes it, the
    /// root reads out of it the parts that cannot change, and nothing downstream ever
    /// sees one.
    ///
    /// The word rather than a shape like `: Identity`, which is what this looked for
    /// first and which one of the screens it was written for would have walked
    /// straight past: the greeting's prop was spelt `identity: tocata::types::Identity`.
    #[test]
    fn only_the_root_is_built_from_the_identity() {
        /// Where it is allowed to be: the form that obtains one, and the root that
        /// unpacks it.
        const ITS_PLACE: [&str; 2] = ["main.rs", "login.rs"];

        let carried: Vec<&str> = SOURCES
            .iter()
            .filter(|(name, _)| !ITS_PLACE.contains(name))
            .filter(|(_, source)| source.contains("Identity"))
            .map(|(name, _)| *name)
            .collect();

        assert!(
            carried.is_empty(),
            "handed the identity the session arrived with: {}. It says who you were \
             when you logged in, and an administrator can rename themselves — take \
             the name from layout::Me, which follows.",
            carried.join(", ")
        );
    }

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
    /// Every measure is in `rem`, and every rule is in pixels.
    ///
    /// The panel is sized against whatever the reader told their browser to use,
    /// so somebody who set it to twenty because they cannot read fifteen gets a
    /// whole panel a quarter larger — text, air, glyphs and the sidebar with it —
    /// and somebody who set it to twelve gets one that is smaller. A measure left
    /// in pixels does not move with them, and one pixel of a row of otherwise
    /// scaling ones is how a layout stops adding up.
    ///
    /// The exceptions are not measures. A hairline is one physical pixel and has
    /// to stay one: `0.0625rem` is 1.25px for that reader at twenty, which a
    /// browser rounds differently from one edge to the next, and the whole look of
    /// this sheet is that its lines are all the same. A corner radius, a shadow
    /// and a focus offset are drawing rather than measuring, and none of them says
    /// anything about how much room the text needs.
    #[test]
    fn measures_are_in_rem_and_only_the_lines_are_not() {
        /// What is allowed to be, and has to be, in pixels.
        const DRAWN: [&str; 3] = ["border", "outline", "box-shadow"];

        /// The one rule allowed to measure in characters, and the one place where that
        /// is not a slip.
        ///
        /// `ch` was never caught here, because this only knew about `px`. It is not the
        /// accessibility fault `px` is — a `ch` scales with the reader's own size just as
        /// a `rem` does — but it does not line up with anything: it is a fraction of
        /// whatever typeface is in force, so a block sized in it agrees with no other
        /// width in the sheet and moves if the font ever changes. Four rules had picked
        /// one up, all four to hold prose to a reading measure, and nothing else in this
        /// panel is set to a reading measure at all — so each of them came out looking
        /// like a column that had failed to fill.
        ///
        /// The exception earns it. `.claim` is the one line of large type here and its
        /// size is fluid, `clamp(2rem, 4.4vw, 3.5rem)`; a cap in characters is what makes
        /// it break after the same words at every one of those sizes, which a cap in rem
        /// cannot do at any value.
        const IN_CHARACTERS: &str = ".claim";

        let mut wrong = Vec::new();

        for rule in rules() {
            let counting = rule.selectors.iter().all(|one| *one == IN_CHARACTERS);

            for (property, value) in declarations(rule.body) {
                let drawn = DRAWN.iter().any(|kind| property.starts_with(kind));

                // Percentages, viewport units and bare numbers are all fine; this
                // is only about the units that mean the same kind of thing.
                if !drawn && measured_in(&value, "px") {
                    wrong.push(format!("{property}: {value}"));
                }

                if drawn && measured_in(&value, "rem") {
                    wrong.push(format!("{property}: {value}"));
                }

                if !counting && measured_in(&value, "ch") {
                    wrong.push(format!("{property}: {value}"));
                }
            }
        }

        assert!(
            wrong.is_empty(),
            "a measure in the wrong unit: {}. Measures scale with the reader's own \
             font size and are written in rem; lines, corners and shadows are drawn \
             at a fixed size and are written in px; and a width in `ch` agrees with \
             nothing else in the sheet, so only {IN_CHARACTERS} has one.",
            wrong.join(", ")
        );
    }

    /// Whether a value carries a number in this unit.
    ///
    /// As a unit and not as a substring, which is what `contains` gave: `ch` is inside
    /// `stretch`, so `align-content: stretch` was reported as a width measured in
    /// characters. A unit is preceded by a digit and followed by nothing that could make
    /// it part of a longer word.
    fn measured_in(value: &str, unit: &str) -> bool {
        value.match_indices(unit).any(|(at, _)| {
            let before = value[..at].chars().next_back();
            let after = value[at + unit.len()..].chars().next();

            before.is_some_and(|c| c.is_ascii_digit() || c == '.')
                && after.is_none_or(|c| !c.is_ascii_alphanumeric())
        })
    }

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
    /// No selector is written twice, saying different things.
    ///
    /// The narrower cousin of the test below, and the one that catches what it
    /// cannot: two rules with the *same* selector, where there is no question of
    /// one deliberately narrowing the other and no list of blessed names to keep.
    /// Whichever is written later silently wins, and the earlier one reads like it
    /// is doing something.
    ///
    /// `.entry .lead` was written twice — thirty pixels under the lead of the
    /// login form, and eight left over from when that form had been a flex column
    /// with a gap. The eight came later in the file, so the thirty did nothing and
    /// the sheet still said it did.
    ///
    /// Only rules that name one selector. A rule naming several is a reset or a
    /// group — `p, h1, h2, dl, dd { margin: 0 }` — and giving one of them its own
    /// rule afterwards is how a stylesheet is written, not a mistake.
    #[test]
    fn no_selector_is_written_twice() {
        let mut seen: Vec<(&str, &str)> = Vec::new();

        for rule in rules() {
            if rule.inside_media || rule.selectors.len() != 1 {
                continue;
            }

            for selector in rule.selectors {
                if let Some((_, body)) = seen.iter().find(|(known, _)| *known == selector) {
                    assert_eq!(
                        strip_comments(body).split_whitespace().collect::<String>(),
                        strip_comments(rule.body)
                            .split_whitespace()
                            .collect::<String>(),
                        "{selector} is written twice and differently. The later one wins \
                         and the earlier one only looks like it is doing something: fold \
                         them into one rule."
                    );
                }

                seen.push((selector, rule.body));
            }
        }
    }

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
        let mut found: Vec<&'static str> = SOURCES
            .iter()
            .flat_map(|(_, source)| classes_in(source))
            .collect();

        found.sort_unstable();
        found.dedup();
        found
    }

    /// The same, for one file, and only from its markup.
    ///
    /// Comments are dropped first, which they were not before and had to be: the note
    /// over the shared-words test quotes `class="reading"` to explain what went wrong,
    /// and a scanner that reads prose counted the Overview and this very comment as two
    /// screens wearing the word. A test that its own explanation breaks is a test
    /// nobody will keep.
    fn classes_in(source: &'static str) -> Vec<&'static str> {
        /// Enough to cover the longest `class=` in the panel. It is bounded by the end
        /// of the line as well, and this is the belt to that pair of braces.
        const WINDOW: usize = 160;

        let mut found = Vec::new();
        let mut at = 0;

        while let Some(next) = source[at..].find("class") {
            let began = at + next;
            at = began + "class".len();

            if commented(source, began) {
                continue;
            }

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

            // To the end of the line and no further. It used to run on for a fixed
            // number of characters, which crossed into whatever followed — and what
            // follows a row of markup in this panel is usually a comment explaining
            // it. One stray quotation mark in that prose puts every pair after it out
            // of step, and then a word somebody wrote in a sentence is a class.
            let line = source[at..].find('\n').unwrap_or(source.len() - at);
            let mut rest = &source[at..at + line.min(WINDOW)];

            while let Some(open) = rest.find('"') {
                rest = &rest[open + 1..];
                let Some(close) = rest.find('"') else { break };
                let (quoted, after) = rest.split_at(close);
                rest = after;

                found.extend(quoted.split_whitespace());
            }
        }

        found.sort_unstable();
        found.dedup();
        found
    }

    /// Whether what is at `at` sits on a line that is a comment.
    fn commented(source: &str, at: usize) -> bool {
        let line = source[..at].rfind('\n').map_or(0, |break_at| break_at + 1);

        source[line..at].trim_start().starts_with("//")
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

        for (_, source) in SOURCES {
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
