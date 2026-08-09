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
-- something fetched from the network, and only remote entries expire. It also
-- decides which directory the bytes are in — what came off the network is kept
-- apart from what can be read again for nothing.
--
-- The four columns at the end are what a licence asks for and only something
-- fetched has: who made the picture, what it is licensed under, where those
-- terms are, and the page it came off. Null for everything read out of a file
-- on the user's own disk, which is nobody's to attribute.
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
    fetched_at   TEXT    NOT NULL,
    author       TEXT,
    license      TEXT,
    license_url  TEXT,
    source_url   TEXT
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
    -- Which record this row is, as the scanner decided it: the release id, or
    -- the album artist and the name folded to one string. Written here because
    -- otherwise the decision lives only in the scanner's memory, and a scan that
    -- rereads a file it already knows has no way to find the row it made last
    -- time — it inserts a second one, moves the tracks over, and leaves the
    -- first orphaned with the play counts and ratings hanging off it.
    --
    -- Not unique: an original and its remaster share an artist and a name and
    -- are two records, told apart by the year. The year is deliberately not in
    -- here, because a year missing from some of a record's tracks must not split
    -- it in two, and that is a judgement between candidates rather than a
    -- lookup — see `AlbumKey::grouping_key`.
    grouping_key       TEXT    NOT NULL,
    name               TEXT    NOT NULL,
    sort_name          TEXT,
    year               INTEGER,
    release_date       TEXT,
    original_date      TEXT,
    is_compilation     INTEGER NOT NULL DEFAULT 0
                       CHECK (is_compilation IN (0, 1)),
    mbid_release       TEXT,
    mbid_release_group TEXT,
    -- Who put the record out, as the tag says. A fact about the release and not
    -- about any one song on it, which is why it is here rather than on tracks —
    -- and taken from whichever of its files the album was first built from, since
    -- a record with two different labels in its tags has one label and a tagging
    -- mistake.
    label              TEXT,
    rg_album_gain      REAL,
    rg_album_peak      REAL,
    artwork_id         INTEGER REFERENCES artworks (id) ON DELETE SET NULL,
    created_at         TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL
);

CREATE INDEX albums_grouping_idx  ON albums (grouping_key);
CREATE INDEX albums_name_idx      ON albums (name);
CREATE INDEX albums_sort_name_idx ON albums (sort_name);
CREATE INDEX albums_mbid_idx      ON albums (mbid_release);

-- The three orders getAlbumList is asked for, so that a page of twenty is twenty
-- rows read and not the whole catalogue read, sorted in a temporary table and
-- then thrown away twenty at a time. Without them, "the newest twenty" walks
-- every album there is; with them SQLite reads the index in order and stops.
--
-- The third indexes the expression the name is sorted by rather than a column,
-- because the name a record files under is its sort name when it has one and its
-- own name when it has not, and an index on either column alone cannot answer
-- for that. The collation belongs in the index for the same reason: an index
-- ordered one way cannot serve an ORDER BY that asks for another.
CREATE INDEX albums_created_idx ON albums (created_at);
CREATE INDEX albums_year_idx    ON albums (year);
CREATE INDEX albums_alpha_idx   ON albums (coalesce(sort_name, name) COLLATE NOCASE);

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
    -- How the track credits whoever is on it, as the file writes it whole:
    -- "Tiziano Ferro feat. Anahí & Dulce María".
    --
    -- Kept as well as the rows in track_artists, not instead of them, because it
    -- says something they cannot. Those rows are who is on the track — three
    -- people, each with an identity of their own — and this is the sentence the
    -- record uses about them. Joining the names back up gives "Tiziano Ferro,
    -- Anahí, Dulce María", which is a list where the file had "feat." and "&", and
    -- those are the tagger's words about who did what.
    --
    -- Null for the ordinary file that credits one artist, where the name is the
    -- credit and storing it twice would be storing it twice.
    artist_credit    TEXT,
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
    -- Since when this file's tags could not be read: a permission that was taken
    -- away, a disk with a bad sector, a file half written by whatever was copying
    -- it. Null means the last attempt worked, which is the ordinary case.
    --
    -- It is here because a quick scan decides what to reopen by size and
    -- modification time, and a file that could not be read has neither of them
    -- changed — so it would never be read again, and would sit there with whatever
    -- little is known about it until somebody thought to ask for a full scan. This
    -- column is what makes the next quick scan try it again.
    --
    -- Not the same as missing. A missing file is not there; this one is there and
    -- will not open. Both can be true at once and neither implies the other.
    unreadable_since TEXT,
    -- And why, as the last attempt put it. Null whenever the column above is.
    --
    -- Kept because the alternative is what it replaced: the reason went to a warning
    -- in the log and nowhere else, so a collection with four files it cannot read
    -- said four and would not say which, let alone what to do about them. What a
    -- reader says is often cryptic, and it is still the only true answer there is —
    -- so it is stored as given, and only turned into plain words where we genuinely
    -- know what happened.
    --
    -- Overwritten on every failed attempt rather than kept from the first, which is
    -- the opposite of the column above and deliberate: since when is a fact about
    -- how long this has been going on, and why is a fact about the state of the file
    -- now. A permission restored and a tag corrupted in the same week is one file
    -- that never opened and two different things to do about it.
    unreadable_error TEXT,
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

-- Whether a record has anything left to play, which is asked of every album in
-- every listing and is therefore the most repeated question in the database.
--
-- Both columns are in it so the question is answered inside the index: the album
-- narrows it and the library decides it, and neither costs a visit to the track
-- itself. `tracks_album_idx` above cannot do that — it finds the tracks of an
-- album and then has to read each one to learn which library it is in and
-- whether its file is still there. It stays, because plenty of statements want
-- an album's tracks whether they are missing or not.
--
-- Partial for the same reason the missing index is, and the opposite way round:
-- a listing never wants a track whose file is gone, so they are better left out
-- of the index than filtered out of it.
CREATE INDEX tracks_present_idx ON tracks (album_id, library_id)
    WHERE missing_since IS NULL;

-- Picking songs at random, which cannot avoid considering every song that could
-- be picked but can avoid reading them. Library, year and identity are what the
-- choosing needs, so it is all here and the table is never opened for the ones
-- not chosen.
CREATE INDEX tracks_pick_idx ON tracks (library_id, year, id)
    WHERE missing_since IS NULL;

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
    -- When that hash was last written. Not derivable from `updated_at`, which moves
    -- for a change of address as readily as for a change of password, and the whole
    -- point of showing it is that it answers when the password was last changed.
    --
    -- Defaulted rather than demanded of every insert, and the default is the right
    -- answer rather than a placeholder: a row that arrives with a hash and says
    -- nothing about when it was set had it set now. What has to be explicit is the
    -- update, where saying nothing would mean a date that moves with the address.
    password_set_at    TEXT    NOT NULL
                       DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    -- What this person would rather be called, or null to be called by the name
    -- above. Nothing authenticates with it and nothing looks anybody up by it: it is
    -- read where somebody is being addressed or shown, and nowhere else.
    --
    -- It exists because `username` stopped being theirs to change: renaming an
    -- account is administration, since the name is what every OpenSubsonic client
    -- logs in with. So the name an administrator files you under and the name you
    -- are greeted by are two different things, and this is the second one.
    --
    -- Null rather than a copy of the username, and never an empty string: emptying
    -- the field means going back to being called by the account's name, and two ways
    -- of saying that would be two things to check everywhere it is read.
    display_name       TEXT    CHECK (display_name IS NULL OR display_name <> ''),
    -- Optional, and a way in: somebody may log into the panel with this instead of
    -- with the name above, which is the whole reason it is worth keeping. `/rest`
    -- takes the username and only the username, because the protocol says so.
    --
    -- Never an empty string, for the same reason `display_name` is not: with two ways
    -- to say "no address" the index below would find every account without one to be
    -- the same account.
    email              TEXT    CHECK (email IS NULL OR email <> ''),
    -- Roughly when a request last arrived on this account, by any door: the panel,
    -- a password over /rest, or an API key. Null means never — an account that was
    -- created and has not been used since, which is the thing an administrator is
    -- looking for when they wonder who is still listening.
    --
    -- Written at the same resolution as a session's `last_seen_at` and for the same
    -- reason: a column read once in a while does not deserve a write per request.
    last_seen_at       TEXT,
    is_admin           INTEGER NOT NULL DEFAULT 0 CHECK (is_admin IN (0, 1)),
    scrobbling_enabled INTEGER NOT NULL DEFAULT 1
                       CHECK (scrobbling_enabled IN (0, 1)),
    created_at         TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL
);

-- One account per address, since an address is a way into the panel and an
-- identifier that answers for two accounts cannot let anybody in.
--
-- Folded, because nobody types their own address the same way twice and every mail
-- system in use has long since stopped caring: OGarcia@Example.org and
-- ogarcia@example.org are one address here, as they are everywhere else.
--
-- Partial, so that having no address is not a thing two accounts can collide over.
-- Which is also why the column refuses the empty string: without that, every account
-- somebody left the field blank on would be the same account as far as this is
-- concerned.
CREATE UNIQUE INDEX users_email_idx ON users (lower(email)) WHERE email IS NOT NULL;

-- What somebody chose about how the panel looks and speaks. Kept with the
-- account rather than in the browser so that logging in somewhere else brings it
-- along, which is the whole reason it is on this side at all.
--
-- Its own table, not columns on users: users is the account — who you are, what
-- you may do, what authenticates you — and none of this is. A panel is one client
-- of this server, and its idea of an accent colour has no business widening the
-- row that every authentication reads.
--
-- No row means nothing was chosen, and so does a null column, which is not the
-- same as a value: no theme is following the machine, no locale is following the
-- browser, no accent is the one the panel ships with. A default here would be the
-- server deciding what it cannot know.
--
-- The values are opaque. The server stores identifiers and never reads them,
-- because what a theme or an accent can be belongs to the panel: adding a colour
-- should be a line of CSS and not a migration. The panel falls back to its own
-- default for anything it does not recognise, which is also what makes a value
-- left by an older panel harmless.
CREATE TABLE panel_preferences (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    theme   TEXT,
    locale  TEXT,
    accent  TEXT
) WITHOUT ROWID;

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
-- removed, so the date can be pushed out and the same key start working again
-- rather than every client having to be set up afresh.
--
-- Revoking is a state and not a delete, so a key is withdrawn and removed in two
-- steps rather than one. A row that vanished the moment it was revoked took its
-- name with it, and left whoever pressed the button with nothing to check
-- against — while the word "revoke" is not one everybody reads as "delete".
-- Revoking is final, though: a revoked key authenticates nothing and there is no
-- way back to a key that works, only the way out.
CREATE TABLE api_keys (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key_hash     TEXT    NOT NULL UNIQUE,
    label        TEXT    NOT NULL,
    created_at   TEXT    NOT NULL,
    -- Null means it never expires. Stored in the same shape as every other
    -- timestamp here, because it is compared as text against one of them.
    expires_at   TEXT,
    last_used_at TEXT,
    -- Null means it still works. Set, and the key is dead whatever its expiry
    -- says, which is why the moment is kept rather than a flag: a row that says
    -- when it was withdrawn is a row that can be read afterwards.
    revoked_at   TEXT
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
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    -- Words dropped when deciding which letter a name files under, separated by
    -- spaces. Stored the way OpenSubsonic reports them, and an article with a
    -- space in it would not work anyway: only the first word is compared.
    ignored_articles  TEXT    NOT NULL,
    -- Whether a quick scan runs every time the server starts. On by default,
    -- since that is what a server that was off all night ought to do; off for a
    -- library on a network share that takes an hour to walk.
    scan_at_startup   INTEGER NOT NULL CHECK (scan_at_startup IN (0, 1)),
    -- The minute of the local day a quick scan runs at, "HH:MM", or null for no
    -- schedule at all. Local rather than UTC because it is chosen by somebody
    -- who means "while I am asleep".
    scan_at           TEXT             CHECK (scan_at IS NULL OR scan_at GLOB
                                              '[0-2][0-9]:[0-5][0-9]'),
    -- How many days a track stays marked absent before a scan clears it out for
    -- good, or null to never clear it automatically. Zero means the scan that
    -- finds a file gone is the one that removes it.
    --
    -- Absent is not the same as deleted: the usual reason a file is not there is
    -- a disk that failed to mount, so the default is null and the purge stays a
    -- thing somebody asks for.
    absent_grace_days INTEGER          CHECK (absent_grace_days IS NULL
                                              OR absent_grace_days >= 0),
    -- How long a panel login lasts, in days. Absolute: see the sessions table.
    session_days      INTEGER NOT NULL CHECK (session_days > 0),
    -- Whether this server may talk to anybody at all: walking out to MusicBrainz
    -- and Wikimedia Commons for photographs of the artists it holds, and passing
    -- listens on to a scrobbling service. Off, and off is what a collection that
    -- has never been asked gets: everything else here happens between this
    -- machine and its own disk, and reaching somebody else's server is a thing to
    -- be asked for rather than assumed.
    --
    -- Read straight out of this row by the statements that decide whether a
    -- listen is queued and where one may be sent, so switching it off stops
    -- those in the same breath rather than after something has been sent.
    reach_out         INTEGER NOT NULL DEFAULT 0
                                       CHECK (reach_out IN (0, 1)),
    updated_at        TEXT    NOT NULL
);

-- ---------------------------------------------------------------------------
-- Maintenance
-- ---------------------------------------------------------------------------

-- What each maintenance job did, and when. One row per run, kept so that the
-- panel can say "last run twelve days ago, removed 21" rather than offering a
-- button with no memory behind it.
--
-- What a run found is one number on purpose. Every job counts something
-- different — tracks removed, bytes reclaimed, files deleted, problems found —
-- but every one of them counts something, and the job says what the number is
-- of. A column per job, or a blob of JSON, would both be ways of not deciding
-- that.
CREATE TABLE job_runs (
    id          INTEGER PRIMARY KEY,
    -- Which job, as the name the API uses. Not a foreign key: the jobs are in
    -- the program, not in a table, and a job that stops existing should leave
    -- its history readable rather than take it along.
    job         TEXT    NOT NULL,
    started_at  TEXT    NOT NULL,
    -- Null while it is still running, which is also what tells a run that was
    -- interrupted by the server stopping from one that finished.
    finished_at TEXT,
    -- How much of whatever this job counts. Zero is a real answer: a check that
    -- found nothing wrong is the answer somebody wanted.
    affected    INTEGER NOT NULL DEFAULT 0,
    error       TEXT
);

-- The panel asks for the last run of each job, and for the last few runs of
-- anything, and both are this index read in one direction.
CREATE INDEX job_runs_by_job ON job_runs (job, started_at DESC);

-- ---------------------------------------------------------------------------
-- Scrobbling
-- ---------------------------------------------------------------------------

-- Where somebody's listens go besides here.
--
-- One row per service per person, so that music can be sent to a hosted
-- ListenBrainz and to something running at home at once: they are not two names
-- for one destination, and somebody trying a self hosted scrobbler out has every
-- reason to keep the other one filling up meanwhile.
--
-- Its own table rather than columns on users, for the reason panel_preferences is
-- one: users is the account, and a token belonging to somebody else's website has
-- no business widening the row that every authentication reads. Which service it
-- is stays a name in the program rather than a table of its own, the way a job
-- does — what a service is amounts to a URL and a dialect, and both are code.
--
-- Not in settings either. A collection has one set of ignored articles and one
-- scan hour; listens belong to whoever listened, and two people sharing a server
-- do not share a scrobbling account.
CREATE TABLE scrobblers (
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Which service, as the name the API uses: 'listenbrainz', 'koito'. Not a
    -- foreign key for the same reason job_runs.job is not one.
    service     TEXT    NOT NULL,
    -- The root of the instance, without the path the protocol adds: what somebody
    -- typed, or the official host for a service that has one. Kept even when the
    -- service has a fixed address, so that a service moving is a row to update
    -- rather than a release to wait for.
    url         TEXT    NOT NULL,
    -- In clear, unavoidably: it is a bearer token and it has to be sent, so there
    -- is nothing a hash could be checked against. Everything else secret in this
    -- schema is hashed precisely because it never has to be replayed — a password
    -- and an API key are checked, not presented — and the difference is worth
    -- writing down here rather than looking like an oversight.
    --
    -- Which means a stolen database hands over these, and nothing else. It is one
    -- more reason the panel never sends a token back out once it is stored.
    token       TEXT    NOT NULL,
    -- What the service says the account is called, from the check made when the
    -- token was saved. Null when it could not be asked — a machine that was off,
    -- or a service with nothing to ask — which is not the same as a token known
    -- to be wrong: that one is refused and never gets a row.
    remote_name TEXT,
    -- Off keeps the queue and stops both the sending and the filling: what is
    -- already waiting stays waiting, and nothing new is added. Removing the row is
    -- the other thing, and it takes the queue with it.
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    PRIMARY KEY (user_id, service)
) WITHOUT ROWID;

-- Listens waiting to be handed over.
--
-- A row per listen per service, because the same song can be accepted by one and
-- refused by the other, and a single row with two half states would have to be
-- read as two rows anyway.
--
-- **It holds the song and not a reference to it.** A queued listen names no
-- track_id: what is written down is what the song was at the moment it was
-- listened to. Otherwise a purge — or a library removed, or a file retagged —
-- would rewrite or destroy what somebody heard yesterday and has not managed to
-- send yet, and a listen that arrives at ListenBrainz as a different song is
-- worse than one that never arrives.
--
-- Named columns rather than the JSON that will be sent, for the reason settings
-- has columns: the queue is data about listening, not bytes of one protocol. It
-- survives a fix to the wire format, it can be shown to somebody as "3 waiting,
-- oldest from Tuesday", and the second protocol to come along reads the same
-- rows.
CREATE TABLE scrobble_queue (
    id             INTEGER PRIMARY KEY,
    user_id        INTEGER NOT NULL,
    service        TEXT    NOT NULL,
    -- When it was heard, as everything else here is written. The wire wants a
    -- unix timestamp and gets one at the last moment; storing that instead would
    -- make this the one date in the schema nobody can read.
    played_at      TEXT    NOT NULL,
    -- The two the far end insists on. A song with no artist at all is never
    -- queued: there is nothing to submit, and inventing "Unknown Artist" would
    -- put a listen in somebody's history against a band that does not exist.
    title          TEXT    NOT NULL,
    artist         TEXT    NOT NULL,
    album          TEXT,
    -- Everything that helps the far end match it to the right recording rather
    -- than to a song of the same name. All optional, all as they were tagged.
    mbid_recording TEXT,
    mbid_release   TEXT,
    mbid_artist    TEXT,
    isrc           TEXT,
    track_number   INTEGER,
    duration_ms    INTEGER,
    -- How many times it has been offered and not accepted. Kept so that the wait
    -- can grow with it, and so that the panel can say something is stuck instead
    -- of only that it is waiting.
    attempts       INTEGER NOT NULL DEFAULT 0,
    -- Not before this. Set to now when it is queued, and pushed further out after
    -- every failure; a service that is down for a day is asked a handful of times
    -- rather than every minute of it.
    next_try_at    TEXT    NOT NULL,
    -- Why the last attempt failed, for somebody looking at why nothing is moving.
    -- A wrong token and a machine that is off read very differently.
    last_error     TEXT,
    created_at     TEXT    NOT NULL,
    -- Composite, so that removing a service takes its unsent listens with it: a
    -- queue for a destination nobody is configured to reach is a queue nothing
    -- will ever drain.
    FOREIGN KEY (user_id, service) REFERENCES scrobblers (user_id, service)
        ON DELETE CASCADE
);

-- The one question the sender asks: what is due. Ordered by when it is due and
-- then by when it happened, so a backlog is handed over oldest first.
CREATE INDEX scrobble_queue_due_idx ON scrobble_queue (next_try_at, played_at);

-- And the one the panel asks: how much of mine is waiting.
CREATE INDEX scrobble_queue_whose_idx ON scrobble_queue (user_id, service);
