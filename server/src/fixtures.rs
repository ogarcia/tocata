// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Real files on real disk, for the tests that cannot do without them.
//!
//! Most of this program can be tested against a database in memory, and is. Two
//! things cannot: a scan, which walks directory entries and reads bytes, and the
//! calls that answer by opening the file a track came from. Both need something on
//! disk that lofty will accept as audio.
//!
//! Here rather than in either test module because it was in one of them and the
//! other needed it. A second copy of a RIFF header is a second copy to keep right,
//! and neither module would notice the other drifting.

use lofty::prelude::{ItemKey, TagExt};
use lofty::tag::{ItemValue, Tag, TagItem, TagType};
use std::fs;
use std::path::{Path, PathBuf};

/// An empty directory of its own, wiped first so a test that failed halfway last
/// time does not hand its leftovers to this one.
pub fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("tocata-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// The smallest thing lofty will read as audio: a RIFF/WAVE header and one silent
/// sample, so the properties it reports — 44.1 kHz, stereo, sixteen bits — are read
/// out of a real file rather than mocked.
///
/// Beats shipping a binary fixture, and needs no encoder installed to build.
pub fn write_wav(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    let data = [0u8; 4];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes()); // stereo
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&176_400u32.to_le_bytes()); // byte rate
    bytes.extend_from_slice(&4u16.to_le_bytes()); // block align
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);

    fs::write(path, bytes).unwrap();
}

/// Writes a tag onto a file already on disk.
///
/// The kind of tag is the caller's to choose and it matters: which keys a format can
/// even hold differs, and the reader names its frames after whichever this is.
pub fn tag_file(path: &Path, kind: TagType, items: &[(ItemKey, &str)]) {
    let mut tag = Tag::new(kind);

    for (key, value) in items {
        tag.insert(TagItem::new(*key, ItemValue::Text(value.to_string())));
    }

    tag.save_to_path(path, Default::default()).unwrap();
}
