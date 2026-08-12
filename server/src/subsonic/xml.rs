// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Renders a `serde_json::Value` as the XML flavour of the API.
//!
//! Responses are modelled for JSON, which is what every current client
//! speaks. XML is still part of the specification and is what a client gets
//! when it does not ask for anything, so it cannot simply be dropped — but it
//! does not deserve a second set of types either.
//!
//! The mapping is mechanical, which is why this works at all: within an
//! object, scalars become attributes and anything structured becomes a child
//! element; an array becomes a repeated element under its key. Adding an
//! endpoint costs nothing here.
//!
//! With two exceptions the protocol forces. A few responses put their payload in
//! the element's text rather than in an attribute — `<genre songCount="28">Rock
//! </genre>`, and the body of a lyric — while JSON has to call it something, and
//! calls it `value`. So a key named `value` becomes the text of its element. And
//! three responses are written entirely of child elements: see [`TEXT_BODIED`].

use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use serde_json::Value;
use std::io::Cursor;

/// Key whose value belongs in the element's text instead of an attribute.
const TEXT_KEY: &str = "value";

/// The responses whose fields the protocol writes as child elements carrying
/// text, where every other response writes them as attributes:
///
/// ```xml
/// <artistInfo2><musicBrainzId>c8da2e40…</musicBrainzId></artistInfo2>
/// ```
///
/// Gated on the name of the element they sit in rather than on their own, because
/// `musicBrainzId` is an attribute in the three places it appears elsewhere — on
/// an artist, on a record and on a song. A rule about the key alone would rewrite
/// those and break every client that reads them.
///
/// JSON is unaffected: there a field is a field, and it is the same field.
const TEXT_BODIED: [&str; 3] = ["artistInfo", "artistInfo2", "albumInfo"];

/// Namespace every XML response carries. It is meaningless in JSON, so it
/// lives here rather than in the response types.
const XMLNS: &str = "http://subsonic.org/restapi";

const DECLARATION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>"#;

pub fn render(root: &str, value: &Value) -> quick_xml::Result<String> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    write_element(&mut writer, root, value, Some(XMLNS))?;

    let body = String::from_utf8(writer.into_inner().into_inner())
        .expect("quick-xml only ever emits UTF-8");

    Ok(format!("{DECLARATION}\n{body}"))
}

fn write_element(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    value: &Value,
    xmlns: Option<&str>,
) -> quick_xml::Result<()> {
    let mut start = BytesStart::new(name);
    if let Some(ns) = xmlns {
        start.push_attribute(("xmlns", ns));
    }

    match value {
        Value::Object(map) => {
            let text = map.get(TEXT_KEY).and_then(scalar_to_string);
            let spelt_out = TEXT_BODIED.contains(&name);

            if !spelt_out {
                for (key, child) in map {
                    if key == TEXT_KEY && text.is_some() {
                        continue;
                    }
                    if let Some(attribute) = scalar_to_string(child) {
                        start.push_attribute((key.as_str(), attribute.as_str()));
                    }
                }
            }

            let has_children = map
                .iter()
                .any(|(key, v)| key != TEXT_KEY && (spelt_out || scalar_to_string(v).is_none()));

            if !has_children && text.is_none() {
                writer.write_event(Event::Empty(start))?;
                return Ok(());
            }

            writer.write_event(Event::Start(start))?;

            if let Some(text) = &text {
                writer.write_event(Event::Text(BytesText::new(text)))?;
            }

            for (key, child) in map {
                match child {
                    // Every item of an array is an element named after the
                    // key, which is how the API expresses repetition.
                    Value::Array(items) => {
                        for item in items {
                            write_element(writer, key, item, None)?;
                        }
                    }
                    Value::Object(_) => write_element(writer, key, child, None)?,
                    // A scalar is an attribute everywhere but in the three
                    // responses above, where it is an element of its own.
                    _ if spelt_out => write_element(writer, key, child, None)?,
                    _ => {}
                }
            }
            writer.write_event(Event::End(BytesEnd::new(name)))?;
        }
        // A bare scalar has nowhere to go as an attribute, so it becomes the
        // element's text. `getLyrics` is the case that needs this.
        other => match scalar_to_string(other) {
            Some(text) => {
                writer.write_event(Event::Start(start))?;
                writer.write_event(Event::Text(BytesText::new(&text)))?;
                writer.write_event(Event::End(BytesEnd::new(name)))?;
            }
            None => writer.write_event(Event::Empty(start))?,
        },
    }

    Ok(())
}

/// Renders the values that can live in an XML attribute. Returns `None` for
/// anything structured, which is how callers tell attributes from children.
fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalars_become_attributes() {
        let xml = render(
            "subsonic-response",
            &json!({"status": "ok", "version": "1.16.1"}),
        )
        .unwrap();
        assert!(xml.contains(r#"<subsonic-response xmlns="http://subsonic.org/restapi""#));
        assert!(xml.contains(r#"status="ok""#));
        assert!(xml.contains(r#"version="1.16.1""#));
    }

    #[test]
    fn an_empty_body_closes_itself() {
        let xml = render("subsonic-response", &json!({"status": "ok"})).unwrap();
        assert!(xml.ends_with("/>"));
    }

    #[test]
    fn objects_become_child_elements() {
        let xml = render(
            "subsonic-response",
            &json!({"status": "ok", "license": {"valid": true}}),
        )
        .unwrap();
        assert!(xml.contains(r#"<license valid="true"/>"#));
        assert!(xml.contains("</subsonic-response>"));
    }

    #[test]
    fn arrays_repeat_their_element() {
        let xml = render(
            "subsonic-response",
            &json!({"artists": {"artist": [{"name": "Queen"}, {"name": "Pulp"}]}}),
        )
        .unwrap();
        assert!(xml.contains(r#"<artist name="Queen"/>"#), "got {xml}");
        assert!(xml.contains(r#"<artist name="Pulp"/>"#), "got {xml}");
    }

    #[test]
    fn a_repeated_element_can_carry_text() {
        let xml = render(
            "subsonic-response",
            &json!({"genres": {"genre": [{"value": "Rock"}, {"value": "Jazz"}]}}),
        )
        .unwrap();
        assert!(xml.contains("<genre>Rock</genre>"), "got {xml}");
        assert!(xml.contains("<genre>Jazz</genre>"), "got {xml}");
    }

    /// `<genre songCount="28" albumCount="6">Rock</genre>`: the payload is the
    /// element's text, and JSON has to give it a name, so it is called `value`.
    #[test]
    fn a_value_key_becomes_the_element_text() {
        let xml = render(
            "subsonic-response",
            &json!({"genres": {"genre": [{"value": "Rock", "songCount": 28}]}}),
        )
        .unwrap();

        assert!(xml.contains(r#"songCount="28""#), "got {xml}");
        assert!(xml.contains(">Rock</genre>"), "got {xml}");
        assert!(
            !xml.contains(r#"value="Rock""#),
            "not as an attribute: {xml}"
        );
    }

    #[test]
    fn a_value_key_alongside_children_keeps_both() {
        let xml = render(
            "subsonic-response",
            &json!({"outer": {"value": "text", "inner": {"a": 1}}}),
        )
        .unwrap();

        assert!(xml.contains(">text"), "got {xml}");
        assert!(xml.contains(r#"<inner a="1"/>"#), "got {xml}");
    }

    #[test]
    fn text_in_a_value_key_is_escaped_too() {
        let xml = render("subsonic-response", &json!({"g": {"value": "AC&DC <x>"}})).unwrap();
        assert!(xml.contains("AC&amp;DC &lt;x&gt;"), "got {xml}");
    }

    #[test]
    fn text_is_escaped() {
        let xml = render(
            "subsonic-response",
            &json!({"error": {"message": "AC&DC <bad>"}}),
        )
        .unwrap();
        assert!(xml.contains("AC&amp;DC &lt;bad&gt;"));
        assert!(!xml.contains("<bad>"));
    }

    #[test]
    fn booleans_and_numbers_keep_their_shape() {
        let xml = render(
            "subsonic-response",
            &json!({"openSubsonic": true, "count": 42, "gain": 1.5}),
        )
        .unwrap();
        assert!(xml.contains(r#"openSubsonic="true""#));
        assert!(xml.contains(r#"count="42""#));
        assert!(xml.contains(r#"gain="1.5""#));
    }

    /// The three responses the protocol spells out in elements. What makes this
    /// worth a test of its own is that the same key is an attribute anywhere else,
    /// so getting it right here and wrong there is one line apart.
    #[test]
    fn the_info_responses_spell_their_fields_out() {
        let xml = render(
            "subsonic-response",
            &json!({"artistInfo2": {"musicBrainzId": "c8da2e40", "biography": "Left home."}}),
        )
        .unwrap();

        assert!(
            xml.contains("<musicBrainzId>c8da2e40</musicBrainzId>"),
            "got {xml}"
        );
        assert!(
            xml.contains("<biography>Left home.</biography>"),
            "got {xml}"
        );
        assert!(
            !xml.contains(r#"musicBrainzId="c8da2e40""#),
            "and not as an attribute: {xml}"
        );
    }

    #[test]
    fn an_info_response_with_nothing_in_it_still_closes_itself() {
        let xml = render("subsonic-response", &json!({"albumInfo": {}})).unwrap();
        assert!(xml.contains("<albumInfo/>"), "got {xml}");
    }

    /// The same key elsewhere is untouched, which is the whole reason the rule is
    /// gated on the element and not on the key.
    #[test]
    fn the_same_key_on_a_song_stays_an_attribute() {
        let xml = render(
            "subsonic-response",
            &json!({"song": {"musicBrainzId": "c8da2e40"}}),
        )
        .unwrap();
        assert!(xml.contains(r#"musicBrainzId="c8da2e40""#), "got {xml}");
    }

    #[test]
    fn nulls_are_omitted() {
        let xml = render(
            "subsonic-response",
            &json!({"status": "ok", "comment": null}),
        )
        .unwrap();
        assert!(!xml.contains("comment"));
    }
}
