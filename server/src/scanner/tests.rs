// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! End to end tests of a scan, against a real database.

use super::*;
use std::fs;

/// A database with the schema applied, in memory.
async fn database() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

/// Registers a library and returns its id.
async fn library(pool: &SqlitePool, root: &Path) -> i64 {
    let timestamp = db::now();
    sqlx::query_scalar(
        "INSERT INTO libraries (name, path, created_at, updated_at)
         VALUES ('test', ?, ?, ?) RETURNING id",
    )
    .bind(root.to_string_lossy().as_ref())
    .bind(&timestamp)
    .bind(&timestamp)
    .fetch_one(pool)
    .await
    .unwrap()
}

use crate::fixtures::write_wav;

/// Sets a file's modification time, which is the only thing a file can say about when it
/// came.
///
/// Through `touch` rather than a crate for it: this is wanted by four tests and nothing
/// else, and a dependency added to the shipped binary to date a file in a test would be
/// paid for by everybody ([[revisar-crates-antes-de-anadir]] in spirit — the cheapest
/// dependency is the one not taken).
fn touched(path: &Path, iso: &str) {
    let done = std::process::Command::new("touch")
        .arg("-d")
        .arg(iso)
        .arg(path)
        .status()
        .expect("touch is on the path");

    assert!(done.success(), "could not date {}", path.display());
}

/// Writes tags onto a file already on disk, in this module's own vocabulary.
///
/// The words rather than lofty's keys, because every test here reads better for it,
/// and ID3v2 because that is what a RIFF container will hold.
///
/// A release id goes in by the frame's name instead of by its key: lofty reads
/// `ItemKey::MusicBrainzReleaseId` out of a file and does not write it into one, so
/// asking for it that way puts nothing on disk — see `tag_file_naming_frames`.
fn tag(path: &Path, items: &[(&str, &str)]) {
    use lofty::prelude::ItemKey;

    let mut keyed: Vec<(ItemKey, &str)> = Vec::new();
    let mut named: Vec<(&str, &str)> = Vec::new();

    for (key, value) in items {
        let key = match *key {
            "album" => ItemKey::AlbumTitle,
            "albumartist" => ItemKey::AlbumArtist,
            "artist" => ItemKey::TrackArtist,
            "title" => ItemKey::TrackTitle,
            // Written as a recording date, which is the field that survives a
            // RIFF container.
            "year" => ItemKey::RecordingDate,
            "release" => {
                named.push(("MusicBrainz Album Id", value));
                continue;
            }
            "compilation" => ItemKey::FlagCompilation,
            "artists" => ItemKey::TrackArtists,
            "genre" => ItemKey::Genre,
            "artist_mbid" => ItemKey::MusicBrainzArtistId,
            "albumartist_mbid" => ItemKey::MusicBrainzReleaseArtistId,
            other => panic!("unknown tag {other}"),
        };

        keyed.push((key, value));
    }

    crate::fixtures::tag_file_naming_frames(path, &keyed, &named);
}

fn temp_root(name: &str) -> PathBuf {
    crate::fixtures::temp_root(&format!("scan-{name}"))
}

/// A scan expected to run to the end, which is every one of these but the last.
/// The interruption flag lives on `Progress`, so a fresh one never trips it.
async fn scan(pool: &SqlitePool, id: i64, root: &Path, mode: Mode) -> Result<Outcome> {
    Ok(scan_library(
        pool,
        id,
        root,
        mode,
        &Progress::default(),
        &mut HashSet::new(),
    )
    .await?
    .expect("the scan ran to the end"))
}

async fn count(pool: &SqlitePool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn a_second_incremental_scan_reads_nothing_again() {
    let root = temp_root("incremental");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;

    let first = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(first.tracks, 5);
    assert_eq!(first.unchanged, 0, "nothing was known yet");

    let second = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(second.tracks, 5);
    assert_eq!(second.unchanged, 5, "no file changed, so none was reopened");
    assert_eq!(second.gone, 0);

    assert_eq!(count(&pool, "SELECT count(*) FROM tracks").await, 5);
}

/// The first scan of a library takes its arrival dates from the files.
///
/// Everything a scan writes is stamped with the moment it ran, so without this every
/// album of a collection just imported arrives at the same second and "recently added"
/// comes out in whatever order the directories were walked in.
#[tokio::test]
async fn a_first_scan_dates_a_library_by_its_files() {
    let root = temp_root("arrival");

    // Two records, one older than the other on disk, and read in the order that would
    // put them the other way round if the clock were what counted.
    for (folder, when) in [
        ("Newer", "2026-06-01T12:00:00Z"),
        ("Older", "2009-03-04T09:00:00Z"),
    ] {
        let path = root.join(format!("{folder}/one.wav"));
        write_wav(&path);
        tag(&path, &[("album", folder), ("artist", folder)]);
        touched(&path, when);
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // Which is the order `getAlbumList?type=newest` asks for.
    let newest: Vec<String> =
        sqlx::query_scalar("SELECT name FROM albums ORDER BY created_at DESC")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(newest, ["Newer", "Older"]);

    let dated: Vec<(String, String)> =
        sqlx::query_as("SELECT name, created_at FROM albums ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(dated[0].1, "2026-06-01T12:00:00Z", "the file's own date");
    assert_eq!(dated[1].1, "2009-03-04T09:00:00Z");

    // The tracks and the names that came with them are dated the same way, so nothing in
    // the collection disagrees about when this arrived.
    let track: String = sqlx::query_scalar("SELECT created_at FROM tracks ORDER BY path LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(track, "2026-06-01T12:00:00Z");

    let artist: String = sqlx::query_scalar("SELECT created_at FROM artists WHERE name = 'Older'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(artist, "2009-03-04T09:00:00Z");
}

/// A record that turns up later arrived later, whatever its files say.
///
/// The half of this that is easy to get wrong. A disc copied in with its timestamps
/// preserved has old files and is new here, so taking the file's word for it on any scan
/// but the first would bury it at the bottom of what is new — the opposite of what the
/// dates are for. And it is why nothing offers to do this by hand: months later it would
/// throw away a real history and put a guess in its place.
#[tokio::test]
async fn only_the_first_scan_of_a_library_believes_the_files() {
    let root = temp_root("arrival-later");
    let first = root.join("First/one.wav");
    write_wav(&first);
    tag(&first, &[("album", "First")]);
    touched(&first, "2009-03-04T09:00:00Z");

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // Copied in afterwards, with a file older than the one already there.
    let later = root.join("Later/one.wav");
    write_wav(&later);
    tag(&later, &[("album", "Later")]);
    touched(&later, "2001-01-01T00:00:00Z");

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let dated: Vec<(String, String)> =
        sqlx::query_as("SELECT name, created_at FROM albums ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(
        dated[0].1, "2009-03-04T09:00:00Z",
        "dated on the first scan"
    );
    assert!(
        dated[1].1 > dated[0].1,
        "and the one that turned up later arrived later, though its file is older: {:?}",
        dated
    );
}

/// An album arrived when the earliest of its tracks did.
///
/// The latest would send a record from 2009 to the top of "recently added" the day a
/// stray track was dropped into it.
#[tokio::test]
async fn an_album_arrived_with_the_first_of_its_tracks() {
    let root = temp_root("arrival-earliest");

    for (n, when) in [(1, "2009-03-04T09:00:00Z"), (2, "2020-08-08T08:00:00Z")] {
        let path = root.join(format!("Album/{n:02}.wav"));
        write_wav(&path);
        tag(&path, &[("album", "Album")]);
        touched(&path, when);
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let dated: String = sqlx::query_scalar("SELECT created_at FROM albums")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(dated, "2009-03-04T09:00:00Z");
}

/// A file dated in the future keeps the moment of the scan.
///
/// A wrong clock or a bad archive, and the point is that it must not sit ahead of what
/// genuinely arrives next.
#[tokio::test]
async fn a_file_from_the_future_does_not_get_ahead() {
    let root = temp_root("arrival-future");
    let path = root.join("Album/one.wav");
    write_wav(&path);
    tag(&path, &[("album", "Album")]);
    touched(&path, "2099-01-01T00:00:00Z");

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // Asked of all three, because each is its own statement and each needs the same
    // brake: a scan writing one of them forward would be a track ahead of its own album.
    for what in [
        "SELECT created_at FROM albums",
        "SELECT created_at FROM tracks",
    ] {
        let dated: String = sqlx::query_scalar(what).fetch_one(&pool).await.unwrap();

        assert!(
            dated.as_str() < "2099",
            "kept the scan's own moment ({what}): {dated}"
        );
    }
}

#[tokio::test]
async fn a_full_scan_reads_everything_again() {
    let root = temp_root("full");
    write_wav(&root.join("Album/one.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    let full = scan(&pool, id, &root, Mode::Full).await.unwrap();

    assert_eq!(full.unchanged, 0, "a full scan skips nothing");
    assert_eq!(full.tracks, 1);
}

/// Reading a file again must not make a second record of the album it is on.
///
/// This is the one an early Tocata got wrong, and it went unseen because the
/// test above scans an untagged file, which belongs to no album at all. The
/// albums lived in a map that a scan built from nothing and threw away at the
/// end, so the next scan to look inside a file it already knew found no record,
/// made another, moved the tracks onto it and left the first one behind — with
/// the play counts, the rating and the star still hanging off the row nobody
/// would ever see again.
///
/// Which is why the assertion is on the id and not just on the count: a record
/// that keeps its row keeps everything anyone attached to it.
#[tokio::test]
async fn a_scan_finds_the_album_a_previous_scan_made() {
    let root = temp_root("album-again");
    for n in 1..=3 {
        let path = root.join(format!("Album/{n:02}.wav"));
        write_wav(&path);
        tag(
            &path,
            &[
                ("album", "Kid A"),
                ("albumartist", "Radiohead"),
                ("artist", "Radiohead"),
                ("year", "2000"),
            ],
        );
    }

    let pool = database().await;
    let id = library(&pool, &root).await;

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    let first: i64 = sqlx::query_scalar("SELECT id FROM albums")
        .fetch_one(&pool)
        .await
        .unwrap();

    scan(&pool, id, &root, Mode::Full).await.unwrap();

    assert_eq!(
        count(&pool, "SELECT count(*) FROM albums").await,
        1,
        "one record, however many times its files are read"
    );
    assert_eq!(
        count(&pool, "SELECT id FROM albums").await,
        first,
        "and the same row, so nothing anyone attached to it is orphaned"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE album_id IS NOT NULL"
        )
        .await,
        3,
        "with its tracks still on it"
    );
}

/// The other two ways a record is identified, which take different paths through
/// the lookup: a release id names the record outright, and a compilation is
/// grouped without regard to who is on it.
#[tokio::test]
async fn a_release_id_and_a_compilation_survive_a_second_scan_too() {
    let root = temp_root("keys-again");

    let released = root.join("Release/01.wav");
    write_wav(&released);
    tag(
        &released,
        &[
            ("album", "OK Computer"),
            ("artist", "Radiohead"),
            ("release", "e7c0e2ef-0b6d-4d6f-b4a5-0d6f4e0f8b3c"),
        ],
    );

    // Two tracks by different people, which is the whole point of the flag.
    for (n, who) in [(1, "Björk"), (2, "Pulp")] {
        let path = root.join(format!("Hits/{n:02}.wav"));
        write_wav(&path);
        tag(
            &path,
            &[("album", "Hits 96"), ("artist", who), ("compilation", "1")],
        );
    }

    let pool = database().await;
    let id = library(&pool, &root).await;

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM albums").await, 2);

    // Which paths they actually took, because the test cannot see it otherwise and
    // spent a while not taking the first one: a release id that never reached the
    // file left OK Computer filed under its artist and its title like any other
    // record, and everything below still passed.
    //
    // Each record asked for by name. It was `ORDER BY id` and took the first row for
    // the release and the second for the compilation, which is the order the two
    // folders were walked in — and that is whatever order the filesystem hands back
    // two directories sitting side by side. Nothing promises it, and the CI came back
    // with "the release id names the record: compilationhits 96" — the separator is in
    // there, it just does not print — to say so. Writing Hits before Release above
    // reproduces that here, word for word, every time.
    let released: String = sqlx::query_scalar("SELECT grouping_key FROM albums WHERE name = ?")
        .bind("OK Computer")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        released.starts_with("release\u{1f}"),
        "the release id names the record: {released}"
    );

    let collected: String = sqlx::query_scalar("SELECT grouping_key FROM albums WHERE name = ?")
        .bind("Hits 96")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        collected.starts_with("compilation\u{1f}"),
        "and the flag files it without regard to who is on it: {collected}"
    );

    scan(&pool, id, &root, Mode::Full).await.unwrap();
    assert_eq!(
        count(&pool, "SELECT count(*) FROM albums").await,
        2,
        "neither of them was made twice"
    );
}

/// A track by three people is by three people.
///
/// Taken from a real file. The credit could not be split — "feat." and "&" are not
/// separators anybody dares cut on — so the whole sentence went in as one artist:
/// a row for somebody who does not exist, nothing to enrich, and no way to find the
/// song by asking for one of the three. The names were in `ARTISTS` all along, and
/// the identifiers beside them in the same order.
#[tokio::test]
async fn a_track_credited_to_several_people_names_each_of_them() {
    let root = temp_root("collaboration");
    let path = root.join("Album/05.wav");
    write_wav(&path);

    tag(
        &path,
        &[
            ("album", "A mi edad"),
            ("albumartist", "Tiziano Ferro"),
            ("albumartist_mbid", "d12b05b0-a0af-4c2c-8c8c-ab8bcf49439e"),
            ("artist", "Tiziano Ferro feat. Anahí & Dulce María"),
            ("artists", "Tiziano Ferro"),
            ("artists", "Anahí"),
            ("artists", "Dulce María"),
            ("artist_mbid", "d12b05b0-a0af-4c2c-8c8c-ab8bcf49439e"),
            ("artist_mbid", "4792522c-9eec-4491-9640-8922d5fbf2c5"),
            ("artist_mbid", "f07d2a0a-955f-4adc-b70a-7aba348f343d"),
        ],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let credited: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT ar.name, ar.mbid FROM track_artists ta
           JOIN artists ar ON ar.id = ta.artist_id
          WHERE ta.role = 'artist' ORDER BY ta.position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(
        credited,
        [
            (
                "Tiziano Ferro".to_string(),
                Some("d12b05b0-a0af-4c2c-8c8c-ab8bcf49439e".to_string())
            ),
            (
                "Anahí".to_string(),
                Some("4792522c-9eec-4491-9640-8922d5fbf2c5".to_string())
            ),
            (
                "Dulce María".to_string(),
                Some("f07d2a0a-955f-4adc-b70a-7aba348f343d".to_string())
            ),
        ],
        "three people, each with the identity the file gave them, in order"
    );

    // And the sentence the record uses about them, which no list of names is.
    let credit: Option<String> = sqlx::query_scalar("SELECT artist_credit FROM tracks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        credit.as_deref(),
        Some("Tiziano Ferro feat. Anahí & Dulce María")
    );

    // Which is the point of naming them: asking for the second one finds the song.
    let found = count(
        &pool,
        "SELECT count(*) FROM tracks_fts WHERE tracks_fts MATCH 'Anahí'",
    )
    .await;
    assert_eq!(found, 1, "the song answers to somebody who is on it");

    // The record is credited to whoever the album artist tag says, with their own
    // identifier — and that one artist is the same row as the first of the three.
    let signed: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT ar.name, ar.mbid FROM album_artists aa
           JOIN artists ar ON ar.id = aa.artist_id
          WHERE aa.role = 'albumartist'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        signed,
        [(
            "Tiziano Ferro".to_string(),
            Some("d12b05b0-a0af-4c2c-8c8c-ab8bcf49439e".to_string())
        )]
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM artists").await,
        3,
        "and no fourth row for the credit as a whole"
    );
}

/// The other file, which is almost every file: one name, one identifier, no list.
/// Nothing here is left cojo by the case above.
#[tokio::test]
async fn one_artist_with_one_identifier_is_identified_too() {
    let root = temp_root("one-artist");
    let path = root.join("Grace/01.wav");
    write_wav(&path);

    tag(
        &path,
        &[
            ("album", "Grace"),
            ("artist", "Jeff Buckley"),
            ("albumartist", "Jeff Buckley"),
            ("artist_mbid", "e6e879c0-3d56-4f12-b3c5-3ce459661a8e"),
            ("albumartist_mbid", "e6e879c0-3d56-4f12-b3c5-3ce459661a8e"),
        ],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let artists: Vec<(String, Option<String>)> = sqlx::query_as("SELECT name, mbid FROM artists")
        .fetch_all(&pool)
        .await
        .unwrap();

    assert_eq!(
        artists,
        [(
            "Jeff Buckley".to_string(),
            Some("e6e879c0-3d56-4f12-b3c5-3ce459661a8e".to_string())
        )],
        "one row, identified once, however many tags name them"
    );

    // Nothing kept twice: the credit is the name, so there is no second copy of it.
    let credit: Option<String> = sqlx::query_scalar("SELECT artist_credit FROM tracks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(credit, None);
}

/// Two names tagged with one identifier between them. The schema will not hold two
/// artists claiming to be the same person, and a scan that failed on it would be a
/// scan that stops at a tagging mistake somebody made years ago.
#[tokio::test]
async fn one_identifier_shared_by_two_names_does_not_break_the_scan() {
    let root = temp_root("same-mbid");

    for (n, who) in [(1, "Anahí"), (2, "Anahi")] {
        let path = root.join(format!("Album/{n:02}.wav"));
        write_wav(&path);
        tag(
            &path,
            &[
                ("album", "Mi Delirio"),
                ("artist", who),
                ("artist_mbid", "4792522c-9eec-4491-9640-8922d5fbf2c5"),
            ],
        );
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    let outcome = scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(outcome.tracks, 2, "both files were read");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM artists WHERE mbid = '4792522c-9eec-4491-9640-8922d5fbf2c5'"
        )
        .await,
        1,
        "the identity belongs to one row: the first to claim it keeps it"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM artists").await,
        2,
        "and the other name is still an artist, just an unidentified one"
    );
}

/// Tags get corrected, and the record has to take the correction. It is the same
/// record — the release id says so — and it now says something else about itself.
///
/// Worth its own test because the record surviving the scan that made it is what
/// makes this possible to get wrong: the row is no longer rewritten from nothing
/// every time, so what is on it and what is in the search index are only right
/// if the scan puts them right.
#[tokio::test]
async fn a_record_that_already_existed_takes_the_corrected_tags() {
    let root = temp_root("retagged");
    let path = root.join("Album/01.wav");
    write_wav(&path);

    let release = "e7c0e2ef-0b6d-4d6f-b4a5-0d6f4e0f8b3c";
    tag(
        &path,
        &[
            ("album", "kid a"),
            ("artist", "Radiohead"),
            ("release", release),
        ],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // Somebody capitalised the title and said who the record is by.
    tag(
        &path,
        &[
            ("album", "Kid A"),
            ("artist", "Radiohead"),
            ("albumartist", "Radiohead"),
            ("release", release),
        ],
    );

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(count(&pool, "SELECT count(*) FROM albums").await, 1);

    let name: String = sqlx::query_scalar("SELECT name FROM albums")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "Kid A", "the record goes by what its files now say");

    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM albums_fts WHERE albums_fts MATCH 'Radiohead'"
        )
        .await,
        1,
        "and answers to the name it was just credited to"
    );
}

/// Correct the name a record is filed under, on every file of it, and the record
/// used to end up under both names at once.
///
/// Its tracks correct themselves, since each is read whole and written again from
/// nothing. The record cannot: it is written once per track of it, each call
/// knowing one file, so all it may do is add. What it added was never taken away,
/// and the old name was left signing a record with no track of its own — which the
/// tidying will not collect either, because a record still names it.
///
/// The clearing belongs to the full pass, which reads every file, so what the
/// tracks put back is what the files say. The quick pass may reread three songs of
/// fifteen, and there it would throw away what the other twelve still hold.
///
/// A release id on the files, because it is what the record needs to stay the same
/// record: what an ordinary album is filed under includes whoever signs it, so
/// renaming them files a second record instead of correcting the first. It is also
/// how this happens in the wild — the tagger that renames the band is the one that
/// wrote the identifier.
#[tokio::test]
async fn renaming_who_signs_a_record_leaves_no_ghost_behind_a_full_scan() {
    let root = temp_root("renamed-signer");
    let release = "6bb7fcd3-fd2a-4cea-a0d0-05c16c6ef7c1";
    let songs = ["01", "02"];

    for n in songs {
        let path = root.join(format!("Some Nights/{n}.wav"));
        write_wav(&path);
        tag(
            &path,
            &[
                ("album", "Some Nights"),
                ("artist", "fun."),
                ("albumartist", "fun."),
                ("genre", "Indie"),
                ("release", release),
            ],
        );
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // Somebody decided the full stop was a typo, and retagged the whole record.
    for n in songs {
        tag(
            &root.join(format!("Some Nights/{n}.wav")),
            &[
                ("album", "Some Nights"),
                ("artist", "Fun"),
                ("albumartist", "Fun"),
                ("genre", "Indie Rock"),
                ("release", release),
            ],
        );
    }

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(
        count(&pool, "SELECT count(*) FROM albums").await,
        1,
        "the same record throughout, which is what makes the rest of this a test"
    );
    assert_eq!(
        signers(&pool).await,
        vec!["Fun".to_string(), "fun.".to_string()],
        "the quick pass adds and does not take away, which is what it is for"
    );

    scan(&pool, id, &root, Mode::Full).await.unwrap();

    assert_eq!(
        signers(&pool).await,
        vec!["Fun".to_string()],
        "and the full pass, having read every file, leaves only what they say"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM album_genres").await,
        1,
        "a genre dropped from the tags goes the same way"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM albums").await,
        1,
        "and the record itself is the one it always was"
    );
}

/// The clearing above is once per record and per scan, and a record is not inside
/// a library: a compilation, or a release id, gathers the copies in every library
/// that holds one. A set of written records per library would have the second
/// library clear what the first had just put there, and the record would come out
/// saying what that one copy says rather than what both do.
///
/// So the two copies here disagree about who signs the record, and the answer is
/// both of them — the same answer two tracks that disagree inside one library get.
#[tokio::test]
async fn two_libraries_holding_one_record_do_not_undo_each_other() {
    let pool = database().await;

    for (n, signed) in [(1, "fun."), (2, "Fun")] {
        let root = temp_root(&format!("shared-record-{n}"));
        let path = root.join(format!("Some Nights/{n:02}.wav"));
        write_wav(&path);
        tag(
            &path,
            &[
                ("album", "Some Nights"),
                ("artist", signed),
                ("albumartist", signed),
                ("release", "6bb7fcd3-fd2a-4cea-a0d0-05c16c6ef7c1"),
            ],
        );

        library(&pool, &root).await;
    }

    scan_all(&pool, Mode::Full, &Progress::default())
        .await
        .unwrap()
        .expect("the scan ran to the end");

    assert_eq!(
        count(&pool, "SELECT count(*) FROM albums").await,
        1,
        "one record, however many libraries hold a copy of it"
    );
    assert_eq!(
        signers(&pool).await,
        vec!["Fun".to_string(), "fun.".to_string()],
        "and it says what both copies say, not what the last library read"
    );
}

/// Who a record is credited to, by name.
async fn signers(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT ar.name FROM album_artists aa
           JOIN artists ar ON ar.id = aa.artist_id
          ORDER BY ar.name",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A year appearing on a file that had none joins the record rather than
/// splitting it — and now that the record outlives the scan that made it, so
/// does the case where the year arrives on a later scan.
#[tokio::test]
async fn a_year_tagged_afterwards_joins_the_album_it_belongs_to() {
    let root = temp_root("year-later");
    let dated = root.join("Album/01.wav");
    let undated = root.join("Album/02.wav");
    for path in [&dated, &undated] {
        write_wav(path);
        tag(
            path,
            &[("album", "Rumours"), ("albumartist", "Fleetwood Mac")],
        );
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(count(&pool, "SELECT count(*) FROM albums").await, 1);

    // Somebody fixed the tags of one file, so that one is read again and the
    // other is not.
    tag(
        &dated,
        &[
            ("album", "Rumours"),
            ("albumartist", "Fleetwood Mac"),
            ("year", "1977"),
        ],
    );

    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(
        count(&pool, "SELECT count(*) FROM albums").await,
        1,
        "the year filled in the album, it did not make another one"
    );
    assert_eq!(count(&pool, "SELECT year FROM albums").await, 1977);
}

#[tokio::test]
async fn a_changed_file_is_read_again() {
    let root = temp_root("changed");
    let path = root.join("Album/one.wav");
    write_wav(&path);

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // Growing the file changes its size, which is half of what is compared.
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(&[0u8; 64]);
    fs::write(&path, bytes).unwrap();

    let second = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(second.unchanged, 0, "the size changed, so it was reopened");
}

#[tokio::test]
async fn a_deleted_file_is_marked_not_removed() {
    let root = temp_root("deleted");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_file(root.join("Album/0.wav")).unwrap();

    let second = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(second.gone, 1);

    // Still there, just marked.
    assert_eq!(count(&pool, "SELECT count(*) FROM tracks").await, 5);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_file_that_comes_back_is_unmarked() {
    let root = temp_root("returns");
    let path = root.join("Album/one.wav");
    write_wav(&path);

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_file(&path).unwrap();
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        1
    );

    write_wav(&path);
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        0,
        "the file is back, so the mark comes off"
    );
}

/// The point of the whole marked-not-deleted design: a library reorganisation
/// must not cost the user their data.
#[tokio::test]
async fn a_moved_file_keeps_its_identity_and_user_data() {
    let root = temp_root("moved");
    let original = root.join("Wrong Folder/song.wav");
    write_wav(&original);

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let (track_id, public_id): (i64, String) =
        sqlx::query_as("SELECT id, public_id FROM tracks LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

    // Give the user something to lose.
    let timestamp = db::now();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, created_at, updated_at)
         VALUES (1, 'listener', 'hash', ?, ?)",
    )
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_track_stats (user_id, track_id, play_count, rating, starred_at)
         VALUES (1, ?, 42, 5, ?)",
    )
    .bind(track_id)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();

    // Reorganise: same file, new home.
    let moved = root.join("Right Folder/Album/01 song.wav");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&original, &moved).unwrap();

    let outcome = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(outcome.gone, 0, "nothing actually went away");

    let rows: Vec<(i64, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, public_id, path, missing_since FROM tracks")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(rows.len(), 1, "the move must not create a second row");
    assert_eq!(rows[0].0, track_id, "same row");
    assert_eq!(rows[0].1, public_id, "same identifier for the client");
    // Relative to the library root, which is the whole point of storing it that
    // way: the row says where the file is inside the library and not which
    // directory the library happened to be in.
    assert_eq!(rows[0].2, "Right Folder/Album/01 song.wav");
    assert!(
        !rows[0].2.starts_with('/'),
        "a stored path that is absolute would tie the row to this machine"
    );
    assert_eq!(rows[0].3, None, "not missing any more");

    let (plays, rating): (i64, i64) =
        sqlx::query_as("SELECT play_count, rating FROM user_track_stats WHERE track_id = ?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((plays, rating), (42, 5), "the user data came along");
}

/// An unmounted disk looks exactly like somebody deleting everything, and the
/// difference matters.
#[tokio::test]
async fn a_library_that_vanishes_wholesale_is_left_alone() {
    let root = temp_root("vanished");
    for n in 0..20 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // The mount point is there but empty, which is what a failed mount looks
    // like from up here.
    fs::remove_dir_all(root.join("Album")).unwrap();

    let outcome = scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(outcome.gone, 0, "the sweep refused to run");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        0,
        "nothing was marked"
    );
}

/// Below the minimum, the fraction means nothing: a two track library losing
/// both tracks is a deletion, not a failed mount.
#[tokio::test]
async fn a_tiny_library_is_still_swept() {
    let root = temp_root("tiny");
    write_wav(&root.join("Album/one.wav"));
    write_wav(&root.join("Album/two.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_file(root.join("Album/one.wav")).unwrap();
    fs::remove_file(root.join("Album/two.wav")).unwrap();

    let outcome = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
    assert_eq!(outcome.gone, 2);
}

#[tokio::test]
async fn a_folder_that_goes_away_is_marked_too() {
    let root = temp_root("folder-gone");
    write_wav(&root.join("Keep/one.wav"));
    write_wav(&root.join("Remove/two.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    fs::remove_dir_all(root.join("Remove")).unwrap();
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let marked: Vec<String> =
        sqlx::query_scalar("SELECT name FROM folders WHERE missing_since IS NOT NULL")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(marked, vec!["Remove".to_string()]);
}

#[test]
fn only_one_scan_can_hold_the_flag() {
    let progress = Progress::default();
    assert!(!progress.is_scanning());

    let running = progress.begin().expect("the first claim wins");
    assert!(progress.is_scanning());
    assert!(
        progress.begin().is_none(),
        "a second claim must be refused while the first is held"
    );

    drop(running);
    assert!(
        !progress.is_scanning(),
        "the flag clears when the scan ends"
    );
    assert!(progress.begin().is_some(), "and the next scan can claim it");
}

#[tokio::test]
async fn a_second_scan_request_while_one_runs_does_nothing() {
    let root = temp_root("concurrent");
    write_wav(&root.join("Album/one.wav"));

    let pool = database().await;
    library(&pool, &root).await;

    let progress = Progress::default();
    let _running = progress.begin().unwrap();

    // With the flag held, scan_all declines rather than queueing a second pass.
    let outcome = scan_all(&pool, Mode::Incremental, &progress).await.unwrap();
    assert!(outcome.is_none());

    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM scan_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 0, "it must not even record a run");
}

/// The case a stray retag uncovered: the year is part of the album key so an
/// original and its remaster stay apart, but a track missing the year must not
/// start an album of its own.
#[tokio::test]
async fn a_missing_year_does_not_split_an_album() {
    let root = temp_root("year");
    let with_year = root.join("Album/01.wav");
    let without = root.join("Album/02.wav");
    write_wav(&with_year);
    write_wav(&without);

    tag(
        &with_year,
        &[
            ("album", "The Wall"),
            ("albumartist", "Pink Floyd"),
            ("year", "1979"),
        ],
    );
    tag(
        &without,
        &[("album", "The Wall"), ("albumartist", "Pink Floyd")],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let albums: Vec<(String, Option<i64>)> = sqlx::query_as("SELECT name, year FROM albums")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(albums.len(), 1, "one album, not one per year: {albums:?}");
    assert_eq!(albums[0].1, Some(1979), "the known year survives");

    let counted: i64 = count(
        &pool,
        "SELECT count(*) FROM tracks WHERE album_id IS NOT NULL",
    )
    .await;
    assert_eq!(counted, 2, "both tracks belong to it");
}

/// And the other direction: the yearless track arriving first must not leave the
/// album without a year once one shows up.
#[tokio::test]
async fn a_year_arriving_late_is_filled_in() {
    let root = temp_root("year-late");
    let without = root.join("Album/01.wav");
    let with_year = root.join("Album/02.wav");
    write_wav(&without);
    write_wav(&with_year);

    tag(
        &without,
        &[("album", "Rumours"), ("albumartist", "Fleetwood Mac")],
    );
    tag(
        &with_year,
        &[
            ("album", "Rumours"),
            ("albumartist", "Fleetwood Mac"),
            ("year", "1977"),
        ],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let albums: Vec<(String, Option<i64>)> = sqlx::query_as("SELECT name, year FROM albums")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(albums.len(), 1, "still one album: {albums:?}");
    assert_eq!(
        albums[0].1,
        Some(1977),
        "the year turned up and was recorded"
    );
}

/// Two editions that both say which year they are do stay apart, which is the
/// reason the year is in the key at all.
#[tokio::test]
async fn two_tagged_years_remain_two_albums() {
    let root = temp_root("year-two");
    let original = root.join("Original/01.wav");
    let remaster = root.join("Remaster/01.wav");
    write_wav(&original);
    write_wav(&remaster);

    tag(
        &original,
        &[
            ("album", "Rumours"),
            ("albumartist", "Fleetwood Mac"),
            ("year", "1977"),
        ],
    );
    tag(
        &remaster,
        &[
            ("album", "Rumours"),
            ("albumartist", "Fleetwood Mac"),
            ("year", "2004"),
        ],
    );

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    assert_eq!(count(&pool, "SELECT count(*) FROM albums").await, 2);
}

/// The shape most hand-tagged music arrives in: an artist on every track and no
/// album artist anywhere. The record used to come out signed by nobody, which a
/// listing has nothing to print for and sorts under no name at all.
#[tokio::test]
async fn an_album_with_no_album_artist_is_credited_to_its_artist() {
    let root = temp_root("no-album-artist");
    let one = root.join("Album/01.wav");
    let two = root.join("Album/02.wav");
    write_wav(&one);
    write_wav(&two);

    for path in [&one, &two] {
        tag(
            path,
            &[
                ("album", "Selected Ambient Works"),
                ("artist", "Aphex Twin"),
            ],
        );
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let credited: Vec<String> = sqlx::query_scalar(
        "SELECT ar.name FROM album_artists aa
           JOIN artists ar ON ar.id = aa.artist_id
          WHERE aa.role = 'albumartist'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    // One row and not two: both tracks credit the same name, and the record is
    // signed once however many tracks arrive saying so.
    assert_eq!(
        credited,
        ["Aphex Twin".to_string()],
        "the name on the files signs the record"
    );

    // And it can be found by that name, which is the other half of a credit.
    let found = count(
        &pool,
        "SELECT count(*) FROM albums_fts WHERE albums_fts MATCH 'Aphex'",
    )
    .await;
    assert_eq!(found, 1, "the record answers to who made it");
}

#[tokio::test]
async fn an_interrupted_scan_writes_nothing() {
    let root = temp_root("interrupted");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;

    let progress = Progress::default();
    progress.cancel();

    let outcome = scan_library(
        &pool,
        id,
        &root,
        Mode::Incremental,
        &progress,
        &mut HashSet::new(),
    )
    .await
    .unwrap();

    assert!(outcome.is_none(), "the scan gave up rather than finishing");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM tracks").await,
        0,
        "the transaction was dropped, not committed"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM folders").await, 0);
}

#[tokio::test]
async fn an_interrupted_scan_marks_nothing_missing() {
    let root = temp_root("interrupted-sweep");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    // The danger the rollback exists for: a scan that stopped early has not seen
    // most of the library, and sweeping on the way out would call all of it gone.
    let progress = Progress::default();
    progress.cancel();
    scan_library(
        &pool,
        id,
        &root,
        Mode::Incremental,
        &progress,
        &mut HashSet::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn cancelling_one_scan_does_not_stop_the_next() {
    let root = temp_root("cancel-then-scan");
    for n in 0..5 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    library(&pool, &root).await;

    // Through scan_all, because the flag is cleared by the guard that ends a
    // scan and that guard is what scan_all holds. A cancelled run must not leave
    // the server refusing to scan again.
    let progress = Progress::default();
    progress.cancel();
    assert!(
        scan_all(&pool, Mode::Incremental, &progress)
            .await
            .unwrap()
            .is_none(),
        "the first scan gave up"
    );

    let outcome = scan_all(&pool, Mode::Incremental, &progress)
        .await
        .unwrap()
        .expect("the second scan ran to the end");

    assert_eq!(outcome.tracks, 5);
    assert_eq!(count(&pool, "SELECT count(*) FROM tracks").await, 5);
}

#[tokio::test]
async fn a_finished_scan_reports_what_it_did() {
    let root = temp_root("snapshot");
    for n in 0..3 {
        write_wav(&root.join(format!("Album/{n}.wav")));
    }

    let pool = database().await;
    library(&pool, &root).await;

    let progress = Progress::default();
    scan_all(&pool, Mode::Incremental, &progress).await.unwrap();

    let snapshot = progress.snapshot();
    assert!(!snapshot.scanning);
    assert!(!snapshot.cancelled);
    assert_eq!(snapshot.tracks, 3);
    // The root counts as a folder as well as the album inside it.
    assert_eq!(snapshot.folders, 2);
    assert!(snapshot.started_at.is_some());
    assert!(snapshot.finished_at.is_some());
    // Cleared on the way out: there is no library being walked any more.
    assert!(snapshot.library.is_none());
}

#[tokio::test]
async fn a_cancelled_scan_says_so() {
    let root = temp_root("snapshot-cancelled");
    write_wav(&root.join("Album/one.wav"));

    let pool = database().await;
    library(&pool, &root).await;

    let progress = Progress::default();
    progress.cancel();
    scan_all(&pool, Mode::Incremental, &progress).await.unwrap();

    assert!(progress.snapshot().cancelled, "the panel can tell");
}

#[tokio::test]
async fn the_environment_adds_libraries_without_removing_others() {
    let pool = database().await;

    // One that somebody added through the API, which the environment knows
    // nothing about.
    let timestamp = db::now();
    sqlx::query(
        "INSERT INTO libraries (name, path, enabled, created_at, updated_at)
         VALUES ('from the panel', '/srv/added', 1, ?, ?)",
    )
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();

    sync_libraries(&pool, &[PathBuf::from("/srv/configured")])
        .await
        .unwrap();

    let enabled: Vec<String> =
        sqlx::query_scalar("SELECT path FROM libraries WHERE enabled = 1 ORDER BY path")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(
        enabled,
        vec!["/srv/added".to_string(), "/srv/configured".to_string()],
        "the variable adds and enables; it does not decide what else may exist"
    );
}

#[tokio::test]
async fn the_environment_re_enables_what_it_names() {
    let pool = database().await;

    let timestamp = db::now();
    sqlx::query(
        "INSERT INTO libraries (name, path, enabled, created_at, updated_at)
         VALUES ('turned off', '/srv/music', 0, ?, ?)",
    )
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&pool)
    .await
    .unwrap();

    sync_libraries(&pool, &[PathBuf::from("/srv/music")])
        .await
        .unwrap();

    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM libraries WHERE path = '/srv/music'")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert!(enabled, "naming it in the variable turns it back on");
}

/// Moving a library is one row, and nothing else has to be touched.
///
/// This is what relative paths are for. Stored absolute, every track would name
/// the old directory and only a rescan could reconcile them one file at a time;
/// stored relative, the root is named once and changing it is the whole move.
#[tokio::test]
async fn moving_a_library_needs_no_rescan() {
    let root = std::env::temp_dir().join("tocata-scan-relocated");
    let elsewhere = std::env::temp_dir().join("tocata-scan-relocated-new");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&elsewhere);

    write_wav(&root.join("Artist/Album/01 song.wav"));

    let pool = database().await;
    let id = library(&pool, &root).await;
    scan(&pool, id, &root, Mode::Incremental).await.unwrap();

    let stored: String = sqlx::query_scalar("SELECT path FROM tracks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, "Artist/Album/01 song.wav");

    // The whole move: the directory on disk, and one column.
    fs::rename(&root, &elsewhere).unwrap();
    sqlx::query("UPDATE libraries SET path = ? WHERE id = ?")
        .bind(elsewhere.to_string_lossy().as_ref())
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    // No scan in between: the file is findable at once, because where it is has
    // always been "inside the library" and only the library moved.
    let composed: String = sqlx::query_scalar(
        "SELECT l.path || '/' || t.path FROM tracks t JOIN libraries l ON l.id = t.library_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        composed,
        elsewhere.join("Artist/Album/01 song.wav").to_string_lossy()
    );
    assert!(
        std::path::Path::new(&composed).exists(),
        "the composed path has to be the file that is actually there"
    );

    fs::remove_dir_all(&elsewhere).unwrap();
}

/// Reading everything again has to forget that a cover was looked for and not
/// found, or a cover added to a directory afterwards would never be seen: the
/// server remembers the answer and stops opening the files.
///
/// And the quick scan has to leave that memory alone, since the whole point of it
/// is to skip work.
#[tokio::test]
async fn reading_everything_again_forgets_the_covers_that_were_not_found() {
    let pool = database().await;

    for (id, found) in [(1, 0), (2, 1), (3, 0)] {
        sqlx::query(
            "INSERT INTO artwork_lookups (entity_type, entity_id, source, attempted_at, found)
             VALUES ('album', ?, 'local', ?, ?)",
        )
        .bind(id)
        .bind(db::now())
        .bind(found)
        .execute(&pool)
        .await
        .unwrap();
    }

    // No library, so neither scan has a file to open. What is being tested
    // happens before the walk.
    scan_all(&pool, Mode::Incremental, &Progress::default())
        .await
        .unwrap();

    assert_eq!(lookups(&pool).await, 3, "a quick scan remembers everything");

    scan_all(&pool, Mode::Full, &Progress::default())
        .await
        .unwrap();

    assert_eq!(lookups(&pool).await, 1, "only the one that found a cover");
}

async fn lookups(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM artwork_lookups")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A file that will not open, and the two ways that used to go wrong.
///
/// Found on a real server: the tags of some files were edited, the scan that
/// followed could not read them — their permissions had gone — and afterwards they
/// sat in the listing with nothing on them. Giving the permissions back and scanning
/// again changed nothing, because from outside the files had not changed.
mod a_file_that_will_not_open {
    use super::*;

    /// Everything about the track, as a test wants to read it.
    async fn told(pool: &SqlitePool) -> (String, Option<i64>, Option<i64>, Option<String>) {
        sqlx::query_as("SELECT title, album_id, duration_ms, unreadable_since FROM tracks LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Takes away the permission to read a file, and gives it back.
    fn readable(path: &Path, may: bool) {
        use std::os::unix::fs::PermissionsExt;

        let mode = if may { 0o644 } else { 0o000 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The damage that was being done: a track scanned correctly, then scanned again
    /// while unreadable, lost its title to its file name and everything else to null.
    /// A scan was destroying the answer it could no longer see.
    #[tokio::test]
    async fn it_keeps_what_was_already_known_about_it() {
        let root = temp_root("unreadable-keeps");
        let path = root.join("Album/one.wav");
        write_wav(&path);
        tag(&path, &[("title", "Trains"), ("album", "In Absentia")]);

        let pool = database().await;
        let id = library(&pool, &root).await;
        scan(&pool, id, &root, Mode::Incremental).await.unwrap();

        let (title, album, length, _) = told(&pool).await;
        assert_eq!(title, "Trains");
        assert!(album.is_some() && length.is_some());

        readable(&path, false);
        let second = scan(&pool, id, &root, Mode::Full).await.unwrap();
        readable(&path, true);

        assert_eq!(second.failed, 1, "it could not be read");

        let (title, album_again, length_again, since) = told(&pool).await;
        assert_eq!(title, "Trains", "not the file name");
        assert_eq!(album_again, album, "still on its record");
        assert_eq!(length_again, length);
        assert!(since.is_some(), "and it says it could not be read");
    }

    /// The other half, and the one that made it permanent: size and modification
    /// time are how the file looks from outside, and from outside nothing changed
    /// when the permissions came back. A quick scan skipped it for ever.
    #[tokio::test]
    async fn it_is_read_again_by_the_next_quick_scan() {
        let root = temp_root("unreadable-retried");
        let path = root.join("Album/one.wav");
        write_wav(&path);

        let pool = database().await;
        let id = library(&pool, &root).await;

        readable(&path, false);
        scan(&pool, id, &root, Mode::Incremental).await.unwrap();
        readable(&path, true);

        let (_, _, _, since) = told(&pool).await;
        assert!(since.is_some(), "the first scan could not read it");

        // Nothing about the file has changed. Only the note on it says otherwise.
        let again = scan(&pool, id, &root, Mode::Incremental).await.unwrap();

        assert_eq!(again.unchanged, 0, "it was reopened rather than skipped");
        assert_eq!(again.failed, 0, "and this time it could be read");

        let (_, _, length, since) = told(&pool).await;
        assert!(length.is_some(), "so its length is known now");
        assert_eq!(since, None, "and the note is gone");
    }

    /// And why, which is the half that used to go to the log and nowhere else.
    ///
    /// Two shapes, because they are answered by two different things. A file the
    /// server may not open is said in words, with what to change, and the reader is
    /// never asked — it would have called it a parse failure like any other, which
    /// is the least useful thing anybody could be told about a `chmod`. A file that
    /// opens and will not parse is the reader's own sentence, cryptic and true and
    /// not ours to rewrite.
    #[tokio::test]
    async fn it_says_why_and_stops_saying_it_when_the_file_reads() {
        let root = temp_root("unreadable-why");
        let shut = root.join("Album/shut.wav");
        let damaged = root.join("Album/damaged.wav");
        write_wav(&shut);

        // Not audio at all. lofty opens it, finds no container it knows and says so,
        // which is the ordinary shape of everything that is not about permissions.
        fs::create_dir_all(damaged.parent().unwrap()).unwrap();
        fs::write(&damaged, b"this is not a wav file").unwrap();

        let pool = database().await;
        let id = library(&pool, &root).await;

        readable(&shut, false);
        let first = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
        readable(&shut, true);

        assert_eq!(first.failed, 2, "neither of them read");

        let why = |name: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Option<String>>(
                    "SELECT unreadable_error FROM tracks WHERE path = ?",
                )
                .bind(format!("Album/{name}"))
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };

        let shut_says = why("shut.wav").await.expect("it says why");
        assert!(
            shut_says.starts_with("Tocata is not allowed to read this file"),
            "said in words, not as whatever the tag reader made of it: {shut_says}"
        );
        assert!(
            shut_says.contains("permissions are 0000"),
            "with the thing to go and change in it: {shut_says}"
        );

        let damaged_says = why("damaged.wav").await.expect("it says why");
        assert!(
            !damaged_says.contains(&root.display().to_string()),
            "the innermost cause and not the chain around it, which repeats the path \
             the row already leads with: {damaged_says}"
        );

        // Read this time, so there is nothing left to explain.
        let again = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
        assert_eq!(again.failed, 1, "only the one that is genuinely not audio");
        assert_eq!(
            why("shut.wav").await,
            None,
            "a reason left behind on a file that reads is a reason that lies"
        );
    }

    /// And the optimisation it must not cost: a file that was read fine and has not
    /// changed is still not opened again. Losing this would mean rereading a whole
    /// library on every quick scan.
    #[tokio::test]
    async fn a_file_that_was_read_fine_is_still_skipped() {
        let root = temp_root("unreadable-not-everything");
        let path = root.join("Album/one.wav");
        write_wav(&path);

        let pool = database().await;
        let id = library(&pool, &root).await;
        scan(&pool, id, &root, Mode::Incremental).await.unwrap();

        let again = scan(&pool, id, &root, Mode::Incremental).await.unwrap();
        assert_eq!(again.unchanged, 1);
    }
}
