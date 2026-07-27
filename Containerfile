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

# Which of the two stages below the binary comes from. Declared out here because
# global scope is the only one a FROM can read.
ARG BINARY_SOURCE=builder

# --- the binary, compiled here -----------------------------------------------

FROM docker.io/library/rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS builder

# Nothing to install: the base image already carries gcc and musl-dev, without
# which rustc could not link at all, and those are also what libsqlite3-sys needs
# to compile SQLite itself.

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# migrate!() reads this directory while compiling and embeds what it finds, which
# is also why the shipped image needs no copy of it.
COPY migrations ./migrations
COPY src ./src

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
# out, so a bind mounted music directory usually belongs to it already; anything
# else is what --user is for. Both mount points exist in the image so that a
# container started without them still has somewhere to look.
RUN addgroup -g 1000 tocata \
 && adduser -D -H -G tocata -u 1000 tocata \
 && mkdir -p /data /music \
 && chown tocata:tocata /data

ENV TOCATA_DATA_DIR=/data \
    TOCATA_LIBRARY_PATHS=/music \
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
