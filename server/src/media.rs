// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Finding a file in a library and handing it over.
//!
//! Both APIs serve the same bytes from the same disk under the same rules — who
//! may see which library, and never a path that climbs out of one — so both come
//! through here. What differs is only how the caller proved who they are, and
//! that has been settled by the time anything below is called.
//!
//! Cover art is here for the same reason, and because it is not simply a file:
//! it is looked for inside the music, then beside it, and what is found is
//! cached and remembered — including the finding of nothing, so the next request
//! does not open twenty files again to reach the same answer.

use crate::artwork;
use crate::db;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use sqlx::SqlitePool;
use std::path::{Component, Path, PathBuf};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::{error, warn};

/// Where a track lives, and the library root it must live under.
pub(crate) struct Located {
    pub path: PathBuf,
    pub content_type: String,
    library_root: PathBuf,
}

pub(crate) enum Refused {
    /// The stored path resolved outside its library. Answered as not found:
    /// whoever asked has no business learning the difference.
    Traversal,
    Database(sqlx::Error),
}

pub(crate) async fn locate(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
) -> Result<Option<Located>, Refused> {
    let row: Option<(String, String, String)> = sqlx::query_as(concat!(
        visible_libraries!(),
        " SELECT t.path, t.content_type, l.path
            FROM tracks t
            JOIN libraries l ON l.id = t.library_id
           WHERE t.public_id = ? AND t.missing_since IS NULL
             AND t.library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(user_id)
    .bind(public_id)
    .fetch_optional(pool)
    .await
    .map_err(Refused::Database)?;

    let Some((relative, content_type, library_root)) = row else {
        return Ok(None);
    };

    let library_root = PathBuf::from(library_root);
    let path = library_root.join(&relative);

    // Defence in depth, and a deliberate exception to not writing guards for
    // conditions that cannot arise today.
    //
    // What the database holds is relative to the root, so the only way out of the
    // library is a stored path that climbs with `..`. Nothing user supplied gets
    // there: it comes from the scanner, which walks real directory entries and
    // skips symlinks. But this is the one place in the program that opens an
    // arbitrary file from disk and hands it to whoever asked, so the cost of
    // being wrong is serving /etc/passwd while the cost of the check is comparing
    // two prefixes. That asymmetry is what justifies it: the day somebody adds a
    // way to register a track by hand, or decides to follow symlinks, this is
    // already here.
    if !is_inside(&path, &library_root) {
        warn!(
            "refusing {}: it resolves outside its library root {}",
            path.display(),
            library_root.display()
        );
        return Err(Refused::Traversal);
    }

    Ok(Some(Located {
        path,
        content_type,
        library_root,
    }))
}

pub(crate) async fn serve(track: Located, request: Request) -> Response {
    // ServeFile brings range requests with it, which is what lets a client
    // seek into the middle of a song instead of fetching the whole thing, and
    // what makes a browser's audio element work at all.
    let service = ServeFile::new_with_mime(
        &track.path,
        &track
            .content_type
            .parse()
            .unwrap_or(mime::APPLICATION_OCTET_STREAM),
    );

    match service.oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            error!(
                "serving {} from {}: {e}",
                track.path.display(),
                track.library_root.display()
            );
            // A bare status, and no envelope of either API's: what failed is the
            // handing over of a file, and the caller is a browser or a player
            // reading bytes rather than a body.
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Whether `path` sits under `root` once both are normalised.
///
/// Normalises rather than calling `canonicalize`: resolving on disk would
/// follow symlinks, which is the opposite of what a containment check wants,
/// and it would touch the filesystem for every request.
fn is_inside(path: &Path, root: &Path) -> bool {
    let path = normalise(path);
    let root = normalise(root);

    !root.as_os_str().is_empty() && path.starts_with(&root)
}

/// Resolves `.` and `..` textually, without consulting the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Refuses to climb above the start, so a path made of nothing
                // but `..` cannot escape by arithmetic.
                out.pop();
            }
            other => out.push(other),
        }
    }

    out
}

/// The album a cover art id refers to. Clients pass an album id, and some pass a
/// song id, so both resolve.
/// The album a cover art identifier refers to, if the person asking may see it.
///
/// This is the one gate for cover art: everything after it works on an album
/// identifier and no longer asks who wanted it.
pub(crate) async fn resolve_album(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let by_album: Option<i64> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        " SELECT id FROM albums WHERE public_id = ? AND ",
        album_is_visible!("albums.id")
    ))
    .bind(user_id)
    .bind(public_id)
    .fetch_optional(pool)
    .await?;

    if let Some(id) = by_album {
        return Ok(Some(id));
    }

    // Clients also ask for a track's cover, meaning its album's.
    sqlx::query_scalar(concat!(
        visible_libraries!(),
        " SELECT album_id FROM tracks
           WHERE public_id = ? AND album_id IS NOT NULL
             AND missing_since IS NULL
             AND library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(user_id)
    .bind(public_id)
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

/// The cover already extracted for this album, if there is one.
pub(crate) async fn cover_in_cache(
    pool: &SqlitePool,
    album_id: i64,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT aw.content_hash, aw.mime_type
           FROM albums al JOIN artworks aw ON aw.id = al.artwork_id
          WHERE al.id = ?",
    )
    .bind(album_id)
    .fetch_optional(pool)
    .await
}

/// Finds a cover for an album and puts it in the cache.
///
/// Tries the embedded picture of each of its tracks in turn, then a file beside
/// them. A failure to find one is remembered, because a client scrolling a list
/// of albums asks again for every one of them on every scroll, and reopening the
/// files each time to learn the same nothing is the cost this avoids.
pub(crate) async fn extract_cover(
    pool: &SqlitePool,
    data_dir: &std::path::Path,
    album_id: i64,
) -> anyhow::Result<Option<(String, String)>> {
    if searched_before(pool, "album", album_id).await? {
        return Ok(None);
    }

    // No library filter here, and that is deliberate. This fills a cache keyed by
    // album, shared by everybody, so its contents must not depend on who asked
    // first. Whether this album may be seen at all was settled before we got
    // here, when its identifier was resolved.
    // Composed here rather than stored composed: what the row holds is relative
    // to the library, so its root comes along.
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT l.path || '/' || t.path
           FROM tracks t
           JOIN libraries l ON l.id = t.library_id
          WHERE t.album_id = ? AND t.missing_since IS NULL
          ORDER BY t.disc_number, t.track_number
          LIMIT 20",
    )
    .bind(album_id)
    .fetch_all(pool)
    .await?;

    let found = tokio::task::spawn_blocking(move || find_cover(&paths)).await?;

    let Some((source, source_ref, bytes)) = found else {
        remember_nothing(pool, "album", album_id).await?;
        return Ok(None);
    };

    // Trust the bytes, not the extension or what a tag claims the type is.
    let Some(mime_type) = artwork::mime_of(&bytes) else {
        remember_nothing(pool, "album", album_id).await?;
        return Ok(None);
    };

    let hash = artwork::store(data_dir, &bytes)?;
    let timestamp = db::now();

    let mut tx = pool.begin().await?;

    // The hash is the identity, so two albums sharing a cover share the row.
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM artworks WHERE content_hash = ? LIMIT 1")
            .bind(&hash)
            .fetch_optional(&mut *tx)
            .await?;

    let artwork_id = match existing {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "INSERT INTO artworks (
                 public_id, kind, source, source_ref, mime_type, content_hash, fetched_at
             ) VALUES (?, 'album_front', ?, ?, ?, ?, ?)
             RETURNING id",
            )
            .bind(db::public_id()?)
            .bind(source)
            .bind(&source_ref)
            .bind(mime_type)
            .bind(&hash)
            .bind(&timestamp)
            .fetch_one(&mut *tx)
            .await?
        }
    };

    sqlx::query("UPDATE albums SET artwork_id = ? WHERE id = ? AND artwork_id IS NULL")
        .bind(artwork_id)
        .bind(album_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(Some((hash, mime_type.to_string())))
}

/// Blocking: opens files. Reads the artwork this time, unlike a scan.
fn find_cover(paths: &[String]) -> Option<(&'static str, Option<String>, Vec<u8>)> {
    for path in paths {
        let path = std::path::Path::new(path);
        if let Ok(metadata) = crate::scanner::read_tags_with_cover_art(path)
            && let Some(bytes) = metadata.picture
        {
            return Some(("embedded", None, bytes));
        }
    }

    // No track carried one, so look for a file next to the music.
    let directory = std::path::Path::new(paths.first()?).parent()?;
    artwork::find_near(directory).map(|(path, bytes)| {
        (
            "local_file",
            Some(path.to_string_lossy().to_string()),
            bytes,
        )
    })
}

async fn searched_before(pool: &SqlitePool, kind: &str, id: i64) -> Result<bool, sqlx::Error> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM artwork_lookups
          WHERE entity_type = ? AND entity_id = ? AND source = 'local'",
    )
    .bind(kind)
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(found.is_some())
}

async fn remember_nothing(pool: &SqlitePool, kind: &str, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO artwork_lookups (entity_type, entity_id, source, attempted_at, found)
         VALUES (?, ?, 'local', ?, 0)
         ON CONFLICT (entity_type, entity_id, source) DO UPDATE SET
             attempted_at = excluded.attempted_at",
    )
    .bind(kind)
    .bind(id)
    .bind(db::now())
    .execute(pool)
    .await?;

    Ok(())
}

/// The picture of an artist, cached the way a cover is.
///
/// Nothing puts these there but this: the scanner reads music, and a photograph
/// of a band is not in a track's tags. So it is looked for the first time
/// somebody asks, and the answer — including "there is none" — is remembered.
///
/// Local files only. The conventions are the ones the tools that fetch these
/// already write, so a collection kept by Lidarr or beets has them on disk
/// already. Reaching out to the network for the rest is a decision of its own,
/// and it is not made here.
pub(crate) async fn artist_picture(
    pool: &SqlitePool,
    data_dir: &Path,
    artist_id: i64,
) -> Result<Option<(String, String)>, anyhow::Error> {
    let held: Option<(String, String)> = sqlx::query_as(
        "SELECT w.content_hash, w.mime_type
           FROM artists a JOIN artworks w ON w.id = a.artwork_id
          WHERE a.id = ?",
    )
    .bind(artist_id)
    .fetch_optional(pool)
    .await?;

    if let Some(found) = held {
        return Ok(Some(found));
    }

    if searched_before(pool, "artist", artist_id).await? {
        return Ok(None);
    }

    let Some(directory) = where_their_records_are(pool, artist_id).await? else {
        remember_nothing(pool, "artist", artist_id).await?;
        return Ok(None);
    };

    // Opening files is the filesystem's business rather than the executor's.
    let found = tokio::task::spawn_blocking(move || {
        artwork::find_named(Path::new(&directory), artwork::ARTIST_FILE_STEMS)
    })
    .await?;

    let Some((path, bytes)) = found else {
        remember_nothing(pool, "artist", artist_id).await?;
        return Ok(None);
    };

    // Trust the bytes rather than the extension: an `artist.jpg` that is really
    // a text file is not a picture.
    let Some(mime_type) = artwork::mime_of(&bytes) else {
        remember_nothing(pool, "artist", artist_id).await?;
        return Ok(None);
    };

    let hash = artwork::store(data_dir, &bytes)?;
    let timestamp = db::now();

    let mut tx = pool.begin().await?;

    // The hash is the identity, so a picture already known under another row is
    // that row rather than a second copy of it.
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM artworks WHERE content_hash = ? LIMIT 1")
            .bind(&hash)
            .fetch_optional(&mut *tx)
            .await?;

    let artwork_id = match existing {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "INSERT INTO artworks (public_id, kind, source, source_ref, mime_type,
                                       content_hash, fetched_at)
                 VALUES (?, 'artist', 'local_file', ?, ?, ?, ?)
                 RETURNING id",
            )
            .bind(db::public_id()?)
            .bind(path.to_string_lossy().to_string())
            .bind(mime_type)
            .bind(&hash)
            .bind(&timestamp)
            .fetch_one(&mut *tx)
            .await?
        }
    };

    sqlx::query("UPDATE artists SET artwork_id = ? WHERE id = ? AND artwork_id IS NULL")
        .bind(artwork_id)
        .bind(artist_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(Some((hash, mime_type.to_string())))
}

/// The directory an artist's records sit in, if there is one worth calling that.
///
/// Worked out rather than stored, because nothing stores it: what the scanner
/// knows is where each file is. In a collection laid out the usual way — a
/// directory per artist holding a directory per record — the parent of the
/// album directories is the artist's, and that is where the tools that fetch
/// pictures leave them.
///
/// The commonest parent rather than the only one, so a guest appearance on
/// somebody else's compilation does not stop a band having a photograph. And
/// nothing at all when their music is scattered, which is the honest answer:
/// there is no such directory to look in.
async fn where_their_records_are(
    pool: &SqlitePool,
    artist_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT l.path || '/' || p.path
           FROM tracks t
           JOIN track_artists ta ON ta.track_id = t.id
           JOIN folders f ON f.id = t.folder_id
           JOIN folders p ON p.id = f.parent_id
           JOIN libraries l ON l.id = t.library_id
          WHERE ta.artist_id = ? AND t.missing_since IS NULL
          GROUP BY p.id
          ORDER BY count(*) DESC
          LIMIT 1",
    )
    .bind(artist_id)
    .fetch_optional(pool)
    .await
}

/// The artist an identifier refers to, if the person asking may see them.
pub(crate) async fn resolve_artist(
    pool: &SqlitePool,
    user_id: i64,
    public_id: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(concat!(
        visible_libraries!(),
        " SELECT id FROM artists WHERE public_id = ? AND ",
        artist_is_visible!("artists.id")
    ))
    .bind(user_id)
    .bind(public_id)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_path_under_the_root_is_allowed() {
        assert!(is_inside(
            Path::new("/music/Queen/song.flac"),
            Path::new("/music")
        ));
        assert!(is_inside(
            Path::new("/music/song.flac"),
            Path::new("/music")
        ));
    }

    #[test]
    fn climbing_out_of_the_root_is_refused() {
        assert!(!is_inside(
            Path::new("/music/../etc/passwd"),
            Path::new("/music")
        ));
        assert!(!is_inside(
            Path::new("/music/Queen/../../etc/passwd"),
            Path::new("/music")
        ));
        assert!(!is_inside(Path::new("/etc/passwd"), Path::new("/music")));
    }

    #[test]
    fn a_sibling_directory_sharing_a_prefix_is_not_inside() {
        // "/music-private" starts with "/music" as a string but is a different
        // directory, which a naive string comparison would let through.
        assert!(!is_inside(
            Path::new("/music-private/secret.flac"),
            Path::new("/music")
        ));
    }

    #[test]
    fn current_directory_components_do_not_confuse_it() {
        assert!(is_inside(
            Path::new("/music/./Queen/./song.flac"),
            Path::new("/music")
        ));
    }

    #[test]
    fn an_empty_root_allows_nothing() {
        assert!(!is_inside(Path::new("/music/song.flac"), Path::new("")));
    }

    #[test]
    fn a_path_of_nothing_but_parents_cannot_escape() {
        assert!(!is_inside(
            Path::new("../../../etc/passwd"),
            Path::new("/music")
        ));
    }

    /// The directory a band's records sit in is worked out from where their
    /// files are, since nothing stores it. A guest appearance somewhere else
    /// must not be what decides it.
    #[tokio::test]
    async fn an_artist_is_where_most_of_their_music_is() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'm', '/m', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        // Two of their own records under one directory, and one appearance on a
        // compilation filed somewhere else entirely.
        for (id, parent, path) in [
            (1i64, None, "Triana"),
            (2, Some(1i64), "Triana/El Patio"),
            (3, Some(1), "Triana/Hijos del Agobio"),
            (4, None, "Varios"),
            (5, Some(4), "Varios/Rock Andaluz"),
        ] {
            sqlx::query(
                "INSERT INTO folders (id, public_id, library_id, parent_id, name, path,
                                      last_seen_scan)
                 VALUES (?, ?, 1, ?, ?, ?, 1)",
            )
            .bind(id)
            .bind(format!("f{id}"))
            .bind(parent)
            .bind(path)
            .bind(path)
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (1, 'ar1', 'Triana', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();

        for (id, folder) in [(1i64, 2i64), (2, 2), (3, 3), (4, 5)] {
            sqlx::query(
                "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                     file_modified_at, content_type, suffix, title,
                                     last_seen_scan, created_at, updated_at)
                 VALUES (?, ?, 1, ?, ?, 1, ?, 'audio/wav', 'wav', 'x', 1, ?, ?)",
            )
            .bind(id)
            .bind(format!("t{id}"))
            .bind(folder)
            .bind(format!("{id}.wav"))
            .bind(&at)
            .bind(&at)
            .bind(&at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO track_artists (track_id, artist_id, role, position)
                 VALUES (?, 1, 'artist', 0)",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }

        assert_eq!(
            where_their_records_are(&pool, 1).await.unwrap().as_deref(),
            Some("/m/Triana"),
            "the parent of their own records, not the compilation's"
        );
    }

    /// Music scattered across a library has no directory to call the artist's,
    /// and saying so is better than picking one.
    #[tokio::test]
    async fn music_at_the_root_of_a_library_has_no_artist_directory() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let at = db::now();
        sqlx::query(
            "INSERT INTO libraries (id, name, path, created_at, updated_at)
             VALUES (1, 'm', '/m', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO folders (id, public_id, library_id, name, path, last_seen_scan)
             VALUES (1, 'f1', 1, 'root', '', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO artists (id, public_id, name, created_at, updated_at)
             VALUES (1, 'ar1', 'Nadie', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tracks (id, public_id, library_id, folder_id, path, file_size,
                                 file_modified_at, content_type, suffix, title, last_seen_scan,
                                 created_at, updated_at)
             VALUES (1, 't1', 1, 1, 'x.wav', 1, ?, 'audio/wav', 'wav', 'x', 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .bind(&at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO track_artists (track_id, artist_id, role, position)
             VALUES (1, 1, 'artist', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(where_their_records_are(&pool, 1).await.unwrap(), None);
    }
}
