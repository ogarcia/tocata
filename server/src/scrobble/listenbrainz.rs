// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How to say it in ListenBrainz.
//!
//! Everything protocol shaped lives here: the two paths, what a listen looks like
//! on the wire, and how to read what comes back. What is *not* here is which
//! services speak it or whose token is being used — that is [`super`], and the
//! split is what lets a second protocol arrive as a file beside this one rather
//! than as an `if` in the middle of the sender.
//!
//! Three services in the catalogue speak it, which is why one dialect covers
//! several destinations: a hosted ListenBrainz, and the self hosted scrobblers
//! that chose to answer the same calls under a path of their own.

use super::Listen;
use crate::net::Answer;
use serde::{Deserialize, Serialize};

/// How many listens go in one request.
///
/// The far end takes a thousand. This is fifty, because a backlog is drained by
/// repeating the loop and fifty is already twenty-five records: a request that
/// carried the whole thousand would, when it failed, have failed at all of them.
pub const AT_ONCE: usize = 50;

/// The header ListenBrainz answers a refusal with, in seconds. Preferred over the
/// absolute one it also sends, on their own advice: a client whose clock is wrong
/// reads the absolute one wrongly and the relative one exactly.
pub const RESET_IN: &str = "x-ratelimit-reset-in";

/// Where the listens go.
pub fn submitting(root: &str) -> String {
    format!("{}/1/submit-listens", root.trim_end_matches('/'))
}

/// Where a token is checked.
pub fn checking(root: &str) -> String {
    format!("{}/1/validate-token", root.trim_end_matches('/'))
}

/// What a service said about a token.
///
/// `valid` is what decides, and it arrives inside a 200: the call succeeded, and
/// the answer to the question it asked is no. Reading the status alone would take
/// a rejected token for a good one.
#[derive(Deserialize)]
pub struct Checked {
    pub valid: bool,
    /// What the account is called over there. Absent from a refusal, and absent
    /// from anything that is not quite this API.
    pub user_name: Option<String>,
}

/// Reads the answer to a token check, or nothing if it did not answer like this
/// API at all — which is not the same as a refusal, and the caller treats it
/// differently: a service that has no such call is not a service saying no.
pub fn read_check(answer: &Answer) -> Option<Checked> {
    serde_json::from_str(&answer.body).ok()
}

/// One listen as the wire wants it.
///
/// `listened_at` is missing for a now playing notification and present for
/// everything else, which is the whole difference between the two on this
/// protocol — that and the type beside it.
#[derive(Serialize)]
struct Entry<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    listened_at: Option<i64>,
    track_metadata: Metadata<'a>,
}

#[derive(Serialize)]
struct Metadata<'a> {
    artist_name: &'a str,
    track_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_name: Option<&'a str>,
    additional_info: Extra<'a>,
}

/// Everything optional, and every one of it there to keep a song from being
/// matched to a different recording of the same name. Omitted rather than sent
/// empty: a null in one of these is a claim that the tag was blank.
#[derive(Serialize)]
struct Extra<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    recording_mbid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_mbid: Option<&'a str>,
    /// An array on this protocol even when there is one of them, which there
    /// always is here: what a track credits is stored as a row per artist and
    /// what is queued is the one that leads the credit.
    #[serde(skip_serializing_if = "Option::is_none")]
    artist_mbids: Option<[&'a str; 1]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    isrc: Option<&'a str>,
    /// A string on this protocol, though it is a number everywhere else.
    #[serde(skip_serializing_if = "Option::is_none")]
    tracknumber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<i64>,
    /// What played it and what sent it. Both are Tocata here, and both are asked
    /// for: a service looking at a badly formed listen wants to know whose client
    /// made it, and a name with no version leaves them nothing to go on.
    media_player: &'static str,
    submission_client: &'static str,
    submission_client_version: &'static str,
}

#[derive(Serialize)]
struct Submission<'a> {
    listen_type: &'static str,
    payload: Vec<Entry<'a>>,
}

/// Tocata, as the far end will file it.
const PLAYER: &str = "Tocata";

impl<'a> Entry<'a> {
    fn of(listen: &'a Listen, when: Option<i64>) -> Self {
        Self {
            listened_at: when,
            track_metadata: Metadata {
                artist_name: &listen.artist,
                track_name: &listen.title,
                release_name: listen.album.as_deref(),
                additional_info: Extra {
                    recording_mbid: listen.mbid_recording.as_deref(),
                    release_mbid: listen.mbid_release.as_deref(),
                    artist_mbids: listen.mbid_artist.as_deref().map(|mbid| [mbid]),
                    isrc: listen.isrc.as_deref(),
                    tracknumber: listen.track_number.map(|number| number.to_string()),
                    duration_ms: listen.duration_ms,
                    media_player: PLAYER,
                    submission_client: PLAYER,
                    submission_client_version: env!("CARGO_PKG_VERSION"),
                },
            },
        }
    }
}

/// Listens that have been heard, as one request.
///
/// The type depends on how many there are, because the protocol says so: `single`
/// is defined as exactly one listen, and a batch of one sent as an `import` is
/// asking a service to accept something it documents as different.
pub fn submission(listens: &[Listen]) -> Result<String, serde_json::Error> {
    let payload: Vec<Entry> = listens
        .iter()
        .map(|listen| Entry::of(listen, Some(listen.at)))
        .collect();

    serde_json::to_string(&Submission {
        listen_type: if payload.len() == 1 {
            "single"
        } else {
            "import"
        },
        payload,
    })
}

/// What is sounding right now, which is a claim about the present and so carries
/// no time at all.
pub fn playing_now(listen: &Listen) -> Result<String, serde_json::Error> {
    serde_json::to_string(&Submission {
        listen_type: "playing_now",
        payload: vec![Entry::of(listen, None)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listen() -> Listen {
        Listen {
            at: 1_764_500_000,
            title: "Trains".into(),
            artist: "Porcupine Tree".into(),
            album: Some("In Absentia".into()),
            mbid_recording: Some("rec-1".into()),
            mbid_release: Some("rel-1".into()),
            mbid_artist: Some("art-1".into()),
            isrc: Some("GBAAA0000001".into()),
            track_number: Some(2),
            duration_ms: Some(351_000),
        }
    }

    /// The three field names the far end insists on, spelled its way rather than
    /// ours. Getting any of them wrong is a 400 nobody sees until a listen is
    /// already in the queue.
    #[test]
    fn a_listen_carries_the_names_the_protocol_uses() {
        let json = submission(&[listen()]).unwrap();

        assert!(json.contains(r#""listen_type":"single""#));
        assert!(json.contains(r#""listened_at":1764500000"#));
        assert!(json.contains(r#""artist_name":"Porcupine Tree""#));
        assert!(json.contains(r#""track_name":"Trains""#));
        assert!(json.contains(r#""release_name":"In Absentia""#));
        assert!(json.contains(r#""recording_mbid":"rec-1""#));
        assert!(json.contains(r#""artist_mbids":["art-1"]"#));
        assert!(json.contains(r#""isrc":"GBAAA0000001""#));
        assert!(json.contains(r#""duration_ms":351000"#));
        assert!(json.contains(r#""submission_client":"Tocata""#));
    }

    /// A number on this protocol and a string in the JSON. Sent as a number it is
    /// silently dropped, which is the worst of the three possible outcomes.
    #[test]
    fn the_track_number_goes_as_text() {
        let json = submission(&[listen()]).unwrap();

        assert!(json.contains(r#""tracknumber":"2""#), "{json}");
    }

    /// What was never tagged is left out rather than sent as null: a null claims
    /// the tag was there and empty.
    #[test]
    fn what_is_not_known_is_not_mentioned() {
        let bare = Listen {
            album: None,
            mbid_recording: None,
            mbid_release: None,
            mbid_artist: None,
            isrc: None,
            track_number: None,
            duration_ms: None,
            ..listen()
        };

        let json = submission(&[bare]).unwrap();

        assert!(!json.contains("release_name"));
        assert!(!json.contains("mbid"));
        assert!(!json.contains("isrc"));
        assert!(!json.contains("tracknumber"));
        assert!(!json.contains("duration"));
        assert!(json.contains(r#""artist_name":"Porcupine Tree""#));
    }

    /// One is `single` and several are an `import`, which the protocol defines as
    /// two different things.
    #[test]
    fn a_batch_is_an_import_and_one_is_not() {
        let one = submission(&[listen()]).unwrap();
        let two = submission(&[listen(), listen()]).unwrap();

        assert!(one.contains(r#""listen_type":"single""#));
        assert!(two.contains(r#""listen_type":"import""#));
    }

    /// The one difference between a listen and a now playing notification, beyond
    /// the type: it says nothing about when, because it means now.
    #[test]
    fn what_is_sounding_carries_no_time() {
        let json = playing_now(&listen()).unwrap();

        assert!(json.contains(r#""listen_type":"playing_now""#));
        assert!(!json.contains("listened_at"), "{json}");
    }

    /// Both paths, and neither doubling a slash for somebody who pasted their
    /// address with one on the end.
    #[test]
    fn a_trailing_slash_is_not_a_second_one() {
        assert_eq!(
            submitting("https://api.listenbrainz.org/"),
            "https://api.listenbrainz.org/1/submit-listens"
        );
        assert_eq!(
            checking("http://kitchen.lan:4533/apis/listenbrainz"),
            "http://kitchen.lan:4533/apis/listenbrainz/1/validate-token"
        );
    }

    /// A refused token arrives inside a perfectly successful call, so the status
    /// cannot be what decides.
    #[test]
    fn a_refusal_comes_wrapped_in_a_200() {
        let answer = Answer {
            status: 200,
            headers: hyper::HeaderMap::new(),
            body: r#"{"code":200,"message":"Token invalid.","valid":false}"#.into(),
        };

        let checked = read_check(&answer).unwrap();
        assert!(!checked.valid);
        assert_eq!(checked.user_name, None);
    }

    #[test]
    fn a_good_token_says_who_it_belongs_to() {
        let answer = Answer {
            status: 200,
            headers: hyper::HeaderMap::new(),
            body: r#"{"code":200,"message":"Token valid.","user_name":"ogarcia","valid":true}"#
                .into(),
        };

        let checked = read_check(&answer).unwrap();
        assert!(checked.valid);
        assert_eq!(checked.user_name.as_deref(), Some("ogarcia"));
    }

    /// Something that is not this API at all — an HTML error page from a proxy,
    /// say. Not a refusal: nothing said the token was bad.
    #[test]
    fn an_answer_that_is_not_this_api_is_not_a_refusal() {
        let answer = Answer {
            status: 404,
            headers: hyper::HeaderMap::new(),
            body: "<html><body>Not Found</body></html>".into(),
        };

        assert!(read_check(&answer).is_none());
    }
}
