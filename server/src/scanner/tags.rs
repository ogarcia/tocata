// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Reading what a file says about itself.

use anyhow::{Context, Result};
use lofty::config::ParseOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::probe::Probe;
use lofty::tag::{ItemValue, Tag, TagType};
use std::path::Path;

/// Separators taggers use to cram several artists into one field. Splitting on
/// them is a guess, but leaving "Simon & Garfunkel; Art Garfunkel" as a single
/// artist name is a worse one.
const ARTIST_SEPARATORS: [char; 2] = [';', '\0'];

/// And the one more that a list of identifiers takes.
///
/// ID3v2.3 has no null separator: it divides several values inside one field with a
/// slash, so a file with three artists writes their three ids as one string. Cutting
/// on it is safe here and nowhere else — a MusicBrainz id is a UUID and can never
/// contain a slash, while a name very well can: AC/DC is one band.
const ID_SEPARATORS: [char; 3] = [';', '\0', '/'];

/// Names paired with the identifiers that belong to them, and nothing paired when
/// they cannot be.
///
/// A file writes the two as parallel lists and nothing in it says which id belongs
/// to which name — only that the orders match, which is a convention rather than a
/// rule. So the pairing is by position, and only when the counts agree.
///
/// When they do not, every name goes unidentified. That is the whole point: the
/// alternative is guessing, and a wrong MusicBrainz id is worse than none. It is
/// what would happen to the common case of a credit that could not be split —
/// "Tiziano Ferro feat. Anahí & Dulce María" is one name against three ids, and
/// giving it the first would mark a person who does not exist with the identity of
/// one who does, then fetch their photograph for it.
pub fn identified<'m>(
    names: &'m [String],
    mbids: &'m [String],
) -> impl Iterator<Item = (&'m str, Option<&'m str>)> {
    let paired = names.len() == mbids.len();

    names
        .iter()
        .enumerate()
        .map(move |(at, name)| (name.as_str(), paired.then(|| mbids[at].as_str())))
}

/// Everything read out of one audio file.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub sort_title: Option<String>,
    pub artists: Vec<String>,
    pub album_artists: Vec<String>,
    /// The credit as the file writes it, whole: "Tiziano Ferro feat. Anahí & Dulce
    /// María".
    ///
    /// Kept beside the names rather than instead of them, because it is a different
    /// thing. The names are who is on the track — three people, each with a page
    /// somewhere and a MusicBrainz id of their own — and this is how the record
    /// credits them, which no list of names can be joined back into: "feat." and
    /// "&" are the tagger's words about who did what.
    pub artist_credit: Option<String>,
    /// MusicBrainz ids for `artists`, in the same order, or empty.
    ///
    /// Empty rather than partly filled: see `identified`, which is where the two
    /// lists are lined up or given up on.
    pub mbid_artists: Vec<String>,
    /// And the same for `album_artists`.
    pub mbid_album_artists: Vec<String>,
    pub album: Option<String>,
    pub sort_album: Option<String>,
    pub genres: Vec<String>,
    pub year: Option<i32>,
    pub date: Option<String>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub disc_subtitle: Option<String>,
    pub bpm: Option<i64>,
    pub comment: Option<String>,
    pub is_compilation: bool,
    pub mbid_recording: Option<String>,
    pub mbid_track: Option<String>,
    pub mbid_release: Option<String>,
    pub mbid_release_group: Option<String>,
    pub isrc: Option<String>,
    /// Who put the record out. A fact about the release, kept on the album.
    pub label: Option<String>,
    pub rg_track_gain: Option<f64>,
    pub rg_track_peak: Option<f64>,
    pub rg_album_gain: Option<f64>,
    pub rg_album_peak: Option<f64>,
    pub lyrics: Option<String>,
    /// Which frame the lyrics came out of, as the file's own format names it.
    ///
    /// The scanner has no use for it — it does not keep the words in the first
    /// place. What wants it is the panel: telling somebody their words are in
    /// `USLT` rather than in an `.lrc` beside the file is the whole point of
    /// showing them at all in an administration panel. It is a borrowed name from
    /// the reader's own table rather than a string, so carrying it costs nothing
    /// on the scans that never look at it.
    pub lyrics_frame: Option<&'static str>,
    /// Bytes of the embedded front cover, when the file carries one.
    pub picture: Option<Vec<u8>>,
    pub duration_ms: Option<i64>,
    pub bit_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub sampling_rate: Option<i64>,
    pub channel_count: Option<i64>,
}

/// Reads one file, without its embedded artwork. Blocking: callers put this on a
/// blocking task.
///
/// Leaving the artwork out is what keeps a scan cheap. lofty loads an embedded
/// picture into memory as part of parsing, and a library of five thousand albums
/// carrying five hundred kilobytes each is gigabytes read and thrown away for
/// the sake of a title and a track number.
pub fn read(path: &Path) -> Result<Metadata> {
    read_with(path, ParseOptions::new().read_cover_art(false))
}

/// Reads one file including its embedded artwork, for when somebody actually
/// asked to see the cover.
pub fn read_with_cover_art(path: &Path) -> Result<Metadata> {
    read_with(path, ParseOptions::new().read_cover_art(true))
}

/// A picture of whoever made the record, out of a file that carries one.
///
/// Never the front cover, and that is the whole of the rule: the types are
/// different and a file may hold both. A sleeve is a picture of a record, and using
/// one where a photograph of the band belongs would put the same image on the
/// artist and on every album they made — which looks less like a photograph than
/// like something gone wrong.
///
/// The three types that do answer it are `Artist`, `LeadArtist` and `Band`: one
/// person, the person out front, and the group. Which of them a file used says
/// something about the file and nothing about what is wanted here, so the first of
/// the three it carries is the answer.
///
/// Rare in the wild. It costs one pass over pictures already parsed, and the
/// alternative for those files is asking a website for something that was on the
/// disk all along.
pub fn read_artist_picture(path: &Path) -> Result<Option<Vec<u8>>> {
    let tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .options(ParseOptions::new().read_cover_art(true))
        .read()
        .with_context(|| format!("reading tags from {}", path.display()))?;

    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(None);
    };

    Ok(tag
        .pictures()
        .iter()
        .find(|picture| {
            matches!(
                picture.pic_type(),
                PictureType::Artist | PictureType::LeadArtist | PictureType::Band
            )
        })
        .map(|picture| picture.data().to_vec()))
}

/// Every tag in a file, spelt as its own format spells them.
///
/// The answer the database cannot give, and the reason the panel offers it: the
/// scanner keeps the fields the schema has columns for and lets the rest go by, so
/// the composer, the producer and whoever engineered the record are only ever
/// visible here.
///
/// Not every byte of the tag, and it does not claim to be. What arrives has been
/// through a reader that maps the frames it knows onto one set of names, so a
/// vendor's own invention is not merely unnamed — it never got here, and cannot even
/// be counted.
///
/// Blocking, and it reads the embedded picture: how much of the file the artwork
/// accounts for is half of what makes the list worth reading.
pub fn read_every(path: &Path) -> Result<crate::types::Tags> {
    let tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .options(ParseOptions::new().read_cover_art(true))
        .read()
        .with_context(|| format!("reading tags from {}", path.display()))?;

    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(crate::types::Tags {
            kind: None,
            tags: Vec::new(),
            picture: None,
        });
    };

    let kind = tag.tag_type();

    // A number and its total are one frame in the file and two keys to the reader:
    // `TRCK` holding "1/5" arrives as a track number and a track total, and both of
    // them name `TRCK` when asked what frame they came from. Listed as they arrive
    // that is the same frame twice with half an answer in each, which is a list
    // disagreeing with the file it claims to be reading. So the halves are put back
    // together and the totals are not rows of their own.
    let totals: Vec<(ItemKey, String)> = tag
        .items()
        .filter(|item| matches!(item.key(), ItemKey::TrackTotal | ItemKey::DiscTotal))
        .filter_map(|item| Some((item.key(), item.value().text()?.trim().to_string())))
        .collect();

    let total_for = |number: ItemKey| {
        let total = match number {
            ItemKey::TrackNumber => ItemKey::TrackTotal,
            ItemKey::DiscNumber => ItemKey::DiscTotal,
            _ => return None,
        };

        totals
            .iter()
            .find(|(key, _)| *key == total)
            .map(|(_, value)| value.clone())
    };

    let tags = tag
        .items()
        .filter(|item| !matches!(item.key(), ItemKey::TrackTotal | ItemKey::DiscTotal))
        .filter_map(|item| {
            // Its name in this format, and nothing where the format has none for it:
            // a reader that knows a key an encoding cannot write has nothing to show
            // for it, and a made up name would look like something in the file.
            let name = item.key().map_key(kind)?.to_string();

            let value = match item.value() {
                ItemValue::Text(text) | ItemValue::Locator(text) => text.trim().to_string(),
                // Nothing in a tag that is not text belongs on screen as text, and
                // its size is the only thing anybody would read anyway.
                ItemValue::Binary(bytes) => bytes_over(bytes.len()),
            };

            if value.is_empty() {
                return None;
            }

            let value = match total_for(item.key()) {
                Some(total) => format!("{value}/{total}"),
                None => value,
            };

            Some(crate::types::Tagged { name, value })
        })
        .collect();

    // The front cover if the file says which one it is, and otherwise whatever it
    // carries, which is the same choice the scanner makes about which to cache.
    let picture = tag
        .pictures()
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())
        .map(|picture| crate::types::Tagged {
            name: picture_frame(kind).to_string(),
            value: [
                depicting(picture.pic_type()).to_string(),
                picture
                    .mime_type()
                    .map(MimeType::to_string)
                    .unwrap_or_else(|| "unknown".to_string()),
                bytes_over(picture.data().len()),
            ]
            .join(" · "),
        });

    Ok(crate::types::Tags {
        kind: Some(named(kind).to_string()),
        tags,
        picture,
    })
}

/// What a kind of tag is called, in the words the formats themselves use.
fn named(kind: TagType) -> &'static str {
    match kind {
        TagType::Ape => "APE",
        TagType::Id3v1 => "ID3v1",
        TagType::Id3v2 => "ID3v2",
        TagType::Mp4Ilst => "MP4",
        TagType::VorbisComments => "Vorbis comments",
        TagType::RiffInfo => "RIFF INFO",
        TagType::AiffText => "AIFF text",
        // lofty may learn another before we do.
        _ => "unknown",
    }
}

/// Where a format keeps its artwork, so the picture is named the way every other row
/// in the list is.
fn picture_frame(kind: TagType) -> &'static str {
    match kind {
        TagType::Id3v2 => "APIC",
        TagType::Mp4Ilst => "covr",
        TagType::VorbisComments => "METADATA_BLOCK_PICTURE",
        TagType::Ape => "Cover Art (Front)",
        _ => "picture",
    }
}

/// What the picture is of, as the tag says. Only the kinds anybody's collection
/// actually holds are worded; the rest are a picture, which is all the row needs to
/// say for something nobody put there on purpose.
fn depicting(what: PictureType) -> &'static str {
    match what {
        PictureType::CoverFront => "front cover",
        PictureType::CoverBack => "back cover",
        PictureType::Artist | PictureType::LeadArtist => "artist",
        PictureType::Band => "band",
        PictureType::Media => "media",
        PictureType::Icon | PictureType::OtherIcon => "icon",
        _ => "picture",
    }
}

/// A size in the units people read them in. Rounded to whole units above a kilobyte,
/// because nothing here is worth a decimal.
fn bytes_over(size: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];

    let mut size = size as f64;
    let mut unit = 0;

    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }

    format!("{} {}", size.round(), UNITS[unit])
}

fn read_with(path: &Path, options: ParseOptions) -> Result<Metadata> {
    let tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .options(options)
        .read()
        .with_context(|| format!("reading tags from {}", path.display()))?;

    let properties = tagged.properties();
    let mut metadata = Metadata {
        duration_ms: Some(properties.duration().as_millis() as i64),
        bit_rate: properties.audio_bitrate().map(i64::from),
        bit_depth: properties.bit_depth().map(i64::from),
        sampling_rate: properties.sample_rate().map(i64::from),
        channel_count: properties.channels().map(i64::from),
        ..Default::default()
    };

    // A file with no tag at all is still a track; the caller falls back to the
    // file name for a title.
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(metadata);
    };

    read_tag(tag, &mut metadata);
    Ok(metadata)
}

fn read_tag(tag: &Tag, metadata: &mut Metadata) {
    metadata.title = clean(tag.title().as_deref());
    metadata.album = clean(tag.album().as_deref());
    metadata.comment = clean(tag.comment().as_deref());
    metadata.track_number = tag.track().map(i64::from);
    metadata.disc_number = tag.disk().map(i64::from);

    // Who is on the track, and how the track credits them.
    //
    // `ARTISTS` first, which is where a tagger that knows the difference puts the
    // names one by one — Picard writes it, and writes the ids in the same order.
    // Without it there is only the credit, and splitting that is the guess it has
    // always been: "Tiziano Ferro feat. Anahí & Dulce María" has no separator we
    // dare cut on, so it stays one name and the file's three ids go unused.
    metadata.artist_credit = clean(tag.artist().as_deref());

    // Read before the names, because how many there are is what settles whether a
    // slash in a name divides two of them — see `as_many_as`.
    metadata.mbid_artists = every_id(tag, ItemKey::MusicBrainzArtistId);
    metadata.mbid_album_artists = every_id(tag, ItemKey::MusicBrainzReleaseArtistId);

    let listed = match every(tag, ItemKey::TrackArtists) {
        listed if !listed.is_empty() => listed,
        _ => split_artists(tag.artist().as_deref()),
    };

    metadata.artists = as_many_as(listed, metadata.mbid_artists.len());
    metadata.album_artists = as_many_as(
        split_artists(text(tag, ItemKey::AlbumArtist).as_deref()),
        metadata.mbid_album_artists.len(),
    );

    metadata.genres = split_artists(tag.genre().as_deref());

    metadata.sort_title = text(tag, ItemKey::TrackTitleSortOrder);
    metadata.sort_album = text(tag, ItemKey::AlbumTitleSortOrder);
    metadata.disc_subtitle = text(tag, ItemKey::SetSubtitle);
    metadata.comment = metadata
        .comment
        .take()
        .or_else(|| text(tag, ItemKey::Comment));

    // A date of "1996-07-15" still tells us the year, and taggers put the
    // year in either field depending on their mood.
    metadata.date = text(tag, ItemKey::RecordingDate);
    metadata.year = text(tag, ItemKey::Year)
        .as_deref()
        .and_then(|y| y.get(..4))
        .and_then(|y| y.parse().ok())
        .or_else(|| {
            metadata
                .date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok())
        });

    metadata.bpm = text(tag, ItemKey::Bpm).and_then(|v| v.parse().ok());
    metadata.isrc = text(tag, ItemKey::Isrc);
    // `Label` and `Publisher` are the same frame in every format that has one —
    // `TPUB` in ID3v2 — and taggers reach for either name, so both are asked.
    metadata.label = text(tag, ItemKey::Label).or_else(|| text(tag, ItemKey::Publisher));
    metadata.mbid_recording = text(tag, ItemKey::MusicBrainzRecordingId);
    metadata.mbid_track = text(tag, ItemKey::MusicBrainzTrackId);
    metadata.mbid_release = text(tag, ItemKey::MusicBrainzReleaseId);
    metadata.mbid_release_group = text(tag, ItemKey::MusicBrainzReleaseGroupId);

    metadata.rg_track_gain = decibels(text(tag, ItemKey::ReplayGainTrackGain).as_deref());
    metadata.rg_track_peak = text(tag, ItemKey::ReplayGainTrackPeak).and_then(|v| v.parse().ok());
    metadata.rg_album_gain = decibels(text(tag, ItemKey::ReplayGainAlbumGain).as_deref());
    metadata.rg_album_peak = text(tag, ItemKey::ReplayGainAlbumPeak).and_then(|v| v.parse().ok());

    // Both keys, and this is not belt and braces: lofty has no ID3v2 mapping for
    // `Lyrics` at all — its own table says to use `UnsyncLyrics`, which is the
    // `USLT` frame — so asking only for the first found nothing in an MP3, which is
    // where most of the world's embedded lyrics are. That was silent both here and
    // over `/rest`, because no lyrics and unreadable lyrics look identical from
    // outside. Vorbis comments do distinguish the two, `LYRICS` against
    // `UNSYNCEDLYRICS`, so trying them in this order reads either.
    (metadata.lyrics, metadata.lyrics_frame) = [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
        .into_iter()
        .find_map(|key| {
            let words = text(tag, key)?;
            Some((Some(words), key.map_key(tag.tag_type())))
        })
        .unwrap_or_default();

    metadata.is_compilation = text(tag, ItemKey::FlagCompilation)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
    // The front cover if the file says which one it is, otherwise whatever
    // picture it carries: a file with one unlabelled image means that image.
    metadata.picture = tag
        .pictures()
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())
        .map(|p| p.data().to_vec());
}

fn text(tag: &Tag, key: ItemKey) -> Option<String> {
    clean(tag.get_string(key))
}

/// Every value written under one key, in the order the file wrote them.
///
/// Two shapes to gather, because a tagger may use either and Picard has used both:
/// the key repeated, and one value with the separators inside it. Splitting each of
/// them means neither shape is read short — and the order matters here more than
/// anywhere else, since it is what lines names up with identifiers.
fn every(tag: &Tag, key: ItemKey) -> Vec<String> {
    every_divided_by(tag, key, &ARTIST_SEPARATORS)
}

/// The same for a key holding identifiers, which takes the slash as well.
fn every_id(tag: &Tag, key: ItemKey) -> Vec<String> {
    every_divided_by(tag, key, &ID_SEPARATORS)
}

fn every_divided_by(tag: &Tag, key: ItemKey, separators: &[char]) -> Vec<String> {
    tag.get_strings(key)
        .flat_map(|value| value.split(separators))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Names with a slash read as a separator, but only when the identifiers say it was
/// one.
///
/// ID3v2.3 writes "Alejandro Sanz/Juan Habichuela/Ketama" into one field, and nothing
/// in a name says whether a slash divides two of them or belongs to one of them —
/// AC/DC is a band, and cutting there invents two that do not exist. What does say it
/// is the other field: identifiers cannot contain a slash, so how many there are is
/// known exactly, and a name that comes apart into precisely that many pieces was a
/// list divided that way. Two independent fields agreeing on a count is not a guess.
///
/// Anything else is left alone. With one id and one name there is nothing to resolve;
/// with no ids there is no witness, and a slash stays part of the name.
fn as_many_as(names: Vec<String>, ids: usize) -> Vec<String> {
    if ids == 0 || names.len() == ids {
        return names;
    }

    let divided: Vec<String> = names
        .iter()
        .flat_map(|name| name.split('/'))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();

    if divided.len() == ids { divided } else { names }
}

/// Trims and discards anything that was only whitespace, because an empty tag
/// and a missing tag mean the same thing to us.
fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn split_artists(value: Option<&str>) -> Vec<String> {
    value
        .map(|v| {
            v.split(ARTIST_SEPARATORS)
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// ReplayGain values are written as "-7.32 dB", so the unit has to come off
/// before parsing.
fn decibels(value: Option<&str>) -> Option<f64> {
    value?
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() || c.is_whitespace())
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::prelude::TagExt;
    use lofty::tag::{TagItem, TagType};
    use std::path::PathBuf;

    /// Smallest thing lofty will accept as audio: a RIFF/WAVE header with one
    /// silent sample. Beats shipping a binary fixture, and needs no encoder
    /// installed.
    fn silent_wav(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tocata-tags-{name}.wav"));
        let mut bytes = Vec::new();
        let data: [u8; 4] = [0, 0, 0, 0];

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

        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn tagged_wav(name: &str, items: &[(ItemKey, &str)]) -> PathBuf {
        let listed: Vec<(ItemKey, Vec<&str>)> = items
            .iter()
            .map(|(key, value)| (*key, vec![*value]))
            .collect();

        wav_with_lists(name, &listed)
    }

    /// The same, for the keys a file may write more than once: the artists of a
    /// track and their identifiers. `push` rather than `insert`, which is the
    /// difference between a second value and a replacement.
    fn wav_with_lists(name: &str, items: &[(ItemKey, Vec<&str>)]) -> PathBuf {
        let path = silent_wav(name);
        let mut tag = Tag::new(TagType::Id3v2);

        for (key, values) in items {
            for value in values {
                tag.push(TagItem::new(
                    *key,
                    lofty::tag::ItemValue::Text(value.to_string()),
                ));
            }
        }

        tag.save_to_path(&path, Default::default()).unwrap();
        path
    }

    #[test]
    fn audio_properties_are_read_even_without_tags() {
        let metadata = read(&silent_wav("bare")).unwrap();
        assert_eq!(metadata.sampling_rate, Some(44_100));
        assert_eq!(metadata.channel_count, Some(2));
        assert_eq!(metadata.bit_depth, Some(16));
        assert_eq!(metadata.title, None, "an untagged file has no title");
    }

    #[test]
    fn the_common_fields_are_read() {
        let path = tagged_wav(
            "common",
            &[
                (ItemKey::TrackTitle, "Bohemian Rhapsody"),
                (ItemKey::TrackArtist, "Queen"),
                (ItemKey::AlbumTitle, "A Night at the Opera"),
                (ItemKey::AlbumArtist, "Queen"),
                (ItemKey::Genre, "Rock"),
                (ItemKey::TrackNumber, "11"),
                (ItemKey::DiscNumber, "1"),
            ],
        );

        let metadata = read(&path).unwrap();
        assert_eq!(metadata.title.as_deref(), Some("Bohemian Rhapsody"));
        assert_eq!(metadata.artists, vec!["Queen"]);
        assert_eq!(metadata.album.as_deref(), Some("A Night at the Opera"));
        assert_eq!(metadata.album_artists, vec!["Queen"]);
        assert_eq!(metadata.genres, vec!["Rock"]);
        assert_eq!(metadata.track_number, Some(11));
        assert_eq!(metadata.disc_number, Some(1));
    }

    #[test]
    fn several_artists_in_one_field_are_split() {
        let path = tagged_wav("multi", &[(ItemKey::TrackArtist, "David Bowie; Queen")]);
        assert_eq!(read(&path).unwrap().artists, vec!["David Bowie", "Queen"]);
    }

    #[test]
    fn whitespace_only_tags_count_as_absent() {
        let path = tagged_wav("blank", &[(ItemKey::TrackTitle, "   ")]);
        assert_eq!(read(&path).unwrap().title, None);
    }

    #[test]
    fn a_year_is_recovered_from_a_full_date() {
        let path = tagged_wav("date", &[(ItemKey::RecordingDate, "1975-11-21")]);
        let metadata = read(&path).unwrap();
        assert_eq!(metadata.date.as_deref(), Some("1975-11-21"));
        assert_eq!(metadata.year, Some(1975));
    }

    /// Builds a tag in memory, without a file behind it.
    ///
    /// The extraction of some keys cannot be tested through the WAV fixture:
    /// MusicBrainz identifiers live in ID3v2 as TXXX frames with specific
    /// descriptions, and they do not survive being written into a RIFF
    /// container. That is a limitation of the fixture, not of the reading, so
    /// these keys are checked against the tag directly. Do not "fix" this by
    /// moving the assertions onto a file: they will silently stop covering
    /// anything.
    fn tag_of(kind: TagType, items: &[(ItemKey, &str)]) -> Tag {
        let mut tag = Tag::new(kind);
        for (key, value) in items {
            tag.insert(TagItem::new(
                *key,
                lofty::tag::ItemValue::Text(value.to_string()),
            ));
        }
        tag
    }

    fn tag_with(items: &[(ItemKey, &str)]) -> Tag {
        tag_of(TagType::VorbisComments, items)
    }

    fn metadata_from(items: &[(ItemKey, &str)]) -> Metadata {
        read_from(&tag_with(items))
    }

    fn read_from(tag: &Tag) -> Metadata {
        let mut metadata = Metadata::default();
        read_tag(tag, &mut metadata);
        metadata
    }

    /// The regression that was invisible: lofty has no ID3v2 mapping for
    /// `ItemKey::Lyrics`, so reading only that key found nothing in an MP3 — which
    /// is where most embedded lyrics in the world are. Nothing said so, because a
    /// file with no lyrics and a file whose lyrics could not be reached answer the
    /// same way.
    #[test]
    fn lyrics_are_read_from_either_frame_and_the_frame_is_named() {
        let id3 = read_from(&tag_of(
            TagType::Id3v2,
            &[(ItemKey::UnsyncLyrics, "Is this the real life")],
        ));
        assert_eq!(id3.lyrics.as_deref(), Some("Is this the real life"));
        assert_eq!(
            id3.lyrics_frame,
            Some("USLT"),
            "the panel names the frame the words were in"
        );

        // Vorbis comments keep the two apart, and the timed one is preferred.
        let timed = metadata_from(&[(ItemKey::Lyrics, "[00:12.00] words")]);
        assert_eq!(timed.lyrics.as_deref(), Some("[00:12.00] words"));
        assert_eq!(timed.lyrics_frame, Some("LYRICS"));

        let plain = metadata_from(&[(ItemKey::UnsyncLyrics, "words")]);
        assert_eq!(plain.lyrics.as_deref(), Some("words"));
        assert_eq!(plain.lyrics_frame, Some("UNSYNCEDLYRICS"));

        let neither = metadata_from(&[(ItemKey::TrackTitle, "Silence")]);
        assert_eq!(neither.lyrics, None);
        assert_eq!(neither.lyrics_frame, None, "no words, nothing to name");
    }

    #[test]
    fn musicbrainz_identifiers_are_read() {
        let metadata = metadata_from(&[
            (
                ItemKey::MusicBrainzRecordingId,
                "b1a9c0e9-d987-4042-ae91-78d6a3267d69",
            ),
            (ItemKey::MusicBrainzTrackId, "0a1b2c3d"),
            (ItemKey::MusicBrainzReleaseId, "release-mbid"),
            (ItemKey::MusicBrainzReleaseGroupId, "group-mbid"),
        ]);

        assert_eq!(
            metadata.mbid_recording.as_deref(),
            Some("b1a9c0e9-d987-4042-ae91-78d6a3267d69")
        );
        assert_eq!(metadata.mbid_track.as_deref(), Some("0a1b2c3d"));
        assert_eq!(metadata.mbid_release.as_deref(), Some("release-mbid"));
        assert_eq!(metadata.mbid_release_group.as_deref(), Some("group-mbid"));
    }

    #[test]
    fn an_isrc_survives_a_real_file() {
        let path = tagged_wav("isrc", &[(ItemKey::Isrc, "GBUM71029604")]);
        assert_eq!(read(&path).unwrap().isrc.as_deref(), Some("GBUM71029604"));
    }

    /// Both fields, because taggers disagree about where the year goes and
    /// lofty does not persist ItemKey::Year into a RIFF container at all, so
    /// the file fixture cannot cover this half.
    #[test]
    fn a_year_is_read_from_either_field() {
        let from_year = metadata_from(&[(ItemKey::Year, "1975")]);
        assert_eq!(from_year.year, Some(1975));

        let from_date = metadata_from(&[(ItemKey::RecordingDate, "1975-11-21")]);
        assert_eq!(from_date.year, Some(1975));
        assert_eq!(from_date.date.as_deref(), Some("1975-11-21"));

        // A year in the tag wins over one derived from the date.
        let both = metadata_from(&[
            (ItemKey::Year, "1975"),
            (ItemKey::RecordingDate, "2004-01-01"),
        ]);
        assert_eq!(both.year, Some(1975));
    }

    #[test]
    fn replaygain_and_sort_names_are_read() {
        let metadata = metadata_from(&[
            (ItemKey::ReplayGainTrackGain, "-7.32 dB"),
            (ItemKey::ReplayGainTrackPeak, "0.98"),
            (ItemKey::ReplayGainAlbumGain, "-6.5 dB"),
            (ItemKey::TrackTitleSortOrder, "Bohemian Rhapsody"),
            (ItemKey::AlbumTitleSortOrder, "Night at the Opera, A"),
        ]);

        assert_eq!(metadata.rg_track_gain, Some(-7.32));
        assert_eq!(metadata.rg_track_peak, Some(0.98));
        assert_eq!(metadata.rg_album_gain, Some(-6.5));
        assert_eq!(metadata.sort_title.as_deref(), Some("Bohemian Rhapsody"));
        assert_eq!(
            metadata.sort_album.as_deref(),
            Some("Night at the Opera, A")
        );
    }

    #[test]
    fn a_compilation_flag_is_recognised() {
        let path = tagged_wav("comp", &[(ItemKey::FlagCompilation, "1")]);
        assert!(read(&path).unwrap().is_compilation);

        let path = tagged_wav("nocomp", &[(ItemKey::FlagCompilation, "0")]);
        assert!(!read(&path).unwrap().is_compilation);
    }

    #[test]
    fn replaygain_loses_its_unit() {
        assert_eq!(decibels(Some("-7.32 dB")), Some(-7.32));
        assert_eq!(
            decibels(Some("+2.5 dB")),
            Some(2.5),
            "a leading plus is fine"
        );
        assert_eq!(decibels(Some("-7.32")), Some(-7.32));
        assert_eq!(decibels(Some("nonsense")), None);
        assert_eq!(decibels(None), None);
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_a_panic() {
        let path = std::env::temp_dir().join("tocata-tags-not-audio.flac");
        std::fs::write(&path, b"this is not a flac file at all").unwrap();
        assert!(read(&path).is_err());
    }

    /// What Picard writes for a collaboration, taken from a real file: the credit
    /// whole in the artist field, the names one by one in `ARTISTS`, and the
    /// identifiers in that same order.
    #[test]
    fn the_names_come_from_the_list_and_the_credit_stays_whole() {
        let path = wav_with_lists(
            "collab",
            &[
                (
                    ItemKey::TrackArtist,
                    vec!["Tiziano Ferro feat. Anahí & Dulce María"],
                ),
                (
                    ItemKey::TrackArtists,
                    vec!["Tiziano Ferro", "Anahí", "Dulce María"],
                ),
                (
                    ItemKey::MusicBrainzArtistId,
                    vec![
                        "d12b05b0-a0af-4c2c-8c8c-ab8bcf49439e",
                        "4792522c-9eec-4491-9640-8922d5fbf2c5",
                        "f07d2a0a-955f-4adc-b70a-7aba348f343d",
                    ],
                ),
            ],
        );

        let metadata = read(&path).unwrap();

        assert_eq!(
            metadata.artists,
            ["Tiziano Ferro", "Anahí", "Dulce María"],
            "three people, not one name with two conjunctions in it"
        );
        assert_eq!(
            metadata.artist_credit.as_deref(),
            Some("Tiziano Ferro feat. Anahí & Dulce María"),
            "and the sentence the record uses about them, kept as it was written"
        );
        assert_eq!(metadata.mbid_artists.len(), 3);

        let paired: Vec<(&str, Option<&str>)> =
            identified(&metadata.artists, &metadata.mbid_artists).collect();
        assert_eq!(paired[1].0, "Anahí");
        assert_eq!(paired[1].1, Some("4792522c-9eec-4491-9640-8922d5fbf2c5"));
    }

    /// And without that list, which is most files: the credit is all there is, so
    /// it is split as it always was. Nothing is left cojo — a file that names one
    /// artist and gives one identifier still gets it.
    #[test]
    fn without_the_list_the_credit_is_split_as_before() {
        let one = tagged_wav(
            "solo",
            &[
                (ItemKey::TrackArtist, "Jeff Buckley"),
                (
                    ItemKey::MusicBrainzArtistId,
                    "e6e879c0-3d56-4f12-b3c5-3ce459661a8e",
                ),
            ],
        );

        let metadata = read(&one).unwrap();
        assert_eq!(metadata.artists, ["Jeff Buckley"]);
        assert_eq!(
            identified(&metadata.artists, &metadata.mbid_artists).collect::<Vec<_>>(),
            [("Jeff Buckley", Some("e6e879c0-3d56-4f12-b3c5-3ce459661a8e"))]
        );

        // Two names in the one field, which is the other shape a tagger uses, and
        // two identifiers to go with them.
        let two = wav_with_lists(
            "split",
            &[
                (ItemKey::TrackArtist, vec!["David Bowie; Queen"]),
                (
                    ItemKey::MusicBrainzArtistId,
                    vec!["bowie-mbid", "queen-mbid"],
                ),
            ],
        );

        let metadata = read(&two).unwrap();
        assert_eq!(metadata.artists, ["David Bowie", "Queen"]);
        assert_eq!(
            identified(&metadata.artists, &metadata.mbid_artists).collect::<Vec<_>>(),
            [
                ("David Bowie", Some("bowie-mbid")),
                ("Queen", Some("queen-mbid"))
            ],
            "the counts agree, so they pair up by position"
        );
    }

    /// ID3v2.3, which has no null separator and puts several values in one field
    /// divided by a slash. Taken from a real file.
    ///
    /// Read as one name it was worse than a name with slashes in it: the identifiers
    /// came through the same way, so three of them pasted together looked like one,
    /// the counts agreed, and a person who does not exist was marked with an
    /// identifier that identifies nobody — in a column the schema keeps unique.
    #[test]
    fn a_slash_divides_them_when_the_identifiers_say_it_does() {
        let path = tagged_wav(
            "id3v23-list",
            &[
                (
                    ItemKey::TrackArtist,
                    "Alejandro Sanz con Juan Habichuela y Ketama",
                ),
                (
                    ItemKey::TrackArtists,
                    "Alejandro Sanz/Juan Habichuela/Ketama",
                ),
                (
                    ItemKey::MusicBrainzArtistId,
                    "9bacf78f-9132-43da-8873-8a9eb49da0e9/\
                     2c915cf4-231e-49f3-93f8-e35cbd8e9ca2/\
                     7fe8e911-d706-44d9-b633-702065f8fd6c",
                ),
            ],
        );

        let metadata = read(&path).unwrap();

        assert_eq!(
            metadata.artists,
            ["Alejandro Sanz", "Juan Habichuela", "Ketama"]
        );
        assert_eq!(metadata.mbid_artists.len(), 3);
        assert_eq!(
            metadata.mbid_artists[2],
            "7fe8e911-d706-44d9-b633-702065f8fd6c"
        );
        assert_eq!(
            metadata.artist_credit.as_deref(),
            Some("Alejandro Sanz con Juan Habichuela y Ketama"),
            "and the credit is the sentence the record uses, slashes nowhere in it"
        );
    }

    /// The band the slash belongs to. One identifier, so there is one artist, and
    /// cutting there would invent two who do not exist.
    #[test]
    fn a_slash_inside_a_name_stays_inside_it() {
        let path = tagged_wav(
            "acdc",
            &[
                (ItemKey::TrackArtist, "AC/DC"),
                (
                    ItemKey::MusicBrainzArtistId,
                    "66c662b6-6e2f-4930-8610-912e24c63ed1",
                ),
            ],
        );

        let metadata = read(&path).unwrap();
        assert_eq!(metadata.artists, ["AC/DC"]);
        assert_eq!(metadata.mbid_artists.len(), 1);
    }

    /// And with nothing to check it against, a slash is left where it is. There is
    /// no witness, and inventing one is how AC/DC becomes two bands.
    #[test]
    fn without_identifiers_a_slash_is_left_alone() {
        let path = tagged_wav("no-witness", &[(ItemKey::TrackArtist, "Alice/Bob")]);

        assert_eq!(read(&path).unwrap().artists, ["Alice/Bob"]);
    }

    /// The case the pairing exists to refuse. One name that could not be split,
    /// three identifiers: giving it the first would mark somebody who does not
    /// exist with the identity of somebody who does.
    #[test]
    fn a_credit_that_could_not_be_split_takes_no_identifier() {
        let names = vec!["A feat. B & C".to_string()];
        let mbids = vec!["a-mbid".to_string(), "b-mbid".into(), "c-mbid".into()];

        assert_eq!(
            identified(&names, &mbids).collect::<Vec<_>>(),
            [("A feat. B & C", None)]
        );
    }

    /// A picture of the band, out of a file that also carries its sleeve. Getting
    /// this wrong is not a missing picture but a wrong one: the sleeve would end up
    /// as the photograph of the artist and on every record they made.
    #[test]
    fn a_picture_of_the_band_is_not_the_sleeve() {
        use lofty::picture::Picture;

        // Bytes only have to be recognisable as an image where somebody checks, and
        // what is checked here is which picture came back.
        let sleeve = b"\xff\xd8\xffSLEEVE".to_vec();
        let band = b"\xff\xd8\xffBAND".to_vec();

        let path = silent_wav("both-pictures");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(
            Picture::unchecked(sleeve.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Jpeg)
                .build(),
        );
        tag.push_picture(
            Picture::unchecked(band.clone())
                .pic_type(PictureType::Band)
                .mime_type(MimeType::Jpeg)
                .build(),
        );
        tag.save_to_path(&path, Default::default()).unwrap();

        assert_eq!(
            read_artist_picture(&path).unwrap(),
            Some(band),
            "the one of the band, out of the two"
        );
        // And the cover is still the cover. Read with the artwork asked for, since a
        // scan deliberately does not carry it.
        assert_eq!(
            read_with_cover_art(&path).unwrap().picture,
            Some(sleeve),
            "neither picture took the other's place"
        );
    }

    /// The ordinary file: a sleeve and nothing else. There is no picture of the
    /// artist in it, and saying so is what sends the question somewhere else.
    #[test]
    fn a_file_with_only_a_sleeve_has_no_picture_of_anybody() {
        use lofty::picture::Picture;

        let path = silent_wav("sleeve-only");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(
            Picture::unchecked(b"\xff\xd8\xffSLEEVE".to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Jpeg)
                .build(),
        );
        tag.save_to_path(&path, Default::default()).unwrap();

        assert_eq!(read_artist_picture(&path).unwrap(), None);
    }

    #[test]
    fn nothing_is_paired_when_the_counts_disagree_either_way() {
        let two = vec!["A".to_string(), "B".to_string()];

        assert!(
            identified(&two, &["only-one".to_string()]).all(|(_, mbid)| mbid.is_none()),
            "fewer identifiers than names"
        );
        assert!(
            identified(&two, &[]).all(|(_, mbid)| mbid.is_none()),
            "and none at all, which is most files"
        );
    }
}
