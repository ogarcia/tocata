// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Lyrics, plain or timed.
//!
//! A tag holds either, in the same field, and the difference is whether the
//! lines carry timestamps. LRC puts them in square brackets at the start of the
//! line: `[01:23.45] the words`.

/// Extensions of a lyric file sitting beside the music.
const LYRIC_EXTENSIONS: &[&str] = &["lrc", "txt"];

/// Finds a lyric file next to a track: `song.lrc` beside `song.flac`.
///
/// This is where a good part of the world keeps its lyrics, and where anything
/// downloaded later should be written, so that one reader covers both. Matching
/// is case insensitive, since `.LRC` is as common as `.lrc` and a case sensitive
/// filesystem would otherwise miss it.
///
/// Blocking: reads a directory.
pub fn find_beside(track: &std::path::Path) -> Option<String> {
    let stem = track.file_stem()?.to_str()?.to_lowercase();
    let directory = track.parent()?;

    let mut candidates: Vec<(usize, std::path::PathBuf)> = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let candidate_stem = path.file_stem()?.to_str()?.to_lowercase();
            let extension = path.extension()?.to_str()?.to_lowercase();

            if candidate_stem != stem {
                return None;
            }

            // Ranked, so a .lrc wins over a .txt for the same song.
            let rank = LYRIC_EXTENSIONS.iter().position(|e| *e == extension)?;
            Some((rank, path))
        })
        .collect();

    candidates.sort_by_key(|(rank, _)| *rank);

    candidates
        .into_iter()
        .find_map(|(_, path)| std::fs::read_to_string(path).ok())
        .filter(|content| !content.trim().is_empty())
}

/// One line of synchronised lyrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Milliseconds from the start of the song.
    pub start: i64,
    pub value: String,
}

/// Whether the content looks like LRC rather than plain text.
///
/// One timed line is enough: a file with a mix is still timed, and the untimed
/// lines are usually LRC metadata like `[ar:Artist]`.
pub fn looks_synchronised(content: &str) -> bool {
    content.lines().any(|line| parse_line(line).is_some())
}

/// Splits LRC into timed lines, dropping anything without a timestamp.
///
/// The tags LRC allows at the top — `[ar:]`, `[ti:]`, `[offset:]` — are not
/// lines of a song and fall out here, because they do not parse as a time.
pub fn parse(content: &str) -> Vec<Line> {
    content.lines().filter_map(parse_line).collect()
}

/// Reads `[mm:ss.cc]` or `[mm:ss]`, and returns the rest of the line with it.
fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_start();
    let rest = line.strip_prefix('[')?;
    let (stamp, text) = rest.split_once(']')?;

    let (minutes, rest) = stamp.split_once(':')?;
    let minutes: i64 = minutes.trim().parse().ok()?;

    // Seconds may carry hundredths or thousandths after a dot or a colon, and
    // taggers disagree about which.
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (rest, None),
    };
    let seconds: i64 = seconds.trim().parse().ok()?;

    let millis = match fraction {
        Some(fraction) => {
            let digits: String = fraction.chars().take_while(char::is_ascii_digit).collect();
            if digits.is_empty() {
                return None;
            }
            // Two digits are hundredths, three are thousandths.
            let value: i64 = digits.parse().ok()?;
            match digits.len() {
                1 => value * 100,
                2 => value * 10,
                _ => value,
            }
        }
        None => 0,
    };

    Some(Line {
        start: minutes * 60_000 + seconds * 1_000 + millis,
        value: text.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lyric_file_beside_the_track_is_found() {
        let directory = std::env::temp_dir().join("tocata-lyrics-beside");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let track = directory.join("song.flac");
        std::fs::write(&track, b"audio").unwrap();
        assert_eq!(find_beside(&track), None, "nothing there yet");

        std::fs::write(directory.join("song.lrc"), "[00:01.00]words").unwrap();
        assert_eq!(find_beside(&track).as_deref(), Some("[00:01.00]words"));
    }

    #[test]
    fn the_extension_may_be_in_any_case() {
        let directory = std::env::temp_dir().join("tocata-lyrics-case");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let track = directory.join("Song.flac");
        std::fs::write(&track, b"audio").unwrap();
        std::fs::write(directory.join("SONG.LRC"), "words").unwrap();

        assert_eq!(find_beside(&track).as_deref(), Some("words"));
    }

    #[test]
    fn an_lrc_wins_over_a_txt() {
        let directory = std::env::temp_dir().join("tocata-lyrics-rank");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let track = directory.join("song.flac");
        std::fs::write(&track, b"audio").unwrap();
        std::fs::write(directory.join("song.txt"), "plain").unwrap();
        std::fs::write(directory.join("song.lrc"), "timed").unwrap();

        assert_eq!(find_beside(&track).as_deref(), Some("timed"));
    }

    #[test]
    fn another_songs_lyrics_are_not_borrowed() {
        let directory = std::env::temp_dir().join("tocata-lyrics-other");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let track = directory.join("one.flac");
        std::fs::write(&track, b"audio").unwrap();
        std::fs::write(directory.join("two.lrc"), "not mine").unwrap();

        assert_eq!(find_beside(&track), None);
    }

    #[test]
    fn an_empty_lyric_file_counts_as_none() {
        let directory = std::env::temp_dir().join("tocata-lyrics-empty");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let track = directory.join("song.flac");
        std::fs::write(&track, b"audio").unwrap();
        std::fs::write(directory.join("song.lrc"), "   \n  ").unwrap();

        assert_eq!(find_beside(&track), None);
    }

    #[test]
    fn plain_text_is_not_synchronised() {
        assert!(!looks_synchronised(
            "Is this the real life\nIs this just fantasy"
        ));
        assert!(!looks_synchronised(""));
    }

    #[test]
    fn a_timed_line_makes_it_synchronised() {
        assert!(looks_synchronised("[00:12.00] Is this the real life"));
    }

    #[test]
    fn timestamps_become_milliseconds() {
        assert_eq!(
            parse("[00:12.34] one"),
            vec![Line {
                start: 12_340,
                value: "one".into()
            }]
        );
        assert_eq!(parse("[01:00] a minute")[0].start, 60_000);
        assert_eq!(parse("[02:03.5] tenths")[0].start, 123_500);
        assert_eq!(parse("[00:01.234] thousandths")[0].start, 1_234);
    }

    #[test]
    fn a_colon_before_the_fraction_works_too() {
        // Some taggers write [mm:ss:cc] instead of [mm:ss.cc].
        assert_eq!(parse("[00:12:34] one")[0].start, 12_340);
    }

    #[test]
    fn lrc_metadata_is_not_a_line_of_the_song() {
        let content = "[ar:Queen]\n[ti:Bohemian Rhapsody]\n[00:12.00] Is this the real life";
        let lines = parse(content);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].value, "Is this the real life");
    }

    #[test]
    fn untimed_lines_are_dropped_from_a_timed_file() {
        let lines = parse("[00:01.00] one\nstray text\n[00:02.00] two");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].start, 2_000);
    }

    #[test]
    fn an_empty_timed_line_is_kept_because_silence_is_part_of_a_song() {
        let lines = parse("[00:05.00]");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].value, "");
    }

    #[test]
    fn nonsense_in_brackets_is_not_a_timestamp() {
        assert!(parse("[not a time] words").is_empty());
        assert!(parse("[00:xx.00] words").is_empty());
        assert!(parse("no brackets at all").is_empty());
    }
}
