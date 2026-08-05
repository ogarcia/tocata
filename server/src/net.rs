// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The one place that talks to somebody else's server.
//!
//! Everything Tocata does otherwise happens between a request, a database and a
//! disk. This is the exception, and it is a narrow one: a GET and a POST that
//! carry a token and come back with a status and some text. Both are small,
//! both time out, and neither follows a redirect.
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

/// What Tocata calls itself when it knocks on somebody's door.
///
/// The version is in it because a service with a broken client wants to know
/// which one, and a name with no version leaves them nothing to say.
const CALLING: &str = concat!("Tocata/", env!("CARGO_PKG_VERSION"));

/// What came back: the status, the headers, and the body — however much of the
/// body was worth reading.
pub struct Answer {
    pub status: u16,
    /// Kept because a refusal often says how long to wait in one, and which one
    /// depends on whose server it is. Reading them belongs to whoever knows what
    /// they are talking to, not here.
    pub headers: hyper::HeaderMap,
    pub body: String,
}

impl Answer {
    /// Whether it was accepted. Everything else is for whoever asked to read,
    /// because what a refusal means differs by what was being asked for.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
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

        self.send(request).await
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

        self.send(request).await
    }

    /// The half both of them share: send it, wait no longer than [`PATIENCE`],
    /// and read no more than [`MOST`].
    ///
    /// A redirect comes back as the redirect. Nothing here follows one, because
    /// the two calls this makes are both to an address somebody typed: a service
    /// that answers a POST with "look over there" is a service that has been
    /// configured wrongly, and following it would send a token to a host nobody
    /// named.
    async fn send(&self, request: Request<Full<Bytes>>) -> Result<Answer> {
        let response = tokio::time::timeout(PATIENCE, self.client.request(request))
            .await
            .map_err(|_| anyhow::anyhow!("it did not answer within {}s", PATIENCE.as_secs()))?
            .context("it could not be reached")?;

        let status = response.status().as_u16();
        let headers = response.headers().clone();

        let read = Limited::new(response.into_body(), MOST).collect().await;

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
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}
