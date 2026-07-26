// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Walking a library root and deciding what is worth looking at.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Extensions treated as audio. Only formats lofty can actually read tags
/// from, so a file that gets picked up here is one we can describe.
const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "wv", "aiff", "aif", "ape",
    "mpc", "wma",
];

/// Directories that are never music, however they got there. The NAS vendors
/// are the reason this list exists: a Synology share is littered with
/// `@eaDir` folders full of thumbnails, and walking into them wastes time and
/// invents albums.
const SKIPPED_DIRECTORIES: &[&str] = &["@eaDir", ".@__thumb", "#recycle", "$RECYCLE.BIN"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Directory {
        path: PathBuf,
        modified: Option<i64>,
    },
    Audio {
        path: PathBuf,
        extension: String,
        size: u64,
        modified: i64,
    },
}

/// Walks a library root, yielding directories before their contents.
///
/// That order is not incidental: `folders.parent_id` references the same
/// table, so a parent row has to exist before its children. Walking top down
/// gives that for free, with no need to sort by path depth afterwards.
///
/// Unreadable entries are skipped rather than aborting the walk. One
/// directory with awkward permissions should not cost the user their library.
pub fn walk(root: &Path) -> impl Iterator<Item = Entry> + use<> {
    WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| !is_skipped(entry.file_name().to_string_lossy().as_ref()))
        .filter_map(Result::ok)
        .filter_map(|entry| classify(entry.path()))
}

/// Hidden files and the vendor rubbish above. Anything starting with a dot is
/// somebody's metadata, not music: `.stfolder` from Syncthing, `.DS_Store`,
/// AppleDouble files.
fn is_skipped(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_DIRECTORIES.contains(&name)
}

fn classify(path: &Path) -> Option<Entry> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let modified = modified_seconds(&metadata);

    if metadata.is_dir() {
        return Some(Entry::Directory {
            path: path.to_path_buf(),
            modified,
        });
    }

    if !metadata.is_file() {
        return None;
    }

    let extension = path.extension()?.to_str()?.to_lowercase();
    if !AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }

    Some(Entry::Audio {
        path: path.to_path_buf(),
        extension,
        size: metadata.len(),
        modified: modified?,
    })
}

/// Seconds since the Unix epoch, or `None` on the filesystems that do not
/// keep a modification time. A track without one is still worth having; it
/// just gets rescanned every time.
fn modified_seconds(metadata: &std::fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Builds a tree under a unique temporary directory and returns its root.
    fn library(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("tocata-walker-{name}"));
        let _ = fs::remove_dir_all(&root);

        for file in files {
            let path = root.join(file);
            if file.ends_with('/') {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, b"not really audio").unwrap();
            }
        }

        root
    }

    fn path_of(entry: &Entry) -> &Path {
        match entry {
            Entry::Directory { path, .. } | Entry::Audio { path, .. } => path,
        }
    }

    fn names(root: &Path) -> Vec<String> {
        walk(root)
            .map(|entry| {
                path_of(&entry)
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn audio_is_picked_up_and_everything_else_is_not() {
        let root = library(
            "mixed",
            &[
                "Queen/song.flac",
                "Queen/song.mp3",
                "Queen/cover.jpg",
                "Queen/notes.txt",
                "Queen/playlist.m3u",
            ],
        );

        let found = names(&root);
        assert!(found.contains(&"Queen/song.flac".to_string()));
        assert!(found.contains(&"Queen/song.mp3".to_string()));
        assert!(!found.iter().any(|n| n.ends_with(".jpg")));
        assert!(!found.iter().any(|n| n.ends_with(".txt")));
        assert!(!found.iter().any(|n| n.ends_with(".m3u")));
    }

    #[test]
    fn extensions_are_matched_regardless_of_case() {
        let root = library("case", &["A/one.FLAC", "A/two.Mp3", "A/three.OGG"]);
        assert_eq!(
            names(&root).iter().filter(|n| n.starts_with("A/")).count(),
            3
        );
    }

    #[test]
    fn a_parent_directory_always_comes_before_its_contents() {
        let root = library("order", &["Artist/Album/Disc 1/song.flac"]);
        let found = names(&root);

        let position = |needle: &str| found.iter().position(|n| n == needle).unwrap();
        assert!(position("Artist") < position("Artist/Album"));
        assert!(position("Artist/Album") < position("Artist/Album/Disc 1"));
        assert!(position("Artist/Album/Disc 1") < position("Artist/Album/Disc 1/song.flac"));
    }

    #[test]
    fn nas_rubbish_is_not_walked_into() {
        let root = library(
            "nas",
            &[
                "Album/song.flac",
                "Album/@eaDir/song.flac/SYNOPHOTO_THUMB_M.jpg",
                "Album/@eaDir/thumb.flac",
                "#recycle/deleted.flac",
            ],
        );

        let found = names(&root);
        assert_eq!(found.iter().filter(|n| n.ends_with(".flac")).count(), 1);
        assert!(!found.iter().any(|n| n.contains("@eaDir")));
        assert!(!found.iter().any(|n| n.contains("#recycle")));
    }

    #[test]
    fn hidden_entries_are_left_alone() {
        let root = library(
            "hidden",
            &[
                "Album/song.flac",
                "Album/.hidden.flac",
                ".stfolder/junk.flac",
                "Album/._song.flac",
            ],
        );

        let found = names(&root);
        assert_eq!(found.iter().filter(|n| n.ends_with(".flac")).count(), 1);
        assert!(found.contains(&"Album/song.flac".to_string()));
    }

    #[test]
    fn audio_entries_carry_size_and_modification_time() {
        let root = library("meta", &["Album/song.flac"]);

        let entry = walk(&root)
            .find(|e| matches!(e, Entry::Audio { .. }))
            .expect("the track should be found");

        match entry {
            Entry::Audio {
                extension,
                size,
                modified,
                ..
            } => {
                assert_eq!(extension, "flac");
                assert_eq!(size, b"not really audio".len() as u64);
                assert!(modified > 0);
            }
            other => panic!("expected audio, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_library_yields_nothing_and_does_not_fail() {
        let root = library("empty", &["Empty/"]);
        assert_eq!(names(&root), vec!["Empty".to_string()]);
    }

    #[test]
    fn a_missing_root_yields_nothing_rather_than_panicking() {
        let root = std::env::temp_dir().join("tocata-walker-does-not-exist");
        let _ = std::fs::remove_dir_all(&root);
        assert!(walk(&root).next().is_none());
    }
}
