-- SPDX-License-Identifier: GPL-3.0-or-later
-- Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

-- Two independent blocks live in this schema:
--
--   * The catalogue, which is a projection of what is on disk. Dropping it
--     and rescanning loses nothing.
--   * User data, which cannot be recovered from anything. The scanner never
--     writes to these tables.
--
-- Timestamps are ISO-8601 UTC strings so they stay readable in a sqlite3
-- shell. Booleans are integers constrained to 0/1.
--
-- Every entity exposed through the API carries a `public_id`: an opaque
-- string minted once and stored. Clients cache these ids in their own
-- favourites and playlists, so they must survive a file being renamed or a
-- tag being corrected. Deriving them from the content would break that.

-- ---------------------------------------------------------------------------
-- Catalogue
-- ---------------------------------------------------------------------------

-- Configured library roots. The API exposes these as music folders with
-- integer ids, so here the internal key doubles as the public one.
CREATE TABLE libraries (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    path        TEXT    NOT NULL UNIQUE,
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);

-- One table for album covers and artist images alike, so getCoverArt works
-- over a single id space and the on-disk cache is uniform. Binaries never go
-- in the database: content_hash names the cached file.
--
-- `source` matters for policy: a local file must never be overwritten by
-- something fetched from the network, and only remote entries expire.
CREATE TABLE artworks (
    id           INTEGER PRIMARY KEY,
    public_id    TEXT    NOT NULL UNIQUE,
    kind         TEXT    NOT NULL,
    source       TEXT    NOT NULL,
    source_ref   TEXT,
    mime_type    TEXT    NOT NULL,
    width        INTEGER,
    height       INTEGER,
    content_hash TEXT    NOT NULL,
    fetched_at   TEXT    NOT NULL
);

CREATE INDEX artworks_hash_idx ON artworks (content_hash);

-- Filesystem tree, backing getIndexes and getMusicDirectory.
--
-- The self-referencing parent_id means rows must be inserted parent first.
-- The scanner walks breadth first, which satisfies that naturally; there is
-- no need to sort by path length as a proxy for depth.
-- Marked, never deleted, for the same reason as tracks: deleting a folder
-- cascades into its tracks and takes the user's data with them.
-- `path` is relative to the library's root, for the reason spelled out over
-- `tracks` below.
CREATE TABLE folders (
    id            INTEGER PRIMARY KEY,
    public_id     TEXT    NOT NULL UNIQUE,
    library_id    INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    parent_id     INTEGER          REFERENCES folders (id)   ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    path          TEXT    NOT NULL,
    modified_at   TEXT,
    missing_since TEXT,
    last_seen_scan INTEGER NOT NULL,
    UNIQUE (library_id, path)
);

CREATE INDEX folders_parent_idx  ON folders (parent_id);
CREATE INDEX folders_library_idx ON folders (library_id);

CREATE TABLE artists (
    id          INTEGER PRIMARY KEY,
    public_id   TEXT    NOT NULL UNIQUE,
    name        TEXT    NOT NULL,
    -- Drives alphabetical grouping in getIndexes and getArtists, where "The
    -- Beatles" must file under B.
    sort_name   TEXT,
    mbid        TEXT,
    biography   TEXT,
    artwork_id  INTEGER REFERENCES artworks (id) ON DELETE SET NULL,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);

CREATE UNIQUE INDEX artists_mbid_idx ON artists (mbid) WHERE mbid IS NOT NULL;
CREATE INDEX artists_name_idx        ON artists (name);
CREATE INDEX artists_sort_name_idx   ON artists (sort_name);

-- An album is NOT identified by (artist, name): that breaks compilations,
-- where the artist differs on every track. Grouping goes by MusicBrainz
-- release id when present, and falls back to (album artist, name, year).
CREATE TABLE albums (
    id                 INTEGER PRIMARY KEY,
    public_id          TEXT    NOT NULL UNIQUE,
    name               TEXT    NOT NULL,
    sort_name          TEXT,
    year               INTEGER,
    release_date       TEXT,
    original_date      TEXT,
    is_compilation     INTEGER NOT NULL DEFAULT 0
                       CHECK (is_compilation IN (0, 1)),
    mbid_release       TEXT,
    mbid_release_group TEXT,
    rg_album_gain      REAL,
    rg_album_peak      REAL,
    artwork_id         INTEGER REFERENCES artworks (id) ON DELETE SET NULL,
    created_at         TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL
);

CREATE INDEX albums_name_idx      ON albums (name);
CREATE INDEX albums_sort_name_idx ON albums (sort_name);
CREATE INDEX albums_mbid_idx      ON albums (mbid_release);

-- Per-disc titles, for the discTitles field OpenSubsonic adds to AlbumID3.
CREATE TABLE album_discs (
    album_id    INTEGER NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
    disc_number INTEGER NOT NULL,
    title       TEXT,
    PRIMARY KEY (album_id, disc_number)
) WITHOUT ROWID;

-- A track whose file is gone is marked, never deleted. Every reference to a
-- track that matters to the user cascades: playlist entries, favourites,
-- ratings, play counts, bookmarks. An unmounted NAS or a disk that failed to
-- mount would otherwise empty every playlist on the next scan, silently and
-- irreversibly.
--
-- So the scanner sets `missing_since` on tracks it did not see, clears it if
-- the file comes back, and never issues a DELETE. Purging is an explicit
-- administrative action.
--
-- This doubles as the reconciliation mechanism the opaque public ids need: a
-- file that shows up at a new path is matched against missing rows by
-- MusicBrainz id, or by size, duration and title, and is then the same track
-- with its public_id and all its user data intact. No hashing: reading every
-- byte of a library to catch a case that size and mtime already catch is not
-- a trade worth making.
--
-- `path` is relative to the library's own directory, and so is folders.path
-- above. Absolute, every row would name the place the library happened to be,
-- and moving it — a different mount point, a new disk, /music instead of
-- /srv/music — would leave every one of them wrong, to be reconciled one file
-- at a time by that heuristic above. Relative, the root is named once, in one
-- row of `libraries`, and moving a library is changing that one row. The
-- reconciliation stays for what it is actually for: a file moved within its
-- library.
CREATE TABLE tracks (
    id               INTEGER PRIMARY KEY,
    public_id        TEXT    NOT NULL UNIQUE,
    library_id       INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    folder_id        INTEGER NOT NULL REFERENCES folders (id)   ON DELETE CASCADE,
    album_id         INTEGER          REFERENCES albums (id)    ON DELETE SET NULL,
    path             TEXT    NOT NULL,
    file_size        INTEGER NOT NULL,
    file_modified_at TEXT    NOT NULL,
    content_type     TEXT    NOT NULL,
    suffix           TEXT    NOT NULL,
    title            TEXT    NOT NULL,
    sort_title       TEXT,
    track_number     INTEGER,
    disc_number      INTEGER,
    year             INTEGER,
    duration_ms      INTEGER,
    bit_rate         INTEGER,
    bit_depth        INTEGER,
    sampling_rate    INTEGER,
    channel_count    INTEGER,
    bpm              INTEGER,
    comment          TEXT,
    explicit_status  TEXT,
    mbid_recording   TEXT,
    mbid_track       TEXT,
    isrc             TEXT,
    rg_track_gain    REAL,
    rg_track_peak    REAL,
    missing_since    TEXT,
    -- Which scan last saw this file, stamped on every pass whether or not the
    -- file changed. What a scan did not touch is what has gone away, which is
    -- one statement at the end instead of a set of paths held in memory.
    --
    -- A scan number rather than a timestamp on purpose: timestamps have finite
    -- granularity, so two scans within the same tick would compare equal and
    -- the sweep would find nothing. A counter is exact, and it does not care
    -- what NTP does to the clock mid-scan.
    last_seen_scan   INTEGER NOT NULL,
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL,
    UNIQUE (library_id, path)
);

CREATE INDEX tracks_album_idx  ON tracks (album_id);
CREATE INDEX tracks_folder_idx ON tracks (folder_id);
CREATE INDEX tracks_title_idx  ON tracks (title);
CREATE INDEX tracks_mbid_idx   ON tracks (mbid_recording);

-- Feeds both the missing files view in the panel and the reconciliation of
-- moved files. Partial, because the expected number of missing tracks is zero.
CREATE INDEX tracks_missing_idx ON tracks (missing_since)
    WHERE missing_since IS NOT NULL;

-- Drives the sweep that marks what a scan did not see.
CREATE INDEX tracks_last_seen_idx ON tracks (library_id, last_seen_scan);

-- Credits carry a role and a position. The role covers artist, albumartist,
-- composer, performer and whatever else a tag throws at us without adding a
-- table per role; the position preserves credit order, so "A feat. B" can be
-- rebuilt exactly as tagged.
CREATE TABLE track_artists (
    track_id  INTEGER NOT NULL REFERENCES tracks (id)  ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    role      TEXT    NOT NULL,
    position  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, artist_id, role)
) WITHOUT ROWID;

CREATE INDEX track_artists_artist_idx ON track_artists (artist_id);

CREATE TABLE album_artists (
    album_id  INTEGER NOT NULL REFERENCES albums (id)  ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    role      TEXT    NOT NULL,
    position  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (album_id, artist_id, role)
) WITHOUT ROWID;

CREATE INDEX album_artists_artist_idx ON album_artists (artist_id);

CREATE TABLE genres (
    id   INTEGER PRIMARY KEY,
    name TEXT    NOT NULL UNIQUE
);

CREATE TABLE track_genres (
    track_id INTEGER NOT NULL REFERENCES tracks (id)  ON DELETE CASCADE,
    genre_id INTEGER NOT NULL REFERENCES genres (id)  ON DELETE CASCADE,
    PRIMARY KEY (track_id, genre_id)
) WITHOUT ROWID;

CREATE INDEX track_genres_genre_idx ON track_genres (genre_id);

CREATE TABLE album_genres (
    album_id INTEGER NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
    genre_id INTEGER NOT NULL REFERENCES genres (id) ON DELETE CASCADE,
    PRIMARY KEY (album_id, genre_id)
) WITHOUT ROWID;

CREATE INDEX album_genres_genre_idx ON album_genres (genre_id);

CREATE TABLE moods (
    id   INTEGER PRIMARY KEY,
    name TEXT    NOT NULL UNIQUE
);

CREATE TABLE track_moods (
    track_id INTEGER NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    mood_id  INTEGER NOT NULL REFERENCES moods (id)  ON DELETE CASCADE,
    PRIMARY KEY (track_id, mood_id)
) WITHOUT ROWID;


-- ---------------------------------------------------------------------------
-- Artwork lookups
-- ---------------------------------------------------------------------------

-- Negative cache for artwork lookups. Most artists have no picture anywhere,
-- and without this the server would hammer the same remote sources on every
-- scan for entities that will never resolve.
CREATE TABLE artwork_lookups (
    entity_type  TEXT    NOT NULL,
    entity_id    INTEGER NOT NULL,
    source       TEXT    NOT NULL,
    attempted_at TEXT    NOT NULL,
    found        INTEGER NOT NULL CHECK (found IN (0, 1)),
    PRIMARY KEY (entity_type, entity_id, source)
) WITHOUT ROWID;

-- ---------------------------------------------------------------------------
-- Users and authentication
-- ---------------------------------------------------------------------------

CREATE TABLE users (
    id                 INTEGER PRIMARY KEY,
    username           TEXT    NOT NULL UNIQUE,
    -- Argon2id. Nothing in this schema needs the plaintext back.
    password_hash      TEXT    NOT NULL,
    email              TEXT,
    is_admin           INTEGER NOT NULL DEFAULT 0 CHECK (is_admin IN (0, 1)),
    scrobbling_enabled INTEGER NOT NULL DEFAULT 1
                       CHECK (scrobbling_enabled IN (0, 1)),
    created_at         TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL
);

-- Revocable per client, which is what makes it possible to hash the password
-- instead of encrypting it: the legacy token scheme needs the plaintext, an
-- API key does not.
--
-- Expiry is opt in, which is why the column is nullable and null is the default.
-- A key is held by a music player, and OpenSubsonic gives a player no way to
-- renew anything, so a key that expires stops the music on a date nothing
-- announced. That is a fine thing to accept deliberately — a key for trying a
-- client out, or one lent to somebody — and a poor thing to be handed.
--
-- An expired key is kept. It cannot authenticate, but the row stays until it is
-- revoked, so the date can be pushed out and the same key start working again
-- rather than every client having to be set up afresh.
CREATE TABLE api_keys (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key_hash     TEXT    NOT NULL UNIQUE,
    label        TEXT    NOT NULL,
    created_at   TEXT    NOT NULL,
    -- Null means it never expires. Stored in the same shape as every other
    -- timestamp here, because it is compared as text against one of them.
    expires_at   TEXT,
    last_used_at TEXT
);

CREATE INDEX api_keys_user_idx ON api_keys (user_id);

-- A panel login. The token is hashed for the same reason an API key is: telling
-- one apart never needs the plaintext, so a stolen database hands over no live
-- session.
--
-- Expiry is absolute rather than a sliding window. A window that moves on every
-- request never closes for anybody who leaves a tab open, and the tab is exactly
-- the thing worth closing.
CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   TEXT    NOT NULL UNIQUE,
    created_at   TEXT    NOT NULL,
    -- Written back only when it is already stale, so following the panel around
    -- does not mean a write per request.
    last_seen_at TEXT    NOT NULL,
    expires_at   TEXT    NOT NULL
);

CREATE INDEX sessions_user_idx ON sessions (user_id);

-- Restricts a user to a subset of libraries. Absence of rows means full
-- access, so the common case costs nothing.
CREATE TABLE user_libraries (
    user_id    INTEGER NOT NULL REFERENCES users (id)     ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, library_id)
) WITHOUT ROWID;

-- ---------------------------------------------------------------------------
-- User data
-- ---------------------------------------------------------------------------

CREATE TABLE user_track_stats (
    user_id     INTEGER NOT NULL REFERENCES users (id)  ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    play_count  INTEGER NOT NULL DEFAULT 0,
    last_played TEXT,
    rating      INTEGER CHECK (rating BETWEEN 1 AND 5),
    starred_at  TEXT,
    PRIMARY KEY (user_id, track_id)
) WITHOUT ROWID;

CREATE INDEX user_track_stats_starred_idx
    ON user_track_stats (user_id, starred_at)
    WHERE starred_at IS NOT NULL;

CREATE TABLE user_album_stats (
    user_id     INTEGER NOT NULL REFERENCES users (id)  ON DELETE CASCADE,
    album_id    INTEGER NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
    play_count  INTEGER NOT NULL DEFAULT 0,
    last_played TEXT,
    rating      INTEGER CHECK (rating BETWEEN 1 AND 5),
    starred_at  TEXT,
    PRIMARY KEY (user_id, album_id)
) WITHOUT ROWID;

CREATE INDEX user_album_stats_starred_idx
    ON user_album_stats (user_id, starred_at)
    WHERE starred_at IS NOT NULL;

CREATE TABLE user_artist_stats (
    user_id    INTEGER NOT NULL REFERENCES users (id)   ON DELETE CASCADE,
    artist_id  INTEGER NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    rating     INTEGER CHECK (rating BETWEEN 1 AND 5),
    starred_at TEXT,
    PRIMARY KEY (user_id, artist_id)
) WITHOUT ROWID;

CREATE INDEX user_artist_stats_starred_idx
    ON user_artist_stats (user_id, starred_at)
    WHERE starred_at IS NOT NULL;

CREATE TABLE playlists (
    id         INTEGER PRIMARY KEY,
    public_id  TEXT    NOT NULL UNIQUE,
    owner_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    comment    TEXT,
    is_public  INTEGER NOT NULL DEFAULT 0 CHECK (is_public IN (0, 1)),
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);

CREATE INDEX playlists_owner_idx ON playlists (owner_id);

-- Position is explicit and duplicates are allowed: the same track can appear
-- as many times as the user wants, so the key is (playlist, position) and
-- never (playlist, track). Keying on the track would silently collapse
-- repeats and lose the ordering, which is the classic way to get this wrong.
--
-- Consequence to keep in mind when writing the queries: shifting positions
-- upwards in place violates the primary key mid-statement, because SQLite
-- walks the rows in ascending order and cannot defer a uniqueness check.
-- Appending and removing by index are fine, and they are all the API offers.
-- Any reordering coming from the panel replaces the whole list inside a
-- transaction rather than doing position arithmetic.
CREATE TABLE playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    INTEGER NOT NULL REFERENCES tracks (id)    ON DELETE CASCADE,
    PRIMARY KEY (playlist_id, position)
) WITHOUT ROWID;

CREATE INDEX playlist_tracks_track_idx ON playlist_tracks (track_id);

CREATE TABLE bookmarks (
    user_id     INTEGER NOT NULL REFERENCES users (id)  ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL,
    comment     TEXT,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    PRIMARY KEY (user_id, track_id)
) WITHOUT ROWID;

CREATE TABLE play_queues (
    user_id          INTEGER NOT NULL PRIMARY KEY REFERENCES users (id)
                     ON DELETE CASCADE,
    current_track_id INTEGER          REFERENCES tracks (id) ON DELETE SET NULL,
    position_ms      INTEGER NOT NULL DEFAULT 0,
    changed_at       TEXT    NOT NULL,
    changed_by       TEXT    NOT NULL
) WITHOUT ROWID;

CREATE TABLE play_queue_tracks (
    user_id  INTEGER NOT NULL REFERENCES play_queues (user_id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    track_id INTEGER NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, position)
) WITHOUT ROWID;

CREATE TABLE now_playing (
    user_id    INTEGER NOT NULL REFERENCES users (id)  ON DELETE CASCADE,
    client     TEXT    NOT NULL,
    track_id   INTEGER NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    started_at TEXT    NOT NULL,
    PRIMARY KEY (user_id, client)
) WITHOUT ROWID;

-- ---------------------------------------------------------------------------
-- Search
-- ---------------------------------------------------------------------------

-- FTS5 gives search3 relevance ranking instead of a full table scan per
-- LIKE '%term%'.
--
-- These are standalone tables rather than FTS5 external content ones,
-- because the indexed text spans joins that external content cannot follow.
-- The scanner maintains them as it writes: it already holds every value in
-- hand, which is cheaper and far less fragile than triggers doing the joins
-- themselves. Rowids match the id of the entity they mirror.
CREATE VIRTUAL TABLE tracks_fts USING fts5 (
    title,
    album,
    artists,
    tokenize = "unicode61 remove_diacritics 2"
);

CREATE VIRTUAL TABLE albums_fts USING fts5 (
    name,
    artists,
    tokenize = "unicode61 remove_diacritics 2"
);

CREATE VIRTUAL TABLE artists_fts USING fts5 (
    name,
    tokenize = "unicode61 remove_diacritics 2"
);

-- ---------------------------------------------------------------------------
-- Scanner state
-- ---------------------------------------------------------------------------

CREATE TABLE scan_runs (
    id           INTEGER PRIMARY KEY,
    library_id   INTEGER          REFERENCES libraries (id) ON DELETE CASCADE,
    started_at   TEXT    NOT NULL,
    finished_at  TEXT,
    full_scan    INTEGER NOT NULL DEFAULT 0 CHECK (full_scan IN (0, 1)),
    tracks_seen  INTEGER NOT NULL DEFAULT 0,
    tracks_added INTEGER NOT NULL DEFAULT 0,
    error        TEXT
);

-- ---------------------------------------------------------------------------
-- Server settings
-- ---------------------------------------------------------------------------

-- One row, enforced by the check. Named columns rather than a key/value table:
-- with key/value every setting is text, every read is an untyped parse, and the
-- schema stops saying which settings exist.
CREATE TABLE settings (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    -- Words dropped when deciding which letter a name files under, separated by
    -- spaces. Stored the way OpenSubsonic reports them, and an article with a
    -- space in it would not work anyway: only the first word is compared.
    ignored_articles TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL
);
