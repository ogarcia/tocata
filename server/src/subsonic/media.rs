// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Serving the files themselves.

use super::auth::Authenticated;
use super::browsing::IdQuery;
use super::error::ApiError;
use super::response::{self};
use crate::artwork;
use crate::config::Config;
use axum::extract::{Query, Request, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::{error, warn};

pub async fn stream(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
    request: Request,
) -> Response {
    match crate::media::locate(&pool, auth.user.id, &query.id).await {
        Ok(Some(track)) => crate::media::serve(track, request).await,
        Ok(None) => ApiError::NotFound.in_format(auth.format).into_response(),
        Err(crate::media::Refused::Traversal) => {
            ApiError::NotFound.in_format(auth.format).into_response()
        }
        Err(crate::media::Refused::Database(e)) => {
            error!("locating a track to stream: {e}");
            ApiError::Internal.in_format(auth.format).into_response()
        }
    }
}

pub async fn download(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
    request: Request,
) -> Response {
    let track = match crate::media::locate(&pool, auth.user.id, &query.id).await {
        Ok(Some(track)) => track,
        Ok(None) | Err(crate::media::Refused::Traversal) => {
            return ApiError::NotFound.in_format(auth.format).into_response();
        }
        Err(crate::media::Refused::Database(e)) => {
            error!("locating a track to download: {e}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };

    let filename = track
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let mut response = crate::media::serve(track, request).await;
    if let Ok(value) = content_disposition(&filename).parse() {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }

    response
}

/// Builds the header for a download, giving both the plain and the encoded
/// form: a name with an accent in it is not representable in the first.
fn content_disposition(filename: &str) -> String {
    let sanitised: String = filename
        .chars()
        .map(|c| match c {
            // Quotes and control characters would end the field early or split
            // the header.
            '"' | '\\' | '\r' | '\n' => '_',
            c if c.is_control() => '_',
            c if c.is_ascii() => c,
            // Anything outside ASCII has no place in the plain form.
            _ => '_',
        })
        .collect();

    format!(
        "attachment; filename=\"{sanitised}\"; filename*=UTF-8''{}",
        percent_encode(filename)
    )
}

fn percent_encode(value: &str) -> String {
    use std::fmt::Write;

    value.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

// ---------------------------------------------------------------------------
// Cover art
// ---------------------------------------------------------------------------

/// Serves an album's cover, extracting it the first time somebody asks.
///
/// Nothing is extracted during a scan. A library of five thousand albums would
/// otherwise have every cover copied into the cache — gigabytes duplicating what
/// is already on disk — for albums nobody may open this month. The first request
/// for one pays for it once; every request after that is served from the cache.
///
/// `size` is not honoured: the original comes back, which the specification
/// allows. Scaling means decoding and re-encoding images, and that waits for a
/// measurement saying it matters.
pub async fn get_cover_art(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    State(config): State<Arc<Config>>,
    Query(query): Query<IdQuery>,
    request: Request,
) -> Response {
    let found = picture_for(&pool, config.data_dir(), auth.user.id, &query.id).await;

    let (hash, mime_type) = match found {
        Ok(Some(found)) => found,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => {
            error!("finding cover art for {}: {e:#}", query.id);
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };

    let path = artwork::cache_path(config.data_dir(), &hash);
    let service = ServeFile::new_with_mime(
        &path,
        &mime_type.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM),
    );

    match service.oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            error!("serving cover art from {}: {e}", path.display());
            ApiError::NotFound.in_format(auth.format).into_response()
        }
    }
}

/// Which picture an identifier is asking for, whichever kind of thing it names.
///
/// The call takes one parameter and the protocol does not say what kind of id goes
/// in it, so a client passes whatever it has: the id of an album, of one of its
/// songs, or of an artist. All three are tried, album first because that is nearly
/// every request.
///
/// An artist used to fall through to a refusal, and the artist's picture was
/// reachable only over the native API — so a client had no way to draw one however
/// many we had found.
///
/// Each kind knows about its own cache: an album's cover is looked up and extracted
/// in two steps, and an artist's picture does both behind one call.
async fn picture_for(
    pool: &SqlitePool,
    data_dir: &std::path::Path,
    user_id: i64,
    id: &str,
) -> anyhow::Result<Option<(String, String)>> {
    if let Some(album) = crate::media::resolve_album(pool, user_id, id).await? {
        if let Some(cached) = crate::media::cover_in_cache(pool, album).await? {
            return Ok(Some(cached));
        }

        return crate::media::extract_cover(pool, data_dir, album).await;
    }

    if let Some(artist) = crate::media::resolve_artist(pool, user_id, id).await? {
        return crate::media::artist_picture(pool, data_dir, artist).await;
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Lyrics
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LyricsQuery {
    artist: Option<String>,
    title: Option<String>,
}

#[derive(Serialize)]
struct LyricsBody {
    lyrics: Lyrics,
}

/// The old shape: everything in one blob, with the words as the element's text.
#[derive(Serialize)]
struct Lyrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LyricsListBody {
    lyrics_list: LyricsList,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LyricsList {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    structured_lyrics: Vec<StructuredLyrics>,
}

/// The OpenSubsonic shape, which can carry timings.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StructuredLyrics {
    lang: String,
    synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_title: Option<String>,
    line: Vec<LyricLine>,
}

#[derive(Serialize)]
struct LyricLine {
    /// Milliseconds from the start, absent on unsynchronised lyrics.
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<i64>,
    value: String,
}

/// Reads the lyrics out of a file, off the async threads.
///
/// Not stored in the database on purpose. Lyrics are the one piece of long text
/// a music file carries, and a copy of them in the database would be hundreds of
/// megabytes duplicating what is already on disk. Reading the tags of one file is
/// a seek and a few kilobytes, and it only happens when somebody actually looks
/// at the words — which also means an edited lyric shows up without a rescan.
async fn read_lyrics(path: PathBuf) -> Option<String> {
    let read = tokio::task::spawn_blocking(move || {
        // A file beside the music wins over an embedded tag: it is what somebody
        // put there deliberately and can edit, and it is where anything fetched
        // from the network will be written.
        if let Some((_, content)) = crate::lyrics::find_beside(&path) {
            return Some(content);
        }

        match crate::scanner::read_tags(&path) {
            Ok(metadata) => metadata.lyrics,
            Err(e) => {
                warn!("reading lyrics from {}: {e:#}", path.display());
                None
            }
        }
    })
    .await;

    match read {
        Ok(content) => content,
        Err(e) => {
            error!("the lyric reader panicked: {e}");
            None
        }
    }
}

/// Lyrics by artist and title, which is how the older call asks for them.
///
/// Answers with an empty element rather than an error when there are none: the
/// older clients that use this treat a 70 as a failure worth showing the user,
/// and having no lyrics is not a failure.
pub async fn get_lyrics(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<LyricsQuery>,
) -> Response {
    let found: Result<Option<(String, String, Option<String>)>, _> = sqlx::query_as(concat!(
        visible_libraries!(),
        "SELECT l.path || '/' || t.path, t.title,
                (SELECT ar.name FROM track_artists ta
                   JOIN artists ar ON ar.id = ta.artist_id
                  WHERE ta.track_id = t.id ORDER BY ta.position LIMIT 1)
           FROM tracks t
           JOIN libraries l ON l.id = t.library_id
          WHERE t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)
            AND (? IS NULL OR t.title = ?)
            AND (? IS NULL OR EXISTS (
                    SELECT 1 FROM track_artists ta
                      JOIN artists ar ON ar.id = ta.artist_id
                     WHERE ta.track_id = t.id AND ar.name = ?
                ))
          LIMIT 1"
    ))
    .bind(auth.user.id)
    .bind(&query.title)
    .bind(&query.title)
    .bind(&query.artist)
    .bind(&query.artist)
    .fetch_optional(&pool)
    .await;

    let (path, title, artist) = match found {
        Ok(Some(row)) => row,
        Ok(None) => {
            return response::ok(
                auth.format,
                LyricsBody {
                    lyrics: Lyrics {
                        artist: query.artist,
                        title: query.title,
                        value: String::new(),
                    },
                },
            );
        }
        Err(e) => {
            error!("looking up a song for its lyrics: {e}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };

    response::ok(
        auth.format,
        LyricsBody {
            lyrics: Lyrics {
                artist,
                title: Some(title),
                value: read_lyrics(PathBuf::from(path)).await.unwrap_or_default(),
            },
        },
    )
}

pub async fn get_lyrics_by_song_id(
    auth: Authenticated,
    State(pool): State<SqlitePool>,
    Query(query): Query<IdQuery>,
) -> Response {
    let found: Result<Option<String>, _> = sqlx::query_scalar(concat!(
        visible_libraries!(),
        "SELECT l.path || '/' || t.path
           FROM tracks t
           JOIN libraries l ON l.id = t.library_id
          WHERE t.public_id = ? AND t.missing_since IS NULL
            AND t.library_id IN (SELECT id FROM visible_libraries)"
    ))
    .bind(auth.user.id)
    .bind(&query.id)
    .fetch_optional(&pool)
    .await;

    let path = match found {
        Ok(Some(path)) => path,
        Ok(None) => return ApiError::NotFound.in_format(auth.format).into_response(),
        Err(e) => {
            error!("looking up a song for its lyrics: {e}");
            return ApiError::Internal.in_format(auth.format).into_response();
        }
    };

    let Some(content) = read_lyrics(PathBuf::from(path)).await else {
        // No lyrics is an empty list, not an error: the song exists.
        return response::ok(
            auth.format,
            LyricsListBody {
                lyrics_list: LyricsList {
                    structured_lyrics: Vec::new(),
                },
            },
        );
    };

    let synced = crate::lyrics::looks_synchronised(&content);
    let line = if synced {
        crate::lyrics::parse(&content)
            .into_iter()
            .map(|line| LyricLine {
                start: Some(line.start),
                value: line.value,
            })
            .collect()
    } else {
        content
            .lines()
            .map(|value| LyricLine {
                start: None,
                value: value.to_string(),
            })
            .collect()
    };

    response::ok(
        auth.format,
        LyricsListBody {
            lyrics_list: LyricsList {
                structured_lyrics: vec![StructuredLyrics {
                    // Nothing in a tag says which language the words are in.
                    lang: "xxx".to_string(),
                    synced,
                    display_artist: None,
                    display_title: None,
                    line,
                }],
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_name_is_safe_to_put_in_a_header() {
        let header = content_disposition("song.flac");
        assert!(header.contains(r#"filename="song.flac""#));
        assert!(header.contains("filename*=UTF-8''song.flac"));
    }

    #[test]
    fn quotes_and_newlines_cannot_break_out_of_the_header() {
        let header = content_disposition("evil\"; rm -rf /\r\nX-Injected: yes.flac");

        // Exactly the two quotes that delimit the plain form, so none came from
        // the file name and closed the field early.
        assert_eq!(
            header.chars().filter(|c| *c == '"').count(),
            2,
            "got {header}"
        );
        // A newline here would split the header and let the rest of the name
        // become a header of its own.
        assert!(!header.contains('\r'), "got {header}");
        assert!(!header.contains('\n'), "got {header}");
        assert!(!header.contains("X-Injected: yes\""), "got {header}");
    }

    #[test]
    fn an_accented_name_survives_in_the_encoded_form() {
        let header = content_disposition("Björk.flac");
        // Unrepresentable in the plain form, so it degrades there...
        assert!(header.contains(r#"filename="Bj_rk.flac""#), "got {header}");
        // ...and travels intact in the encoded one.
        assert!(header.contains("filename*=UTF-8''Bj%C3%B6rk.flac"));
    }
}
