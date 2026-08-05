// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Deciding which tracks belong to the same album.
//!
//! This is the one judgement call the scanner cannot avoid, and getting it
//! wrong is visible immediately: an album split in two, or two albums merged
//! into a mess.

use super::tags::Metadata;

/// Album artist values that mean "this is a collection", not an artist.
const VARIOUS_ARTISTS: [&str; 3] = ["various artists", "various", "va"];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlbumKey {
    /// The tag told us exactly which release this is. Nothing to guess.
    Release(String),
    /// A collection: the artist varies by design, so it plays no part.
    Compilation { name: String, date: Option<String> },
    /// The usual case.
    Tagged {
        artist: String,
        name: String,
        date: Option<String>,
    },
}

impl AlbumKey {
    /// The parts an album is grouped by when there is no release id: who signs
    /// it, what it is called, and which year it claims.
    ///
    /// `None` for a release id, which needs no grouping because it already
    /// identifies one release exactly.
    pub fn grouping(&self) -> Option<(&str, &str, Option<&str>)> {
        match self {
            Self::Release(_) => None,
            // A compilation groups without regard to artist, so it takes an
            // empty one and cannot collide with a real name.
            Self::Compilation { name, date } => Some(("", name, date.as_deref())),
            Self::Tagged { artist, name, date } => Some((artist, name, date.as_deref())),
        }
    }

    /// Returns `None` when there is no album to speak of. A track with no album
    /// tag is a loose track, not a member of an album called "".
    pub fn of(metadata: &Metadata) -> Option<Self> {
        let name = metadata.album.as_deref().map(normalize)?;
        if name.is_empty() {
            return None;
        }

        // A release id settles the question, including for the cases below.
        if let Some(mbid) = metadata.mbid_release.as_deref() {
            let mbid = mbid.trim();
            if !mbid.is_empty() {
                return Some(Self::Release(mbid.to_string()));
            }
        }

        let date = grouping_date(metadata);

        if is_compilation(metadata) {
            return Some(Self::Compilation { name, date });
        }

        // Album artist first, since that is what it is for. Falling back to the
        // track artists is worse but still specific to this album; falling back
        // to a fixed string, as miko.rs does, merges every untagged album that
        // happens to share a title.
        let artist = if metadata.album_artists.is_empty() {
            join_artists(&metadata.artists)
        } else {
            join_artists(&metadata.album_artists)
        };

        Some(Self::Tagged { artist, name, date })
    }

    /// Who signs the record, which is not always who the tag says.
    ///
    /// An artist and no album artist is the ordinary shape of a hand-tagged
    /// album, and reading it strictly leaves the record signed by nobody: a dash
    /// where a listing says who made it, an empty album artist for the clients
    /// that browse by it, and — since the listing sorts on that name — every such
    /// record piled together at the top under no name at all.
    ///
    /// So the credit falls back exactly where the grouping above already fell
    /// back, and that condition is what makes it safe rather than a guess: a
    /// `Tagged` key with no album artist *was built from* the track artists, so
    /// every track on that record carries the same set of them — a different set
    /// is a different key and a different album. The credit cannot grow a name
    /// per track, because there is only one set to grow from.
    ///
    /// A compilation gets nothing, and neither does a record grouped by its
    /// release id. Those two group without regard to artist on purpose, so their
    /// tracks may well be by different people, and there the fallback would
    /// credit the record to whoever happened to be on it.
    pub fn credited<'m>(&self, metadata: &'m Metadata) -> &'m [String] {
        if !metadata.album_artists.is_empty() {
            return &metadata.album_artists;
        }

        match self {
            Self::Tagged { .. } => &metadata.artists,
            Self::Compilation { .. } | Self::Release(_) => &[],
        }
    }
}

/// True for anything that should be grouped without regard to artist.
fn is_compilation(metadata: &Metadata) -> bool {
    if metadata.is_compilation {
        return true;
    }

    metadata
        .album_artists
        .iter()
        .any(|a| VARIOUS_ARTISTS.contains(&normalize(a).as_str()))
}

/// The year is enough to separate an original from its remaster, and it is far
/// more reliably tagged than a full release date. Using the whole date would
/// split an album whose tracks disagree on the day.
fn grouping_date(metadata: &Metadata) -> Option<String> {
    metadata.year.map(|year| year.to_string())
}

/// Sorts before joining, so the order a tagger happened to write the artists in
/// does not create a second album.
fn join_artists(artists: &[String]) -> String {
    let mut normalized: Vec<String> = artists
        .iter()
        .map(|a| normalize(a))
        .filter(|a| !a.is_empty())
        .collect();

    normalized.sort();
    normalized.dedup();
    normalized.join("\u{1f}")
}

/// Folds away the differences that are not differences: surrounding space and
/// letter case.
fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(album: &str, artists: &[&str], album_artists: &[&str]) -> Metadata {
        Metadata {
            album: Some(album.to_string()),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            album_artists: album_artists.iter().map(|a| a.to_string()).collect(),
            year: Some(1975),
            ..Default::default()
        }
    }

    #[test]
    fn a_release_id_wins_over_everything_else() {
        let mut one = metadata("A Night at the Opera", &["Queen"], &["Queen"]);
        one.mbid_release = Some("abc-123".into());

        let mut two = metadata("completely different title", &["Someone"], &["Else"]);
        two.mbid_release = Some("abc-123".into());

        assert_eq!(AlbumKey::of(&one), AlbumKey::of(&two));
        assert_eq!(
            AlbumKey::of(&one),
            Some(AlbumKey::Release("abc-123".into()))
        );
    }

    #[test]
    fn tracks_of_the_same_album_group_together() {
        let one = metadata("A Night at the Opera", &["Queen"], &["Queen"]);
        let two = metadata("A Night at the Opera", &["Queen"], &["Queen"]);
        assert_eq!(AlbumKey::of(&one), AlbumKey::of(&two));
    }

    #[test]
    fn case_and_spacing_do_not_split_an_album() {
        let one = metadata("A Night at the Opera", &["Queen"], &["Queen"]);
        let two = metadata("  a night at the OPERA ", &["queen"], &["  QUEEN"]);
        assert_eq!(AlbumKey::of(&one), AlbumKey::of(&two));
    }

    #[test]
    fn the_order_of_album_artists_does_not_split_an_album() {
        let one = metadata("Watch the Throne", &[], &["Jay-Z", "Kanye West"]);
        let two = metadata("Watch the Throne", &[], &["Kanye West", "Jay-Z"]);
        assert_eq!(AlbumKey::of(&one), AlbumKey::of(&two));
    }

    #[test]
    fn a_compilation_groups_across_different_track_artists() {
        let mut one = metadata("Hits 96", &["Björk"], &["Various Artists"]);
        let mut two = metadata("Hits 96", &["Pulp"], &["Various Artists"]);
        assert_eq!(AlbumKey::of(&one), AlbumKey::of(&two));

        // And the same when it is the flag rather than the artist name.
        one.album_artists.clear();
        two.album_artists.clear();
        one.is_compilation = true;
        two.is_compilation = true;
        assert_eq!(AlbumKey::of(&one), AlbumKey::of(&two));
    }

    #[test]
    fn different_artists_keep_their_albums_apart() {
        let one = metadata("Greatest Hits", &["Queen"], &["Queen"]);
        let two = metadata("Greatest Hits", &["ABBA"], &["ABBA"]);
        assert_ne!(AlbumKey::of(&one), AlbumKey::of(&two));
    }

    /// The failure mode of miko.rs: with no album artist it keys on the literal
    /// string "Unknown Artist", so every untagged album sharing a title becomes
    /// one album.
    #[test]
    fn untagged_albums_sharing_a_title_stay_apart() {
        let one = metadata("Greatest Hits", &["Queen"], &[]);
        let two = metadata("Greatest Hits", &["ABBA"], &[]);
        assert_ne!(AlbumKey::of(&one), AlbumKey::of(&two));
    }

    #[test]
    fn a_remaster_is_not_the_original() {
        let mut original = metadata("Rumours", &["Fleetwood Mac"], &["Fleetwood Mac"]);
        original.year = Some(1977);
        let mut remaster = metadata("Rumours", &["Fleetwood Mac"], &["Fleetwood Mac"]);
        remaster.year = Some(2004);

        assert_ne!(AlbumKey::of(&original), AlbumKey::of(&remaster));
    }

    #[test]
    fn a_track_with_no_album_belongs_to_none() {
        let mut loose = metadata("", &["Queen"], &[]);
        loose.album = None;
        assert_eq!(AlbumKey::of(&loose), None);

        loose.album = Some("   ".into());
        assert_eq!(AlbumKey::of(&loose), None);
    }

    #[test]
    fn a_record_with_no_album_artist_is_credited_to_its_artists() {
        let hand_tagged = metadata("Greatest Hits", &["Queen"], &[]);
        let key = AlbumKey::of(&hand_tagged).unwrap();

        assert_eq!(
            key.credited(&hand_tagged),
            ["Queen".to_string()],
            "the one name the file does carry signs the record"
        );
    }

    #[test]
    fn a_tagged_album_artist_is_the_credit() {
        let both = metadata("Watch the Throne", &["Jay-Z"], &["Jay-Z", "Kanye West"]);
        let key = AlbumKey::of(&both).unwrap();

        assert_eq!(
            key.credited(&both),
            ["Jay-Z".to_string(), "Kanye West".to_string()],
            "tagged as it was tagged, and in the order it was tagged"
        );
    }

    /// The two keys that group without regard to artist get no credit at all: the
    /// tracks on them may be by anybody, so the first one through the scanner
    /// would end up signing a record it does not own.
    #[test]
    fn a_compilation_is_credited_to_nobody() {
        let mut collected = metadata("Hits 96", &["Björk"], &[]);
        collected.is_compilation = true;
        let key = AlbumKey::of(&collected).unwrap();

        assert!(key.credited(&collected).is_empty());
    }

    #[test]
    fn a_record_grouped_by_its_release_id_is_credited_to_nobody() {
        let mut released = metadata("Hits 96", &["Björk"], &[]);
        released.mbid_release = Some("abc-123".into());
        let key = AlbumKey::of(&released).unwrap();

        assert!(key.credited(&released).is_empty());
    }

    #[test]
    fn a_multi_disc_album_stays_one_album() {
        let mut disc_one = metadata("The Wall", &["Pink Floyd"], &["Pink Floyd"]);
        disc_one.disc_number = Some(1);
        let mut disc_two = metadata("The Wall", &["Pink Floyd"], &["Pink Floyd"]);
        disc_two.disc_number = Some(2);

        assert_eq!(
            AlbumKey::of(&disc_one),
            AlbumKey::of(&disc_two),
            "the disc number belongs to the track, not to the album identity"
        );
    }
}
