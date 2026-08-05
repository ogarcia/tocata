# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

# The versions this build rests on, written down once. Here rather than in a file
# beside it because this is the only place the build itself can read, which is
# what lets the obvious thing work:
#
#   podman build -t tocata .
#
# Both name a minor line rather than a patch, so fixes arrive without a commit,
# the same bargain the dependencies are on.
ARG ALPINE_VERSION=3.24
ARG RUST_VERSION=1
# Trunk builds the panel. Pinned to a patch, unlike the two above, because it is
# fetched from a release by name and there is nothing to resolve a range against.
ARG TRUNK_VERSION=0.21.14

# Which of the two stages below the binary comes from. Declared out here because
# global scope is the only one a FROM can read.
ARG BINARY_SOURCE=builder

# --- the binary, compiled here -----------------------------------------------

FROM docker.io/library/rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS builder
ARG TRUNK_VERSION

# gcc and musl-dev come with the base image, without which rustc could not link
# at all, and they are also what libsqlite3-sys needs to compile SQLite itself.
# binaryen is for the panel: trunk would otherwise fetch a wasm-opt built against
# glibc, which on Alpine does not run.
RUN apk add --no-cache binaryen curl

# The musl build, because this image is one. Taken straight from the release and
# checked, rather than compiled here, which would add minutes for a tool that
# only shuffles files around.
RUN set -eux; \
    case "$(uname -m)" in \
        x86_64) arch=x86_64 ;; \
        aarch64) arch=aarch64 ;; \
        *) echo "no trunk build for $(uname -m)" >&2; exit 1 ;; \
    esac; \
    url="https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}"; \
    name="trunk-${arch}-unknown-linux-musl.tar.gz"; \
    curl -fsSL "${url}/${name}" -o /tmp/trunk.tar.gz; \
    curl -fsSL "${url}/${name}.sha256" -o /tmp/trunk.sha256; \
    echo "$(cut -d" " -f1 /tmp/trunk.sha256)  /tmp/trunk.tar.gz" | sha256sum -c; \
    tar -xzf /tmp/trunk.tar.gz -C /usr/local/bin trunk; \
    rm /tmp/trunk.tar.gz /tmp/trunk.sha256; \
    rustup target add wasm32-unknown-unknown

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY panel ./panel
COPY server ./server

# First, because the server embeds what this produces and will not compile
# without it. Both steps share the one target directory, which costs nothing:
# they build for different triples and land in different subdirectories of it.
# What trunk writes to panel/dist is outside the mount, so it is still there
# when the next step goes looking.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    set -eux; \
    cd panel; \
    trunk build --release

# No target is named: this image's own is a musl one, whichever architecture it
# runs on, and building for the host is what makes the same file work on x86_64
# and aarch64 alike.
#
# The caches are what make a second build quick. They also mean the binary is
# produced inside a mount that does not survive the step, hence copying it out
# before the shell exits.
#
# What ships is one file that brings its own libc, which the musl target gives
# for nothing and no flag here asks for. A dynamically linked executable names an
# interpreter to load it, so finding one would mean the ground had moved — a
# patched compiler, a different base image — and that is worth hearing about here
# rather than in production.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    set -eux; \
    cargo build --release --locked; \
    install -D target/release/tocata /out/tocata; \
    if readelf -l /out/tocata | grep -q INTERP; then \
        echo "tocata is not statically linked" >&2; \
        exit 1; \
    fi

# --- the binary, built somewhere better --------------------------------------

# CI compiles with a proper cargo cache and publishes the binary as an artifact;
# repeating that work inside a container image would be slower and would prove
# nothing. Point the build at what it already made, under dist/:
#
#   podman build --build-arg BINARY_SOURCE=prebuilt -t tocata .
#
# Only the stage that gets used is assembled, so dist/ need not exist for an
# ordinary build from source.
#
# The name carries the architecture in the build's own vocabulary rather than
# Rust's, which is what lets one multi-platform build pick each binary out of the
# same directory. Nothing is compiled along that path, so the only thing an
# emulator has to run is the last stage's handful of shell.
FROM scratch AS prebuilt
ARG TARGETARCH
COPY dist/tocata-${TARGETARCH} /out/tocata

# --- whichever of those two it was -------------------------------------------

# COPY --from takes no variables, so the choice cannot be made where the copying
# happens. A stage whose only job is to have a fixed name resolves it, and the
# one it does not name is never assembled.
FROM ${BINARY_SOURCE} AS binary

# --- the image that ships ----------------------------------------------------

FROM docker.io/library/alpine:${ALPINE_VERSION}

LABEL org.opencontainers.image.title="Tocata" \
      org.opencontainers.image.description="An OpenSubsonic compatible music server" \
      org.opencontainers.image.source="https://github.com/ogarcia/tocata" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"

# The mode is set rather than inherited because a binary that has travelled
# through a CI artifact arrives without its executable bit: zip does not carry
# permissions.
COPY --from=binary --chmod=0755 /out/tocata /usr/local/bin/tocata

# Never root. The id is 1000 because that is the first one a Linux desktop hands
# out, so a bind mounted media directory usually belongs to it already; anything
# else is what --user is for.
#
# /media rather than /music because what belongs in it is whatever this server can
# play, and a recorded talk or an audiobook is not music. Both mount points exist
# in the image so that a container started without them has somewhere to put them,
# and only /data changes hands: nothing here ever writes to the collection, so
# mounting /media read only is a thing that works rather than a thing that breaks.
RUN addgroup -g 1000 tocata \
 && adduser -D -H -G tocata -u 1000 tocata \
 && mkdir -p /data /media \
 && chown tocata:tocata /data

# No TOCATA_LIBRARY_PATHS, though it still works and still runs on every start.
# Naming /media here would look like a convenience while quietly deciding, for
# everybody, that the whole mount is one collection — and somebody who keeps their
# music beside their audiobooks wants two, with different people reaching each.
# What collections there are is the first thing the panel asks for, and the answer
# outlives a restart because it is a row rather than an environment variable.
ENV TOCATA_DATA_DIR=/data \
    TOCATA_PORT=4224

# No VOLUME for /data. It would look like protection for the database while
# actually creating an unnamed volume that goes away with the next container;
# where the state lives is the operator's decision to make and to name.
EXPOSE 4224

# No HEALTHCHECK either, though GET /api/health is what one would ask. How
# often to ask, how long to wait and what a failure should set in motion belong
# to whoever runs the container; baking one set of answers in decides them for
# every deployment from here.

USER tocata
ENTRYPOINT ["/usr/local/bin/tocata"]
