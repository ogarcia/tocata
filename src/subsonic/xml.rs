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

use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use serde_json::Value;
use std::io::Cursor;

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
            for (key, child) in map {
                if let Some(text) = scalar_to_string(child) {
                    start.push_attribute((key.as_str(), text.as_str()));
                }
            }

            let has_children = map.values().any(|v| scalar_to_string(v).is_none());
            if !has_children {
                writer.write_event(Event::Empty(start))?;
                return Ok(());
            }

            writer.write_event(Event::Start(start))?;
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
            &json!({"genres": {"genre": [{"value": "Rock"}, {"value": "Jazz"}]}}),
        )
        .unwrap();
        assert!(xml.contains(r#"<genre value="Rock"/>"#));
        assert!(xml.contains(r#"<genre value="Jazz"/>"#));
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
