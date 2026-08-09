// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Cover images, kept on disk rather than in the database.
//!
//! The database holds a row describing each image; the bytes live in a
//! directory named after their own hash. That gets deduplication for free: every
//! track of an album carries the same embedded picture, and they all resolve to
//! one file.
//!
//! **Two directories, not one**, and what separates them is not what the picture
//! is but what it would cost to have it again. Anything read out of the user's
//! own files is a cache: delete it and the next request reads the file again for
//! nothing, which is why the job that tidies it can sweep by hash and delete
//! whatever no row names. Anything off the network is not: it is two or three
//! requests to somebody else's server at one a second, the file is the only copy
//! there is, and a sweep by hash cannot tell it from rubbish. So it lives
//! somewhere that sweep does not walk.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Subdirectory of the data directory where image bytes go.
pub const CACHE_DIRECTORY: &str = "artwork";

/// And where the ones that came off the network go, which is not the same place.
///
/// The difference is what it costs to have them again. Everything in the cache
/// can be read back out of the user's own files for nothing, so the job that
/// tidies it deletes anything no row names and never has to be careful. A
/// portrait fetched from Commons is two or three requests at one a second, and
/// the file is the only copy there is: a sweep by hash cannot tell it from
/// rubbish, so it must not be able to reach it.
pub const ACQUIRED_DIRECTORY: &str = "acquired";

/// What `source` says for a picture that came from Wikimedia Commons.
pub const FROM_COMMONS: &str = "commons";

/// Whether a row's source means the bytes cost a trip to somebody else's server.
///
/// The one question every caller that deletes has to ask, so it is asked in one
/// place: a source this does not know is treated as local, because deleting is
/// the half of this that cannot be undone.
pub fn acquired(source: &str) -> bool {
    source == FROM_COMMONS
}

/// File names that hold an album cover, in the order they are trusted. These are
/// the conventions taggers and other music software have settled on.
pub const COVER_FILE_STEMS: &[&str] = &["cover", "folder", "front", "album", "albumart"];

/// Extensions accepted for a cover found on disk.
pub const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// File names that hold a picture of an artist, in the order they are trusted.
///
/// The same conventions the tools that fetch these already write: Lidarr and
/// beets leave `artist.jpg` in the directory a band's records sit in, and
/// `folder.jpg` is what Windows and Kodi have always called the image of a
/// directory. Not `fanart`, which is a wide backdrop rather than a portrait and
/// would look wrong in a round frame.
pub const ARTIST_FILE_STEMS: &[&str] = &["artist", "folder", "poster"];

/// Where the bytes of a cached image with this hash live.
pub fn cache_path(data_dir: &Path, hash: &str) -> PathBuf {
    inside(data_dir, CACHE_DIRECTORY, hash)
}

/// And where a fetched one lives, under the same layout in the other directory.
pub fn acquired_path(data_dir: &Path, hash: &str) -> PathBuf {
    inside(data_dir, ACQUIRED_DIRECTORY, hash)
}

/// Where an image with this hash actually is, wherever that turns out to be.
///
/// For everybody who reads rather than writes, so serving a picture does not
/// mean carrying its row's source down through four signatures to decide which
/// of two directories to open. Whoever deletes does have to know, and asks.
///
/// The fetched one wins a tie, which cannot happen: same hash is same bytes, so
/// either answer would serve the same picture.
pub fn path_of(data_dir: &Path, hash: &str) -> PathBuf {
    let fetched = acquired_path(data_dir, hash);

    if fetched.exists() {
        return fetched;
    }

    cache_path(data_dir, hash)
}

fn inside(data_dir: &Path, directory: &str, hash: &str) -> PathBuf {
    // Two levels of fan out, so a library with thousands of covers does not put
    // thousands of entries in one directory.
    let (prefix, rest) = hash.split_at(2.min(hash.len()));
    data_dir.join(directory).join(prefix).join(rest)
}

/// Bytes that are on disk, and the promise to take them off again if the row
/// that was going to name them never gets written.
///
/// The reason this is a value rather than a hash is an order that cannot be
/// changed. The file has to exist before the row, because a row promising a file
/// that is not there is worse than a file nothing names: the first is a picture
/// that fails to load, the second is a few kilobytes. But between the two there
/// is a transaction that can fail, and what that used to leave behind was a file
/// nothing would ever name again — measured on a real server, eleven of them,
/// from a shelf of records opened on a cold cache.
///
/// So this cleans up after itself unless it is told the row exists. A guard
/// rather than a call at the end of the happy path, because the failures here
/// are `?` on half a dozen lines and the next one somebody adds would be a leak
/// again.
///
/// Only a file this call created. One that was already there belongs to whoever
/// wrote it, and their row still names it.
#[must_use = "the file is deleted again unless the row that names it is written"]
pub struct Written {
    path: PathBuf,
    hash: String,
    fresh: bool,
    kept: bool,
}

impl Written {
    /// What the bytes are called, for the row that is about to name them.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Says the row exists now, so the file stays.
    pub fn kept(mut self) -> String {
        self.kept = true;
        self.hash.clone()
    }
}

impl Drop for Written {
    fn drop(&mut self) {
        if !self.fresh || self.kept {
            return;
        }

        if let Err(e) = std::fs::remove_file(&self.path) {
            // Worth saying and not worth failing over: what is left behind is a
            // file the covers job will sweep the next time somebody runs it.
            tracing::warn!(
                "could not take back {} after nothing came to name it: {e}",
                self.path.display()
            );
        }

        if let Some(fan) = self.path.parent() {
            let _ = std::fs::remove_dir(fan);
        }
    }
}

/// Writes image bytes into the cache.
///
/// Writing the same image twice is not an error and not extra work: the second
/// call finds the file already there.
pub fn store(data_dir: &Path, bytes: &[u8]) -> Result<Written> {
    write_into(cache_path(data_dir, &hash_of(bytes)), bytes)
}

/// The same, for bytes that came off the network and go where no sweep reaches.
pub fn acquire(data_dir: &Path, bytes: &[u8]) -> Result<Written> {
    write_into(acquired_path(data_dir, &hash_of(bytes)), bytes)
}

fn write_into(path: PathBuf, bytes: &[u8]) -> Result<Written> {
    let hash = hash_of(bytes);

    if path.exists() {
        return Ok(Written {
            path,
            hash,
            fresh: false,
            kept: false,
        });
    }

    let parent = path
        .parent()
        .context("an artwork path always has a parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    std::fs::write(&path, bytes)
        .with_context(|| format!("writing artwork to {}", path.display()))?;

    Ok(Written {
        path,
        hash,
        fresh: true,
        kept: false,
    })
}

pub fn hash_of(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Recognises an image from its leading bytes.
///
/// The extension a file happens to carry, or the type a tag claims, are both
/// hearsay. What matters for the response header is what the bytes actually are.
pub fn mime_of(bytes: &[u8]) -> Option<&'static str> {
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    const GIF: &[u8] = b"GIF8";
    const BMP: &[u8] = b"BM";

    if bytes.starts_with(JPEG) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(PNG) {
        return Some("image/png");
    }
    if bytes.starts_with(GIF) {
        return Some("image/gif");
    }
    if bytes.starts_with(BMP) {
        return Some("image/bmp");
    }
    // WebP is a RIFF container with a WEBP tag four bytes further in.
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }

    None
}

/// Names that mean "this directory is one disc of a set", so the album's cover
/// lives a level up.
const DISC_PREFIXES: &[&str] = &["cd", "disc", "disk"];

/// Looks for a cover image beside the music, and one level up when the tracks
/// sit in a disc subdirectory.
///
/// The second part matters more than it looks: an album split into `CD1` and
/// `CD2` normally keeps one `cover.jpg` in the album directory, and without
/// this it would have no cover at all. It climbs only from a directory that
/// names itself a disc, because climbing unconditionally would reach the
/// artist directory, where an image is a photo of the artist and not the cover
/// of every album they made.
pub fn find_near(directory: &Path) -> Option<(PathBuf, Vec<u8>)> {
    if let Some(found) = find_in_directory(directory) {
        return Some(found);
    }

    if looks_like_a_disc(directory) {
        return find_in_directory(directory.parent()?);
    }

    None
}

fn looks_like_a_disc(directory: &Path) -> bool {
    let Some(name) = directory.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let name = name.trim().to_lowercase();

    DISC_PREFIXES.iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|rest| {
            // "cd1", "cd 1", "disc-2": whatever follows has to be a number, so
            // a band called "Discipline" does not qualify.
            let rest = rest.trim_start_matches([' ', '-', '_', '.']);
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
        })
    })
}

/// Looks for a cover image in one directory.
///
/// Reads the directory once and matches names case insensitively, rather than
/// trying a list of candidate names: `Cover.jpg` and `FOLDER.JPG` are as common
/// as the lowercase spellings, and on a case sensitive filesystem building the
/// names by hand misses them.
///
/// Returns the first name that is really an image, in the order of preference
/// above, so a `cover.jpg` that is actually a text file does not become an album
/// cover.
pub fn find_in_directory(directory: &Path) -> Option<(PathBuf, Vec<u8>)> {
    find_named(directory, COVER_FILE_STEMS)
}

/// The same, for whatever a caller calls its images.
pub fn find_named(directory: &Path, stems: &[&str]) -> Option<(PathBuf, Vec<u8>)> {
    let entries = std::fs::read_dir(directory).ok()?;

    // Collected by stem so preference wins over directory order.
    let mut candidates: Vec<(usize, PathBuf)> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        let stem = stem.to_lowercase();
        let extension = extension.to_lowercase();
        if !COVER_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }

        if let Some(rank) = stems.iter().position(|s| *s == stem) {
            candidates.push((rank, path));
        }
    }

    candidates.sort_by_key(|(rank, _)| *rank);

    for (_, path) in candidates {
        if let Ok(bytes) = std::fs::read(&path)
            && mime_of(&bytes).is_some()
        {
            return Some((path, bytes));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG signature with a body, enough for the sniffing to accept it.
    fn png() -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(b"body");
        bytes
    }

    fn jpeg() -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
        bytes.extend_from_slice(b"padding to make it look like a file");
        bytes
    }

    /// What decides which of the two directories a picture is in, and that
    /// reading finds it either way — the reader is the one caller that must not
    /// have to know.
    #[test]
    fn what_cost_a_walk_is_kept_where_a_sweep_cannot_reach_it() {
        let data_dir = crate::fixtures::temp_root("artwork-two-homes");

        let cached = store(&data_dir, &jpeg()).unwrap().kept();
        let fetched = acquire(&data_dir, &png()).unwrap().kept();

        assert!(cache_path(&data_dir, &cached).exists());
        assert!(acquired_path(&data_dir, &fetched).exists());
        assert!(
            !cache_path(&data_dir, &fetched).exists(),
            "the fetched one is not where the sweep walks"
        );

        // And whoever serves an image finds it without being told which it is.
        assert_eq!(path_of(&data_dir, &cached), cache_path(&data_dir, &cached));
        assert_eq!(
            path_of(&data_dir, &fetched),
            acquired_path(&data_dir, &fetched)
        );
    }

    /// The file goes away again if nothing comes to name it, which is what a
    /// transaction that failed used to leave behind: a few kilobytes that no row
    /// would ever point at and only a sweep would find.
    #[test]
    fn bytes_nothing_came_to_name_are_taken_back() {
        let data_dir = crate::fixtures::temp_root("artwork-unclaimed");

        let path = {
            let written = store(&data_dir, &jpeg()).unwrap();
            let path = cache_path(&data_dir, written.hash());
            assert!(path.exists(), "written first, as it has to be");
            path
        };

        assert!(!path.exists(), "and taken back when nothing claimed it");

        // One that was already there is somebody else's, and their row still
        // names it: a second writer giving up must not take it away.
        let kept = store(&data_dir, &png()).unwrap().kept();
        let path = cache_path(&data_dir, &kept);

        drop(store(&data_dir, &png()).unwrap());
        assert!(path.exists(), "the file was not this call's to take back");
    }

    /// A source nothing knows is treated as local, because being wrong about
    /// this deletes something that cost a walk and cannot be had back for free.
    #[test]
    fn only_a_source_we_know_fetched_counts_as_fetched() {
        assert!(acquired(FROM_COMMONS));
        assert!(!acquired("embedded"));
        assert!(!acquired("local_file"));
        assert!(!acquired("something a later version writes"));
    }

    #[test]
    fn identical_images_share_one_hash() {
        assert_eq!(hash_of(&jpeg()), hash_of(&jpeg()));
        assert_ne!(hash_of(&jpeg()), hash_of(b"something else"));
    }

    #[test]
    fn the_cache_path_fans_out_by_prefix() {
        let path = cache_path(Path::new("/data"), "abcdef123456");
        assert_eq!(path, Path::new("/data/artwork/ab/cdef123456"));
    }

    #[test]
    fn a_short_hash_does_not_panic_on_the_split() {
        assert_eq!(
            cache_path(Path::new("/data"), "a"),
            Path::new("/data/artwork/a")
        );
        assert_eq!(
            cache_path(Path::new("/data"), ""),
            Path::new("/data/artwork")
        );
    }

    #[test]
    fn images_are_recognised_by_their_bytes() {
        assert_eq!(mime_of(&jpeg()), Some("image/jpeg"));
        assert_eq!(
            mime_of(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(mime_of(b"GIF89a...."), Some("image/gif"));
        assert_eq!(mime_of(b"BM......."), Some("image/bmp"));

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBPVP8 ");
        assert_eq!(mime_of(&webp), Some("image/webp"));
    }

    #[test]
    fn what_is_not_an_image_is_not_claimed_to_be_one() {
        assert_eq!(mime_of(b"this is a text file"), None);
        assert_eq!(mime_of(b""), None);
        assert_eq!(mime_of(&[0xFF, 0xD8]), None, "a truncated JPEG signature");
        // A WAV is RIFF too, which is why the check reads further in.
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0, 0, 0, 0]);
        wav.extend_from_slice(b"WAVEfmt ");
        assert_eq!(mime_of(&wav), None);
    }

    #[test]
    fn storing_the_same_image_twice_writes_one_file() {
        let data_dir = std::env::temp_dir().join("tocata-artwork-store");
        let _ = std::fs::remove_dir_all(&data_dir);

        let first = store(&data_dir, &jpeg()).unwrap().kept();
        let second = store(&data_dir, &jpeg()).unwrap().kept();
        assert_eq!(first, second);

        let path = cache_path(&data_dir, &first);
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), jpeg());
    }

    #[test]
    fn a_name_in_any_case_is_recognised() {
        let directory = std::env::temp_dir().join("tocata-artwork-case");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("Cover.JPG"), png()).unwrap();

        let (path, _) = find_in_directory(&directory).expect("Cover.JPG should count");
        assert!(path.ends_with("Cover.JPG"));
    }

    #[test]
    fn preference_beats_directory_order() {
        let directory = std::env::temp_dir().join("tocata-artwork-order");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        // Both valid; cover wins because it comes first in the preference list.
        std::fs::write(directory.join("albumart.png"), png()).unwrap();
        std::fs::write(directory.join("cover.png"), png()).unwrap();

        let (path, _) = find_in_directory(&directory).unwrap();
        assert!(path.ends_with("cover.png"), "got {}", path.display());
    }

    #[test]
    fn a_disc_subdirectory_looks_a_level_up() {
        let album = std::env::temp_dir().join("tocata-artwork-multidisc");
        let _ = std::fs::remove_dir_all(&album);
        std::fs::create_dir_all(album.join("CD1")).unwrap();
        std::fs::create_dir_all(album.join("Disc 2")).unwrap();
        std::fs::write(album.join("cover.jpg"), png()).unwrap();

        assert!(
            find_near(&album.join("CD1")).is_some(),
            "CD1 should find the album cover"
        );
        assert!(
            find_near(&album.join("Disc 2")).is_some(),
            "Disc 2 should too"
        );
    }

    #[test]
    fn an_ordinary_directory_does_not_climb() {
        let artist = std::env::temp_dir().join("tocata-artwork-noclimb");
        let _ = std::fs::remove_dir_all(&artist);
        std::fs::create_dir_all(artist.join("Some Album")).unwrap();
        // A photo of the artist is not the cover of their albums.
        std::fs::write(artist.join("cover.jpg"), png()).unwrap();

        assert!(find_near(&artist.join("Some Album")).is_none());
    }

    #[test]
    fn only_a_disc_name_followed_by_a_number_counts() {
        assert!(looks_like_a_disc(Path::new("/x/CD1")));
        assert!(looks_like_a_disc(Path::new("/x/cd 2")));
        assert!(looks_like_a_disc(Path::new("/x/Disc-3")));
        assert!(looks_like_a_disc(Path::new("/x/DISK_4")));

        assert!(!looks_like_a_disc(Path::new("/x/Discipline")));
        assert!(!looks_like_a_disc(Path::new("/x/CD Singles")));
        assert!(!looks_like_a_disc(Path::new("/x/Disco")));
        assert!(!looks_like_a_disc(Path::new("/x/Album")));
    }

    #[test]
    fn a_cover_beside_the_music_is_found_and_checked() {
        let directory = std::env::temp_dir().join("tocata-artwork-find");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        assert!(find_in_directory(&directory).is_none(), "nothing there yet");

        // A file with the right name but the wrong contents is not a cover.
        std::fs::write(directory.join("cover.jpg"), b"I am not an image").unwrap();
        assert!(find_in_directory(&directory).is_none());

        std::fs::write(directory.join("folder.png"), {
            let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            png.extend_from_slice(b"body");
            png
        })
        .unwrap();

        let (path, bytes) = find_in_directory(&directory).expect("the png should be found");
        assert!(path.ends_with("folder.png"));
        assert_eq!(mime_of(&bytes), Some("image/png"));
    }
}
