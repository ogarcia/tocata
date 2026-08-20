# Tocata

An [OpenSubsonic](https://opensubsonic.netlify.app/) compatible music server
written in Rust.

Tocata focuses on the server side of the API. There are already plenty of good
players that speak OpenSubsonic, so building another one is not the goal: the
only interface Tocata ships is a small administration panel.

What ships is a single binary with the panel inside it, statically linked and
bringing its own libc. There is nothing to install beside it, no runtime to provide
and no web server to put in front of it.

## Status

Early days, and now being run in earnest to find out what is still wrong. The
first release is 0.1.0.

Releases are what there is to run. `master` is where the work happens: it is
built and tested on every push and publishes an image of its own, so it is there
for anybody who wants to see what is coming rather than what is finished.

## Running it

### The container image

```sh
podman run -d --name tocata \
  -p 4224:4224 \
  -v tocata-data:/data \
  -v /srv/music:/media:ro \
  ghcr.io/ogarcia/tocata:latest
```

Three kinds of tag are published. `latest` is the newest release and moves when
one is cut; a version — `0.1.0` — names one release and never moves again, which
is the tag to write down somewhere that has to keep working; and `master` is the
development branch, rebuilt on every push to it.

`docker` in place of `podman` works the same way. Two directories matter: `/data`
is Tocata's own — the database lives there — and `/media` is where the music is.
Mounting the music read only is not a precaution against Tocata, which never
writes to a collection; it is simply true, so the filesystem may as well say so.

Nothing is scanned for having been mounted: `/media`, or a directory inside it, is
added as a collection in the panel afterwards. Which is deliberate — somebody who
keeps their music beside their audiobooks wants two collections with different
people reaching each, and one mount cannot say that.

The image runs as uid 1000 and never as root. A named volume for `/data`, as above,
is owned correctly from the start; a bind mount from the host arrives with the
host's ownership and has to be writable by that uid, or `--user` has to name
another.

The image can also be built from a checkout, which compiles everything inside it
and needs nothing installed but the container tool:

```sh
podman build -t tocata .
```

### A prebuilt binary

Every release carries one per architecture, with a `sha256sums.txt` beside them.
Nothing is needed to fetch one — no session, no tool but the ones already there:

```sh
version=0.1.0
base=https://github.com/ogarcia/tocata/releases/download/$version
curl -fLO "$base/tocata-$version-amd64.tar.gz"
curl -fLO "$base/sha256sums.txt"
sha256sum --check --ignore-missing sha256sums.txt
tar -xzf "tocata-$version-amd64.tar.gz"
```

`arm64` in place of `amd64` is the other one, and the
[releases page](https://github.com/ogarcia/tocata/releases) is the same thing to
click through. A tarball rather than a bare file because a tar carries the
executable bit, so what comes out runs.

Every build of `master` uploads the same two as run artifacts, for anybody
following the branch rather than the releases. Those need a GitHub session —
artifacts are not public downloads — and they travel as zips, which do not carry
permissions, so the executable bit has to be set by hand:

```sh
run=$(gh run list --repo ogarcia/tocata --branch master --workflow CI \
        --status success --limit 1 --json databaseId --jq '.[0].databaseId')
gh run download --repo ogarcia/tocata "$run" --name tocata-amd64
chmod +x tocata
```

All of them are statically linked against musl and bring their own libc, so they
run on any Linux of the right architecture, distribution and version regardless.

### From source

Rust, the WebAssembly target and [trunk](https://trunkrs.dev/) — which builds the
panel — are what it takes:

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
```

The panel is built first, because the server carries the result inside its own
binary and will not compile without it:

```sh
cd panel && trunk build --release && cd ..
cargo build --release
```

What comes out is `target/release/tocata`, linked against the system's libc. For
the static build the images and the artifacts ship, add the musl target and name
it — on a Debian or Ubuntu machine `musl-tools` is needed as well, since SQLite is
compiled from source and its compiler has to target musl too:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

## The first run

Tocata creates its data directory and its database on the way up, and makes one
administrator to get in with. That account's password is generated and written to
the log exactly once:

```
WARN tocata::user: initial password for 'admin': …
```

Then the panel is at <http://localhost:4224/>. Collections are added there, one row
each, and adding one reads nothing by itself: the scan is started from the panel's
first screen, which is also where it reports what it found while it runs.

## Configuration

Everything about the deployment is read from the environment. Everything about the
music — which collections there are, who may see them, how the catalogue reads —
belongs to the panel, because it outlives a restart and a container's command line
does not.

| Variable | Default | What it is |
| --- | --- | --- |
| `TOCATA_DATA_DIR` | `data` | Where the database and the caches live. The one directory Tocata writes to. |
| `TOCATA_PORT` | `4224` | Port to listen on. A port that is not a number stops the server rather than being ignored. |
| `TOCATA_LIBRARY_PATHS` | — | Collections to make sure exist, separated by colons as in `PATH`. Read on every start; it adds and enables, and never removes. |
| `TOCATA_IGNORED_ARTICLES` | `The El La Los Las Le Les` | Seeds the articles dropped when filing artists by letter. First run only — after that the panel owns it. |

## What answers where

| Path | |
| --- | --- |
| `/` | The administration panel |
| `/rest` | The OpenSubsonic API, which is what a player wants |
| `/api/v1` | The panel's own API |
| `/api/docs` | Reference for that API, generated from it |
| `/api/health` | Whether the server is up and its database answers |

## Authentication

There are two doors and they work differently, because a browser and a music
player are not in the same situation.

### The panel

A username — or the account's email address — and a password, exchanged for a
session. The session is a row in Tocata's own database: 256 bits from the system's
random number generator, kept as a SHA-256 digest, so what is stored is not the
token that opens it. The token travels in a cookie named `tocata_session`, scoped
to `/api`, `HttpOnly` and `SameSite=Strict`. `HttpOnly` keeps it out of reach of
the panel's own scripts, and so of anything injected into the page; it is also
what makes the event stream work, since `EventSource` cannot set a header and a
bearer token would have nowhere to go.

None of this needs a secret configured, because nothing is signed. A signed token
— a JWT — is a way of not looking anything up, and it earns its keep when the
lookup is the expense; here every request reaches SQLite anyway, and this
particular lookup is a unique index on a digest. What a signature would cost is
the part that matters: a token nobody can take back. The panel lists the sessions
an account has open and can close the rest, and changing a password closes them —
with a signed token that needs a list of the ones no longer welcome, which is
precisely the state the signature was avoiding. The secret itself is the other
cost: one more thing to generate, keep and hand to a container, where losing it
logs everybody out and leaking it lets anybody issue sessions.

How long a session lasts is a setting rather than an environment variable, thirty
days to begin with, and it is absolute rather than sliding: shortening it applies
to the next login and leaves the people already inside where they are.

Telling one session from another is what the browser said it was, written down as
it said it: a list of three rows all reading "another browser" is a list nobody
can act on. Which browser that sentence names — Firefox, on Linux — is read when
the list is asked for rather than when the session is opened, so the reading can
get better without any row having to be written again, and it is a guess either
way. The string is the client's own account of itself and nothing is decided by
it, so the worst a lie in it can do is put a wrong word on a screen. A session
can also be given a name, and that is the one thing said about it that cannot be
wrong: a browser can only say what it is, and only the person sitting at it knows
it is the laptop in the kitchen.

Keeping the login is what adds `Max-Age` to the cookie. Without it the browser
drops the cookie when it closes while the row keeps its own expiry — what has
been forgotten is the way back in, not the session. Logging out ends the row, and
only that one: the other browsers stay logged in.

### The API

A client under `/rest` proves who it is with a username and a password, `u` and
`p`, the latter either plain or as hex behind `enc:`, which the protocol allows
and which conceals nothing, being reversible by anybody.

Or with an API key, from the `apiKeyAuthentication` extension. Keys are made in
the panel, each with a label to tell it from the others, and each can be withdrawn
on its own without disturbing the rest. The extension is explicit that a key
travels alone, so a request carrying `apiKey` alongside any of `u`, `p`, `t` or
`s` is refused with error 43.

A key is also accepted where a password goes, because a client's login screen has
one box for a password and usually no field for anything else — so a key pasted
into it works. Only the key belonging to the account named in `u`, though:
somebody else's opens nothing, and rather than being refused outright it falls
through to the password check, since what was offered may still be this account's
password. Keys are for players and not for the panel, which takes a password.

### Token and salt, and why not

The mechanism the specification calls token authentication — `s` and
`t=md5(password + salt)` — is refused with error 42, "provided authentication
mechanism not supported", and cannot be anything else.

Verifying that token means computing `md5(password + salt)` here, which means
having the password to hand: in clear, or encrypted beside the key that decrypts
it, which is the same thing the day somebody copies the database. What Tocata
keeps is an Argon2id hash, and the whole purpose of one is that the password
cannot be got back out of it.

So this is not a feature nobody has written yet. It is a mechanism that cannot
coexist with storing passwords properly, and between the two the passwords win.
Error 42 is the specification's own way of saying a server does not offer a
mechanism, and every client we have tried has a setting for the other way —
worded as plain, clear text or legacy password authentication, depending on the
client — which is what Tocata answers.

## What Tocata does not answer

Audio is the whole of what this server is for, and the OpenSubsonic API covers
more than audio. What follows is the part it does not do, and why.

`getOpenSubsonicExtensions` is the canonical answer to this question — a client
asks it rather than reading documentation — and Tocata declares five extensions
there: `apiKeyAuthentication`, `songLyrics`, `topSongsByArtistId`,
`playbackReport` and `indexBasedQueue`.

### Answered in the protocol, with nothing behind them

These are registered, because an HTTP 404 does not tell a client that a server
lacks a feature from one that is broken or behind a misconfigured proxy. A listing
comes back empty, and anything naming one particular thing comes back as error 70.

| Endpoints | Why |
| --- | --- |
| `getVideos`, `getVideoInfo`, `getCaptions`, `hls.m3u8` | Tocata plays audio. Video is another program's job. |
| `getSimilarSongs`, `getSimilarSongs2` | Deciding that two songs are alike takes data this server does not have. Guessing from genres would be inventing recommendations and calling them knowledge. |
| `getShares` | Sharing means serving audio to somebody who has not logged in, and a database to remember what was shared with whom. It is a large piece of work whose subject is access, not audio. |
| `getPodcasts` | A podcast is a feed to fetch and files to download, not the collection somebody chose. There is no shortage of applications that do it well. |
| `getInternetRadioStations` | The same: a station is a URL somebody else is serving, and players for those are their own thing. |
| `getChatMessages`, `addChatMessage` | A music server is not a chat room. |
| `getAvatar` | There are no avatars anywhere in Tocata to serve. |

### Not answered at all

| Endpoints | Why |
| --- | --- |
| `createShare`, `updateShare`, `deleteShare` | As above: access rather than audio, and a large change to earn it. |
| `createPodcastChannel`, `deletePodcastChannel`, `deletePodcastEpisode`, `downloadPodcastEpisode`, `getNewestPodcasts`, `getPodcastEpisode`, `refreshPodcasts` | Fetching and downloading somebody else's feed. |
| `createInternetRadioStation`, `updateInternetRadioStation`, `deleteInternetRadioStation` | Keeping a list of other people's stream URLs. |
| `jukeboxControl` | Playing music out of the speakers attached to the server. That needs an audio device and a decoder inside a binary that is meant to have neither. |
| `getTranscodeDecision`, `getTranscodeStream` | Tocata serves files as they are on disk. There is no transcoder in it. |
| `findSonicPath`, `getSonicSimilarTracks` | Analysing what the music sounds like, which is a field of its own. |
