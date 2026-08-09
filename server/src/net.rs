// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The one place that talks to somebody else's server.
//!
//! Everything Tocata does otherwise happens between a request, a database and a
//! disk. This is the exception, and it is a narrow one: four calls, all of them
//! bounded in time and in how much they will read.
//!
//! Two of them carry a token and go where somebody typed — passing listens on.
//! Those never follow a redirect: a service answering "look over there" has been
//! configured wrongly, and following it would hand the token to a host nobody
//! named. The other two are questions put to public catalogues with nothing to
//! identify us, and one of those does follow redirects, because it fetches a
//! picture and being sent to where the file actually is *is* the answer. It can
//! afford to for the same reason it has to: there is no token on it to lose.
//!
//! Not reqwest, which does all of this and much more. What it adds over the
//! `hyper` axum already brings — a connection pool worth tuning, redirects,
//! cookies, response decoders — is nothing this needs, and it measured 1.1 MB
//! more in a statically linked binary. The price of doing without is this file.
//!
//! The certificate roots are compiled in rather than read from `/etc/ssl`, which
//! is why the image ships without ca-certificates. It also means a root that
//! expires is fixed by a release rather than by the base image, which is the
//! honest half of that bargain.

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use hyper::{Method, Request};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use std::time::Duration;

/// How long any one call may take, from opening the connection to the last byte.
///
/// Generous, because the far end may be a small machine waking a disk up, and
/// nothing here is waiting on the answer: a listen that takes twelve seconds to
/// be accepted is a listen accepted. What this is really for is the other case —
/// a host that accepts the connection and then says nothing at all, which
/// without a limit holds this task for as long as the process lives.
const PATIENCE: Duration = Duration::from_secs(15);

/// How much of an answer is read before giving up on it.
///
/// These are small JSON objects; anything approaching this is not the service we
/// think we are talking to. Without a limit, a URL somebody mistyped into a
/// firehose would be read into memory until there was none left.
const MOST: usize = 64 * 1024;

/// And how much of a picture is, which is a different order of thing.
///
/// Generous against what is actually asked for — a scaled copy a few hundred
/// kilobytes big — because the point is not to be tight but to have a bound at
/// all. What this stops is a URL that turns out to be a firehose, not a
/// photograph that came out larger than expected.
const MOST_OF_A_PICTURE: usize = 8 * 1024 * 1024;

/// How many times one fetch will be told to look somewhere else.
///
/// Commons answers with the file's own address on another host, which may
/// itself redirect. Four is more than that path has ever needed and few enough
/// that a pair of servers pointing at each other stops being our problem.
const HOPS: usize = 4;

/// What Tocata calls itself when it knocks on somebody's door.
///
/// The version is in it because a service with a broken client wants to know
/// which one, and a name with no version leaves them nothing to say.
///
/// And the address of the project, which is not decoration: MusicBrainz and
/// Wikimedia both ask in writing for a way to reach whoever is calling, and both
/// answer a bare name with 429. Measured — three artists in, Wikimedia refused
/// the download with exactly that, and this is what it wanted.
const CALLING: &str = concat!(
    "Tocata/",
    env!("CARGO_PKG_VERSION"),
    " ( ",
    env!("CARGO_PKG_REPOSITORY"),
    " )"
);

/// What came back: the status, the headers, and the body — however much of the
/// body was worth reading.
pub struct Answer {
    pub status: u16,
    /// Kept because a refusal often says how long to wait in one, and which one
    /// depends on whose server it is. Reading them belongs to whoever knows what
    /// they are talking to, not here.
    pub headers: hyper::HeaderMap,
    /// However much of it was worth reading. Bytes rather than text, because one
    /// of the things this fetches is a photograph.
    pub bytes: Vec<u8>,
}

impl Answer {
    /// Whether it was accepted. Everything else is for whoever asked to read,
    /// because what a refusal means differs by what was being asked for.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// What it said, as text. Lossy on purpose: a service answering a question
    /// about music with bytes that are not UTF-8 has already gone wrong, and the
    /// useful thing then is to be able to log what came back rather than to fail
    /// twice.
    pub fn body(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes)
    }

    /// A header read as a number of seconds, for the several ways a server has of
    /// saying "not yet". Anything that is not a plain count is nothing.
    pub fn seconds(&self, header: &str) -> Option<u64> {
        self.headers.get(header)?.to_str().ok()?.trim().parse().ok()
    }
}

/// A client for reaching other people's servers.
///
/// One of these exists for the whole process and is cheap to clone: what it
/// holds is a connection pool, and two of them would be two pools.
#[derive(Clone)]
pub struct Net {
    client: Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl Net {
    pub fn new() -> Self {
        // Not https_only. A scrobbling service somebody runs at home is reached
        // at http:// over their own network as often as not, and refusing that
        // would mean refusing the whole self hosted half of what this is for.
        // The panel says so where somebody types the address, because a token
        // sent in clear is worth knowing about even on a wire you own.
        let tls = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();

        Self {
            client: Client::builder(TokioExecutor::new()).build(tls),
        }
    }

    /// Asks a question, with a token.
    pub async fn get(&self, url: &str, token: &str) -> Result<Answer> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(url)
            .header(USER_AGENT, CALLING)
            .header(AUTHORIZATION, format!("Token {token}"))
            .body(Full::default())
            .context("building the request")?;

        self.send_reading(request, MOST).await
    }

    /// Asks a question of somebody who did not ask us to identify ourselves.
    ///
    /// No `Authorization`, which is the whole difference and the reason it is a
    /// call of its own rather than an `Option<&str>`: the services this reaches
    /// are public catalogues, and a header carrying a token somebody meant for
    /// their own scrobbler has no business on a request to one.
    pub async fn ask(&self, url: &str) -> Result<Answer> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(url)
            .header(USER_AGENT, CALLING)
            .body(Full::default())
            .context("building the request")?;

        self.send_reading(request, MOST).await
    }

    /// Fetches a picture, following the redirects that lead to it.
    ///
    /// The one call here that does follow them, and it can afford to for the
    /// same reason it has to: it carries no token, so there is nothing a host
    /// nobody named could be handed. What sends it somewhere else is Commons
    /// answering with the file's real address, which is how that catalogue is
    /// built rather than a sign of anything being wrong.
    ///
    /// Bytes rather than text, and not sniffed here: what an image is gets
    /// decided by [`crate::artwork::mime_of`], which reads the bytes rather than
    /// believing a header.
    ///
    /// A refusal comes back as the answer rather than as an error, like every
    /// other call here. What a status means is the caller's business: to this
    /// one 429 is a failure, and to whoever knows it is talking to a catalogue
    /// with a published rate limit it is "wait and ask again".
    pub async fn fetch(&self, url: &str) -> Result<Answer> {
        let mut url = url.to_string();

        for _ in 0..=HOPS {
            let request = Request::builder()
                .method(Method::GET)
                .uri(&url)
                .header(USER_AGENT, CALLING)
                .body(Full::default())
                .context("building the request")?;

            let answer = self.send_reading(request, MOST_OF_A_PICTURE).await?;

            // Only a redirect that says where. One that does not is an answer
            // with nothing in it, and there is nowhere to go from here.
            let elsewhere = matches!(answer.status, 301 | 302 | 303 | 307 | 308)
                .then(|| answer.headers.get(hyper::header::LOCATION))
                .flatten()
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);

            match elsewhere {
                Some(next) => url = next,
                None => return Ok(answer),
            }
        }

        bail!("it kept pointing somewhere else after {HOPS} hops")
    }

    /// Hands over some JSON, with a token.
    pub async fn post(&self, url: &str, token: &str, json: String) -> Result<Answer> {
        let request = Request::builder()
            .method(Method::POST)
            .uri(url)
            .header(USER_AGENT, CALLING)
            .header(AUTHORIZATION, format!("Token {token}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Full::from(Bytes::from(json)))
            .context("building the request")?;

        self.send_reading(request, MOST).await
    }

    /// The half they all share: send it, wait no longer than [`PATIENCE`],
    /// and read no more than [`MOST`].
    ///
    /// A redirect comes back as the redirect. Nothing here follows one, and only
    /// [`Net::fetch`] does anything about it: the calls that carry a token go to
    /// an address somebody typed, so a service answering one with "look over
    /// there" has been configured wrongly, and following it would hand that token
    /// to a host nobody named.
    async fn send_reading(&self, request: Request<Full<Bytes>>, most: usize) -> Result<Answer> {
        let response = tokio::time::timeout(PATIENCE, self.client.request(request))
            .await
            .map_err(|_| anyhow::anyhow!("it did not answer within {}s", PATIENCE.as_secs()))?
            .context("it could not be reached")?;

        let status = response.status().as_u16();
        let headers = response.headers().clone();

        let read = Limited::new(response.into_body(), most).collect().await;

        let body = match read {
            Ok(body) => body.to_bytes(),
            // Only worth failing over when the answer was going to be read. A
            // status that says everything there is to say — and every one of
            // these calls has one — is an answer already.
            Err(e) if !(200..300).contains(&status) => {
                bail!("it answered {status} and then failed to say why: {e}")
            }
            Err(e) => bail!("it answered {status} but the body could not be read: {e}"),
        };

        Ok(Answer {
            status,
            headers,
            bytes: body.to_vec(),
        })
    }
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}
