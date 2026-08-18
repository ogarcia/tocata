// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Reading a browser and a system out of what a browser says it is.
//!
//! A `User-Agent` is a sentence a program writes about itself, and every browser
//! writes it partly as another browser: Chrome claims to be Safari, Edge claims to
//! be Chrome claiming to be Safari, and Firefox on an iPhone is Safari underneath
//! and says so. Which is why the marks are looked for in a fixed order and the
//! first one found wins — the specific claim before the one it is dressed up as.
//!
//! Nothing here is a fact. It is a guess about a string that anybody can write
//! whatever they like into, and it is used for one thing: helping somebody
//! recognise which of their own open sessions is which. Nothing is decided by it,
//! so a lie in it costs nothing but a wrong word on a screen.
//!
//! Two names and never a version. "Firefox 126.0.1" on a line whose job is to be
//! recognised is three numbers that change on their own every few weeks, and the
//! browser somebody left open in the kitchen is not a different browser after it
//! updates itself.

/// The marks browsers leave, in the order they have to be looked for.
///
/// `Edg` and not `Edg/`, which is three of them at once: `Edg/` on a desktop,
/// `EdgA/` on Android, `EdgiOS/` on an iPhone. The same goes for the two Chromes
/// and the two Firefoxes — `CriOS` and `FxiOS` are what those two are called on
/// iOS, where the engine underneath them is Safari's by decree.
///
/// Brave is missing on purpose: it says it is Chrome, deliberately and to
/// everybody, so that is what it is read as. A browser that has gone to the
/// trouble of not being recognised is not one to go around identifying.
const BROWSERS: [(&str, &str); 11] = [
    ("SamsungBrowser/", "Samsung Internet"),
    ("Edg", "Edge"),
    ("OPR/", "Opera"),
    ("Opera", "Opera"),
    ("Vivaldi", "Vivaldi"),
    ("Chromium/", "Chromium"),
    ("CriOS/", "Chrome"),
    ("Chrome/", "Chrome"),
    ("FxiOS/", "Firefox"),
    ("Firefox/", "Firefox"),
    ("Safari/", "Safari"),
];

/// The same for the system under it, and the order matters here for one reason:
/// Android says Linux and ChromeOS says X11, because both of them are.
///
/// Said the way the people who make each of them say it, which is why one of these
/// is lowercase and none of the rest are.
const SYSTEMS: [(&str, &str); 10] = [
    ("Android", "Android"),
    ("CrOS", "ChromeOS"),
    ("iPhone", "iOS"),
    ("iPad", "iPadOS"),
    ("Macintosh", "macOS"),
    ("Windows", "Windows"),
    ("FreeBSD", "FreeBSD"),
    ("OpenBSD", "OpenBSD"),
    ("NetBSD", "NetBSD"),
    ("Linux", "Linux"),
];

/// How much of a `User-Agent` is worth keeping.
///
/// Real ones run to a couple of hundred characters. This is not a guard against
/// browsers, which have no reason to send more: it is a guard against the header
/// being a place where anything at all can be written into the database, once per
/// login, by whoever is logging in.
const AT_MOST: usize = 512;

/// What a browser called itself, as much of it as is worth writing down.
///
/// Blank is nothing rather than an empty string: a header that arrived empty says
/// no more than a header that never arrived, and only one of the two should be a
/// row saying something.
pub fn as_said(header: Option<&str>) -> Option<String> {
    let said: String = header?.trim().chars().take(AT_MOST).collect();

    (!said.is_empty()).then_some(said)
}

/// The browser and the system in it, either of which may be unreadable.
///
/// Read at the moment somebody asks rather than worked out when the session was
/// opened, which is what keeps the string itself in the row: this is a guess, and
/// a guess wants to be improvable without every row that was written under the old
/// one having to be found again.
pub fn read(said: &str) -> (Option<&'static str>, Option<&'static str>) {
    (found_in(said, &BROWSERS), found_in(said, &SYSTEMS))
}

/// The first mark of a list that appears in the sentence, and what it is called.
fn found_in(said: &str, marks: &[(&'static str, &'static str)]) -> Option<&'static str> {
    marks
        .iter()
        .find(|(mark, _)| said.contains(mark))
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real strings, because the whole of this is about what browsers actually
    /// write rather than about what they ought to.
    #[test]
    fn a_browser_is_read_as_itself_and_not_as_what_it_dresses_up_as() {
        let cases = [
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0",
                (Some("Firefox"), Some("Linux")),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like \
                 Gecko) Chrome/126.0.0.0 Safari/537.36",
                (Some("Chrome"), Some("Windows")),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like \
                 Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0",
                (Some("Edge"), Some("Windows")),
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, \
                 like Gecko) Version/17.4.1 Safari/605.1.15",
                (Some("Safari"), Some("macOS")),
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1",
                (Some("Safari"), Some("iOS")),
            ),
            (
                "Mozilla/5.0 (iPad; CPU OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, \
                 like Gecko) CriOS/126.0.0.0 Mobile/15E148 Safari/604.1",
                (Some("Chrome"), Some("iPadOS")),
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; SM-S911B) AppleWebKit/537.36 (KHTML, like \
                 Gecko) SamsungBrowser/25.0 Chrome/121.0.0.0 Mobile Safari/537.36",
                (Some("Samsung Internet"), Some("Android")),
            ),
            (
                "Mozilla/5.0 (Android 14; Mobile; rv:126.0) Gecko/126.0 Firefox/126.0",
                (Some("Firefox"), Some("Android")),
            ),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like \
                 Gecko) Chrome/126.0.0.0 Safari/537.36",
                (Some("Chrome"), Some("ChromeOS")),
            ),
            (
                "Mozilla/5.0 (X11; FreeBSD amd64; rv:126.0) Gecko/20100101 Firefox/126.0",
                (Some("Firefox"), Some("FreeBSD")),
            ),
        ];

        for (said, expected) in cases {
            assert_eq!(read(said), expected, "reading {said}");
        }
    }

    /// Whatever is not a browser is not read as one, and half a guess is still worth
    /// half a line: the screen says what it knows and leaves out what it does not.
    #[test]
    fn what_cannot_be_read_is_left_unsaid() {
        assert_eq!(read("curl/8.7.1"), (None, None));
        assert_eq!(read(""), (None, None));
        assert_eq!(read("Tocata/0.1.0 (Linux)"), (None, Some("Linux")));
        assert_eq!(read("Firefox/126.0"), (Some("Firefox"), None));
    }

    /// Nothing and blank are the same absence, and a header long enough to be a
    /// place to store things is cut down to a header.
    #[test]
    fn a_header_is_kept_only_as_far_as_it_says_something() {
        assert_eq!(as_said(None), None);
        assert_eq!(as_said(Some("   ")), None);
        assert_eq!(
            as_said(Some(" curl/8.7.1 ")),
            Some("curl/8.7.1".to_string())
        );

        let flood = "x".repeat(4096);
        assert_eq!(as_said(Some(&flood)).unwrap().chars().count(), AT_MOST);
    }
}
