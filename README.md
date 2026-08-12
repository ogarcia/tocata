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

Early days, and now being run in earnest to find out what is still wrong. There
are no releases yet, so what there is to run comes from the last build of the
`master` branch.

## Running it

### The container image

Published on every push to `master`:

```sh
podman run -d --name tocata \
  -p 4224:4224 \
  -v tocata-data:/data \
  -v /srv/music:/media:ro \
  ghcr.io/ogarcia/tocata:master
```

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

Every build of `master` uploads one per architecture. With the
[GitHub CLI](https://cli.github.com/), where the run has to be named because
`gh run download` takes a run and not a branch:

```sh
run=$(gh run list --repo ogarcia/tocata --branch master --workflow CI \
        --status success --limit 1 --json databaseId --jq '.[0].databaseId')
gh run download --repo ogarcia/tocata "$run" --name tocata-amd64
chmod +x tocata
```

`tocata-arm64` is the other one. They can also be downloaded from the summary page
of any run under the repository's Actions tab, which needs a GitHub session —
artifacts are not public downloads. The executable bit is set by hand above
because a zip file does not carry permissions.

Both are statically linked against musl and bring their own libc, so they run on
any Linux of the right architecture, distribution and version regardless.

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

Clients authenticate with a password or with an API key, which is made in the
panel. Token and salt authentication is refused with error 42 and cannot be
otherwise: verifying `md5(password + salt)` needs the password back in clear, and
what Tocata keeps is an Argon2id hash.

## What Tocata does not answer

Audio is the whole of what this server is for, and the OpenSubsonic API covers
more than audio. What follows is the part it does not do, and why.

`getOpenSubsonicExtensions` is the canonical answer to this question — a client
asks it rather than reading documentation — and Tocata declares four extensions
there: `apiKeyAuthentication`, `songLyrics`, `topSongsByArtistId` and
`playbackReport`.

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
| `getPlayQueueByIndex`, `savePlayQueueByIndex` | The `indexBasedQueue` extension, which exists to say which copy of a repeated track is playing. No known client asks for it, and `getPlayQueue` and `savePlayQueue` are both answered. |
