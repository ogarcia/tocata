// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Cover images, kept on disk rather than in the database.
//!
//! The database holds a row describing each image; the bytes live in a cache
//! directory named after their own hash. That gets deduplication for free: every
//! track of an album carries the same embedded picture, and they all resolve to
//! one file.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Subdirectory of the data directory where image bytes go.
const CACHE_DIRECTORY: &str = "artwork";

/// File names that hold an album cover, in the order they are trusted. These are
/// the conventions taggers and other music software have settled on.
pub const COVER_FILE_STEMS: &[&str] = &["cover", "folder", "front", "album", "albumart"];

/// Extensions accepted for a cover found on disk.
pub const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// Where the bytes of an image with this hash live.
pub fn cache_path(data_dir: &Path, hash: &str) -> PathBuf {
    // Two levels of fan out, so a library with thousands of covers does not put
    // thousands of entries in one directory.
    let (prefix, rest) = hash.split_at(2.min(hash.len()));
    data_dir.join(CACHE_DIRECTORY).join(prefix).join(rest)
}

/// Writes image bytes into the cache and returns their hash.
///
/// Writing the same image twice is not an error and not extra work: the second
/// call finds the file already there.
pub fn store(data_dir: &Path, bytes: &[u8]) -> Result<String> {
    let hash = hash_of(bytes);
    let path = cache_path(data_dir, &hash);

    if path.exists() {
        return Ok(hash);
    }

    let parent = path
        .parent()
        .context("an artwork cache path always has a parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating the artwork cache at {}", parent.display()))?;
    std::fs::write(&path, bytes)
        .with_context(|| format!("writing artwork to {}", path.display()))?;

    Ok(hash)
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

/// Looks for a cover image sitting beside the music.
///
/// Tries the conventional names in order and returns the first that is really an
/// image, so a `cover.jpg` that is actually a text file does not become an album
/// cover.
pub fn find_in_directory(directory: &Path) -> Option<(PathBuf, Vec<u8>)> {
    for stem in COVER_FILE_STEMS {
        for extension in COVER_EXTENSIONS {
            let candidate = directory.join(format!("{stem}.{extension}"));
            let Ok(bytes) = std::fs::read(&candidate) else {
                continue;
            };
            if mime_of(&bytes).is_some() {
                return Some((candidate, bytes));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg() -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
        bytes.extend_from_slice(b"padding to make it look like a file");
        bytes
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

        let first = store(&data_dir, &jpeg()).unwrap();
        let second = store(&data_dir, &jpeg()).unwrap();
        assert_eq!(first, second);

        let path = cache_path(&data_dir, &first);
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), jpeg());
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
