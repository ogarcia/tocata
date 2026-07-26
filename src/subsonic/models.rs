// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The objects the API returns.
//!
//! Only fields Tocata can actually fill. The specification marks almost
//! everything optional, and a field returned empty just to be present tells a
//! client nothing it could not work out from its absence.

use serde::Serialize;

/// A song, as every listing returns it.
///
/// `duration` is in seconds here, not the milliseconds the database keeps:
/// that is what the specification says and clients count on it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Child {
    pub id: String,
    pub is_dir: bool,
    pub title: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_rate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub played: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,

    /// Always false: video is out of scope, but the field is defined and some
    /// clients read it.
    pub is_video: bool,
    /// "music" for everything here, as opposed to podcast or audiobook.
    pub r#type: &'static str,
    pub media_type: &'static str,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_brainz_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub isrc: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<ItemGenre>,
    /// The structured multi-artist list OpenSubsonic added, which is the only
    /// way a client can tell "A & B" from an artist actually called that.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<ArtistId3>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_artist: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub album_artists: Vec<ArtistId3>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_gain: Option<ReplayGain>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistId3 {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_brainz_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
}

impl ArtistId3 {
    /// The bare form used inside a song's artist list, where the client only
    /// needs to know who this is.
    pub fn named(id: String, name: String) -> Self {
        Self {
            id,
            name,
            cover_art: None,
            album_count: None,
            starred: None,
            music_brainz_id: None,
            sort_name: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumId3 {
    pub id: String,
    pub name: String,
    /// Required by the specification even when zero.
    pub song_count: i64,
    /// Seconds, summed over the album.
    pub duration: i64,
    pub created: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_brainz_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_compilation: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<ItemGenre>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<ArtistId3>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_artist: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disc_titles: Vec<DiscTitle>,
}

impl Child {
    /// A directory as it appears inside a listing.
    ///
    /// `getMusicDirectory` returns folders and songs in one array, which is why
    /// Child has an `isDir` flag at all. That flag is a shape of the response,
    /// not a reason to keep both kinds in one table: this is two selects and a
    /// concatenation.
    pub fn directory(id: String, title: String, parent: Option<String>) -> Self {
        Self {
            id,
            is_dir: true,
            title,
            parent,
            album: None,
            artist: None,
            track: None,
            year: None,
            genre: None,
            cover_art: None,
            size: None,
            content_type: None,
            suffix: None,
            duration: None,
            bit_rate: None,
            bit_depth: None,
            sampling_rate: None,
            channel_count: None,
            disc_number: None,
            created: None,
            starred: None,
            user_rating: None,
            play_count: None,
            played: None,
            album_id: None,
            artist_id: None,
            is_video: false,
            r#type: "music",
            media_type: "album",
            bpm: None,
            comment: None,
            sort_name: None,
            music_brainz_id: None,
            isrc: Vec::new(),
            genres: Vec::new(),
            artists: Vec::new(),
            display_artist: None,
            album_artists: Vec::new(),
            display_album_artist: None,
            replay_gain: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ItemGenre {
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscTitle {
    pub disc: i64,
    pub title: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayGain {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_peak: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_peak: Option<f64>,
}

impl ReplayGain {
    /// Returns `None` when nothing was tagged, so the object is left out
    /// entirely rather than sent empty.
    pub fn of(
        track_gain: Option<f64>,
        track_peak: Option<f64>,
        album_gain: Option<f64>,
        album_peak: Option<f64>,
    ) -> Option<Self> {
        if track_gain.is_none()
            && track_peak.is_none()
            && album_gain.is_none()
            && album_peak.is_none()
        {
            return None;
        }

        Some(Self {
            track_gain,
            track_peak,
            album_gain,
            album_peak,
        })
    }
}

/// Milliseconds to the seconds the API speaks in, rounded to nearest so a
/// three-and-a-half minute song does not come back as three.
pub fn seconds(milliseconds: Option<i64>) -> Option<i64> {
    milliseconds.map(|ms| (ms + 500) / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_rounded_to_the_nearest_second() {
        assert_eq!(seconds(Some(0)), Some(0));
        assert_eq!(seconds(Some(499)), Some(0));
        assert_eq!(seconds(Some(500)), Some(1));
        assert_eq!(seconds(Some(210_400)), Some(210));
        assert_eq!(seconds(Some(210_600)), Some(211));
        assert_eq!(seconds(None), None);
    }

    #[test]
    fn replay_gain_is_absent_when_nothing_was_tagged() {
        assert!(ReplayGain::of(None, None, None, None).is_none());
        assert!(ReplayGain::of(Some(-7.0), None, None, None).is_some());
    }

    #[test]
    fn optional_fields_are_left_out_rather_than_sent_empty() {
        let artist = ArtistId3::named("abc".into(), "Queen".into());
        let value = serde_json::to_value(&artist).unwrap();

        assert_eq!(value["id"], "abc");
        assert_eq!(value["name"], "Queen");
        assert!(value.get("coverArt").is_none());
        assert!(value.get("albumCount").is_none());
        assert_eq!(value.as_object().unwrap().len(), 2);
    }
}
