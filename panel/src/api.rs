// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Every call to `/api/v1`, in one place.
//!
//! The shapes come from the server's own crate, so a field that moves there
//! stops this compiling. What is left here is the plumbing: the cookie, the
//! method, and turning a failure into something a screen can show.

use gloo_net::http::{Request, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tocata::types::{
    Account, AccountChanges, Albums, Artists, Closed, Credentials, Genres, Identity, Key, Library,
    LibraryAccess, LibraryChanges, NewAccount, NewKey, NewLibrary, PreferenceChanges, Preferences,
    Revoked, Settings, SettingsChanges, Stats, Track, Tracks,
};
use web_sys::RequestCredentials;

/// Relative, because the panel is served by the server it talks to. Nothing to
/// configure and nothing to get wrong across deployments.
const BASE: &str = "/api/v1";

/// Where the event stream lives. Public because `EventSource` opens it itself
/// rather than going through anything here.
pub const EVENTS: &str = "/api/v1/events";

/// What went wrong, in the terms a screen cares about.
///
/// The distinction that matters is between "your session is gone" — which sends
/// the whole panel back to the login form — and anything else, which is a
/// message inside the screen that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 401: no session, or one that has run out.
    Unauthenticated,
    /// The server said no, and said why in a code we pass along untranslated.
    Refused(String),
    /// It did not answer at all.
    Unreachable,
}

/// The cookie is `HttpOnly`, so nothing here reads it; the browser sends it and
/// `SameOrigin` is what makes it do so.
///
/// Building can fail on a malformed URL, which would be our own mistake rather
/// than anything that happened on the network — but it arrives as the same kind
/// of failure, since a screen can do nothing different about it either way.
fn get(path: &str) -> Result<Request, Failure> {
    build(Request::get(&url(path)))
}

fn delete(path: &str) -> Result<Request, Failure> {
    build(Request::delete(&url(path)))
}

/// A POST with a body, which is the only reason a request here carries one.
fn post<T: Serialize>(path: &str, body: &T) -> Result<Request, Failure> {
    with_body(Request::post(&url(path)), body)
}

/// A PATCH, for the calls that change part of something.
fn patch<T: Serialize>(path: &str, body: &T) -> Result<Request, Failure> {
    with_body(Request::patch(&url(path)), body)
}

/// A PUT, for the one call that replaces a whole set rather than changing part
/// of one.
fn put<T: Serialize>(path: &str, body: &T) -> Result<Request, Failure> {
    with_body(Request::put(&url(path)), body)
}

fn with_body<T: Serialize>(request: RequestBuilder, body: &T) -> Result<Request, Failure> {
    request
        .credentials(RequestCredentials::SameOrigin)
        .json(body)
        .map_err(|_| Failure::Unreachable)
}

fn url(path: &str) -> String {
    format!("{BASE}{path}")
}

fn build(request: RequestBuilder) -> Result<Request, Failure> {
    request
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(|_| Failure::Unreachable)
}

/// Sends a request and reads the answer, mapping every way it can go wrong.
async fn read<T: DeserializeOwned>(request: Request) -> Result<T, Failure> {
    let response = request.send().await.map_err(|_| Failure::Unreachable)?;

    match response.status() {
        200..=299 => response.json().await.map_err(|_| Failure::Unreachable),
        401 => Err(Failure::Unauthenticated),
        _ => Err(refused(response).await),
    }
}

/// The server sends a stable code and an English message. The code is what we
/// keep: the panel says it in the reader's own language.
async fn refused(response: gloo_net::http::Response) -> Failure {
    let code = response
        .json::<tocata::types::ErrorBody>()
        .await
        .map(|body| body.code)
        .unwrap_or_else(|_| "unknown".to_string());

    Failure::Refused(code)
}

/// Who the cookie belongs to, if it belongs to anybody.
pub async fn whoami() -> Result<Identity, Failure> {
    read(get("/session")?).await
}

/// `remember` decides whether the browser keeps the way in after it closes. The
/// session on the server lasts as long either way.
pub async fn log_in(
    username: String,
    password: String,
    remember: bool,
) -> Result<Identity, Failure> {
    read(post(
        "/session",
        &Credentials {
            username,
            password,
            remember,
        },
    )?)
    .await
}

/// Ends this session. A failure here changes nothing worth telling: the panel is
/// going back to the login form either way.
pub async fn log_out() {
    if let Ok(request) = delete("/session") {
        let _ = request.send().await;
    }
}

/// Changes how the panel looks and speaks for whoever is logged in. Never somebody
/// else: these are not administered, so there is no username in the path.
pub async fn set_preferences(changes: PreferenceChanges) -> Result<Preferences, Failure> {
    read(patch("/preferences", &changes)?).await
}

pub async fn stats() -> Result<Stats, Failure> {
    read(get("/stats")?).await
}

/// How the server behaves for everybody. Anybody may read them; only an
/// administrator may write them.
pub async fn settings() -> Result<Settings, Failure> {
    read(get("/settings")?).await
}

/// Changes them. The screen sends every field at once, since it has one Save for
/// the lot, but the call itself has no opinion about that.
pub async fn set_settings(changes: SettingsChanges) -> Result<Settings, Failure> {
    read(patch("/settings", &changes)?).await
}

/// Starts one. `full` reads every file again instead of trusting size and
/// modification time.
pub async fn start_scan(full: bool) -> Result<(), Failure> {
    let path = if full { "/scan?full=true" } else { "/scan" };

    // Nothing worth reading comes back: what happens next arrives on the stream.
    match Request::post(&url(path))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(|_| Failure::Unreachable)?
        .send()
        .await
    {
        Ok(response) if response.ok() => Ok(()),
        Ok(response) if response.status() == 401 => Err(Failure::Unauthenticated),
        Ok(response) => Err(Failure::Refused(response.status().to_string())),
        Err(_) => Err(Failure::Unreachable),
    }
}

/// Every library, in the order the server lists them.
pub async fn libraries() -> Result<Vec<Library>, Failure> {
    read(get("/libraries")?).await
}

/// Adds one. The path has to exist on the server, which is the one thing this
/// cannot check from here.
pub async fn add_library(path: String, name: Option<String>) -> Result<Library, Failure> {
    read(post("/libraries", &NewLibrary { path, name })?).await
}

/// Renames one, switches it on or off, or both.
pub async fn change_library(id: i64, changes: LibraryChanges) -> Result<Library, Failure> {
    read(patch(&format!("/libraries/{id}"), &changes)?).await
}

/// Removes one, and with it everything scanned from it. The server refuses while
/// the library is still enabled, which arrives here as a `Refused`.
pub async fn remove_library(id: i64) -> Result<(), Failure> {
    plain(delete(&format!("/libraries/{id}"))?).await
}

/// Sends a request whose answer carries nothing worth reading, and maps the ways
/// it can go wrong the same way everything else does.
async fn plain(request: Request) -> Result<(), Failure> {
    match request.send().await {
        Ok(response) if response.ok() => Ok(()),
        Ok(response) if response.status() == 401 => Err(Failure::Unauthenticated),
        Ok(response) => Err(refused(response).await),
        Err(_) => Err(Failure::Unreachable),
    }
}

/// Every account. Only an administrator may ask.
pub async fn accounts() -> Result<Vec<Account>, Failure> {
    read(get("/users")?).await
}

/// One account. Yours, or anybody's if you administer the server.
pub async fn account(username: &str) -> Result<Account, Failure> {
    read(get(&format!("/users/{username}"))?).await
}

pub async fn add_account(new: NewAccount) -> Result<Account, Failure> {
    read(post("/users", &new)?).await
}

pub async fn change_account(username: &str, changes: AccountChanges) -> Result<Account, Failure> {
    read(patch(&format!("/users/{username}"), &changes)?).await
}

pub async fn remove_account(username: &str) -> Result<(), Failure> {
    plain(delete(&format!("/users/{username}"))?).await
}

/// Which libraries an account may see. An empty list means no restriction, which
/// is not the same as seeing nothing.
pub async fn restrict(username: &str, libraries: Vec<i64>) -> Result<Account, Failure> {
    read(put(
        &format!("/users/{username}/libraries"),
        &LibraryAccess { libraries },
    )?)
    .await
}

/// What only this account has, counted. Asked when something is about to say what
/// deleting it would cost.
pub async fn holdings(username: &str) -> Result<tocata::types::Holdings, Failure> {
    read(get(&format!("/users/{username}/holdings"))?).await
}

/// The keys an account holds, without the keys themselves.
pub async fn keys(username: &str) -> Result<Vec<Key>, Failure> {
    read(get(&format!("/users/{username}/keys"))?).await
}

/// Issues one. This is the only time the key itself can be read.
pub async fn issue_key(username: &str, new: NewKey) -> Result<tocata::types::IssuedKey, Failure> {
    read(post(&format!("/users/{username}/keys"), &new)?).await
}

/// Gives a key a new secret and keeps the rest. Readable once, like a new one.
pub async fn rotate_key(username: &str, id: i64) -> Result<tocata::types::IssuedKey, Failure> {
    read(post(&format!("/users/{username}/keys/{id}/rotate"), &())?).await
}

/// Withdraws a key. Whatever holds it stops working, and the key stays in the
/// listing, revoked, until it is removed.
pub async fn revoke_key(username: &str, id: i64) -> Result<Key, Failure> {
    read(post(&format!("/users/{username}/keys/{id}/revoke"), &())?).await
}

/// Takes a revoked or expired key out of the listing. The server refuses one that
/// still works, which is what makes the two steps two steps.
pub async fn remove_key(username: &str, id: i64) -> Result<(), Failure> {
    plain(delete(&format!("/users/{username}/keys/{id}"))?).await
}

/// Cuts an account off from every client holding a key. The rows stay, revoked.
pub async fn revoke_keys(username: &str) -> Result<Revoked, Failure> {
    read(post(&format!("/users/{username}/keys/revoke"), &())?).await
}

/// The panel logins an account has open.
pub async fn sessions(username: &str) -> Result<Vec<tocata::types::Login>, Failure> {
    read(get(&format!("/users/{username}/sessions"))?).await
}

pub async fn close_session(username: &str, id: i64) -> Result<(), Failure> {
    plain(delete(&format!("/users/{username}/sessions/{id}"))?).await
}

/// Closes every one of them except the one asking, so this never logs you out.
pub async fn close_sessions(username: &str) -> Result<Closed, Failure> {
    read(delete(&format!("/users/{username}/sessions"))?).await
}

/// Where your listens go, and every service they could go to. Not per username
/// like the calls above it: there is no administering somebody else's scrobbling,
/// so this one is always about whoever is asking.
pub async fn scrobblers() -> Result<tocata::types::Scrobbling, Failure> {
    read(get("/scrobblers")?).await
}

/// Sets one up, or replaces what was there. The server asks the service whether
/// the token is any good before storing it, so this is the call that can come back
/// `tokenRefused`.
pub async fn set_scrobbler(
    service: &str,
    new: tocata::types::NewScrobbler,
) -> Result<tocata::types::Scrobbler, Failure> {
    read(put(&format!("/scrobblers/{service}"), &new)?).await
}

/// Starts or stops sending, keeping the token and whatever is waiting.
pub async fn switch_scrobbler(
    service: &str,
    enabled: bool,
) -> Result<tocata::types::Scrobbler, Failure> {
    read(patch(
        &format!("/scrobblers/{service}"),
        &tocata::types::Switch { enabled },
    )?)
    .await
}

/// Forgets it, and drops what was waiting for it.
pub async fn remove_scrobbler(service: &str) -> Result<(), Failure> {
    plain(delete(&format!("/scrobblers/{service}"))?).await
}

/// Every maintenance job, what each would do right now, and what has been run
/// lately. One call, because the screen wants all of it at once.
pub async fn jobs() -> Result<tocata::types::Maintenance, Failure> {
    read(get("/jobs")?).await
}

/// Runs one and waits. A job that could not be done comes back as a run carrying
/// the reason rather than as a failure here.
pub async fn run_job(job: tocata::types::Job) -> Result<tocata::types::Run, Failure> {
    read(post(&format!("/jobs/{}", job.name()), &())?).await
}

/// What a purge would cost, in the things that cannot be scanned back. Asked by
/// the dialogue that stands in front of the one job that cannot be undone.
pub async fn loss() -> Result<tocata::types::Loss, Failure> {
    read(get("/purge")?).await
}

/// A window of the collection's tracks, narrowed by whatever has been typed.
///
/// The window is asked for rather than left to the server's own default, because
/// what an endless list needs is to carry on from exactly where it stopped: the
/// offset is how many rows are already on screen.
pub async fn tracks(search: &str, offset: usize, limit: i64) -> Result<Tracks, Failure> {
    read(get(&format!("/tracks?{}", window(search, offset, limit)))?).await
}

/// A window of the collection's albums, narrowed the same way.
pub async fn albums(search: &str, offset: usize, limit: i64) -> Result<Albums, Failure> {
    read(get(&format!("/albums?{}", window(search, offset, limit)))?).await
}

/// A window of the collection's artists, narrowed the same way.
pub async fn artists(search: &str, offset: usize, limit: i64) -> Result<Artists, Failure> {
    read(get(&format!("/artists?{}", window(search, offset, limit)))?).await
}

/// One track, by identifier.
///
/// What the player asks as it steps onto a track: a queue is identifiers, so this is
/// where it learns what to call the one now sounding.
pub async fn track(id: &str) -> Result<Track, Failure> {
    read(get(&format!("/tracks/{id}"))?).await
}

/// Everything the database holds about one track, which is what its own panel draws.
///
/// A second call and not a wider [`track`]: that one is what the player asks on every
/// change of song and wants five fields, and a listing of fifty rows must not pay for
/// the columns only a panel reads.
pub async fn detail(id: &str) -> Result<tocata::types::TrackDetail, Failure> {
    read(get(&format!("/tracks/{id}/detail"))?).await
}

/// Everything the database holds about one record: its figures, what it is, the
/// running order, and who played on it.
///
/// One call, because a panel about a record is one thing to read.
pub async fn album(id: &str) -> Result<tocata::types::AlbumDetail, Failure> {
    read(get(&format!("/albums/{id}/detail"))?).await
}

/// Everything the database holds about one artist: their figures, their records, and
/// what of theirs gets played.
pub async fn artist(id: &str) -> Result<tocata::types::ArtistDetail, Failure> {
    read(get(&format!("/artists/{id}/detail"))?).await
}

/// Every tag in a track's file, as the file spells them.
///
/// Read from disk on the server every time it is asked, which is why the panel asks
/// for it apart from everything else and only once there is a file to read: it is the
/// one call here that opens a file rather than a row.
pub async fn tags(id: &str) -> Result<tocata::types::Tags, Failure> {
    read(get(&format!("/tracks/{id}/tags"))?).await
}

/// A track's words, and which of the two places they were in.
pub async fn lyrics(id: &str) -> Result<tocata::types::Lyrics, Failure> {
    read(get(&format!("/tracks/{id}/lyrics"))?).await
}

/// Everything a filter matches, as identifiers, to be played.
///
/// `shuffle` draws the order before any limit is applied, so a shuffled few hundred
/// are a sample of the whole rather than the first few hundred in a jumble.
pub async fn queue(
    search: &str,
    album: Option<&str>,
    artist: Option<&str>,
    shuffle: bool,
    limit: Option<i64>,
) -> Result<Vec<String>, Failure> {
    let mut query = String::new();

    if !search.is_empty() {
        query.push_str(&format!(
            "search={}&",
            String::from(js_sys::encode_uri_component(search))
        ));
    }
    if let Some(album) = album {
        query.push_str(&format!("album={album}&"));
    }
    if let Some(artist) = artist {
        query.push_str(&format!("artist={artist}&"));
    }
    if shuffle {
        query.push_str("shuffle=true&");
    }
    if let Some(limit) = limit {
        query.push_str(&format!("limit={limit}"));
    }

    read::<tocata::types::Queue>(get(&format!("/tracks/ids?{query}"))?)
        .await
        .map(|queue| queue.tracks)
}

/// The rows for a named handful of tracks, in the order asked for.
///
/// What draws a queue. A queue holds identifiers, so showing it needs their titles in
/// one request rather than one per track — and the server answers in the listing's own
/// order, so the reordering back to the queue's order happens here.
pub async fn some_tracks(ids: &[String]) -> Result<Vec<Track>, Failure> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let named = ids.join(",");
    let asked = format!("/tracks?limit={}&ids={named}", ids.len());
    let page: Tracks = read(get(&asked)?).await?;

    // Back into the order they were named in. Anything the server left out — a track
    // removed by a scan since the queue was drawn — simply is not here.
    Ok(ids
        .iter()
        .filter_map(|id| page.tracks.iter().find(|track| &track.id == id).cloned())
        .collect())
}

/// Where a track's audio is, for an `<audio>` element to point at.
pub fn audio(track: &str) -> String {
    format!("{BASE}/tracks/{track}/audio")
}

/// Counts a play, once somebody has heard enough of it.
pub async fn count_play(id: &str) -> Result<(), Failure> {
    plain(
        Request::post(&url(&format!("/tracks/{id}/played")))
            .credentials(RequestCredentials::SameOrigin)
            .build()
            .map_err(|_| Failure::Unreachable)?,
    )
    .await
}

/// Says a track has started, which is the other half of [`count_play`] and a
/// different claim: it puts the panel in what is playing now, and gets the same
/// announcement passed on to wherever this account scrobbles.
pub async fn announce_play(id: &str) -> Result<(), Failure> {
    plain(
        Request::post(&url(&format!("/tracks/{id}/playing")))
            .credentials(RequestCredentials::SameOrigin)
            .build()
            .map_err(|_| Failure::Unreachable)?,
    )
    .await
}

/// A window of the collection's genres, narrowed the same way.
pub async fn genres(search: &str, offset: usize, limit: i64) -> Result<Genres, Failure> {
    read(get(&format!("/genres?{}", window(search, offset, limit)))?).await
}

/// Where an album's cover comes from, for an `<img>` to point at.
///
/// A URL rather than a fetch: the cookie is scoped to `/api` and the browser sends
/// it on its own for the `src` of an element from the same origin, so a grid of two
/// hundred covers is two hundred `<img>` tags and no credentials in any of them.
pub fn cover(album: &str) -> String {
    format!("{BASE}/albums/{album}/cover")
}

/// And where a picture of an artist is. Same arrangement as a cover, for the same
/// reason: an `<img>` from this origin carries the cookie on its own.
pub fn portrait(artist: &str) -> String {
    format!("{BASE}/artists/{artist}/image")
}

/// The query a listing is asked with.
///
/// The search is escaped because it is whatever somebody typed, and an ampersand
/// in a search would otherwise read as the start of the next parameter — which is
/// not an error anywhere, just a search for the wrong words.
///
/// Left out entirely when it is empty, rather than sent as nothing: the server
/// reads a missing search as "no search" and an empty one the same way, and this
/// keeps the two from having to agree about it.
fn window(search: &str, offset: usize, limit: i64) -> String {
    let mut query = format!("offset={offset}&limit={limit}");

    if !search.is_empty() {
        let escaped = String::from(js_sys::encode_uri_component(search));
        query.push_str(&format!("&search={escaped}"));
    }

    query
}

/// Asks the running scan to give up. What it had written is thrown away by the
/// server, so this is not a pause.
pub async fn cancel_scan() -> Result<(), Failure> {
    match delete("/scan")?.send().await {
        Ok(response) if response.ok() => Ok(()),
        Ok(response) if response.status() == 401 => Err(Failure::Unauthenticated),
        Ok(response) => Err(Failure::Refused(response.status().to_string())),
        Err(_) => Err(Failure::Unreachable),
    }
}
