// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Turning what somebody typed into something FTS5 will take.
//!
//! Shared by both APIs, because it is one decision about what a search means and
//! not two: `/rest` answers `search3` with it, and the panel's own collection
//! screens filter with it as the letters arrive.

/// The last word gets a prefix marker, because somebody typing into a search box
/// expects "beatl" to find the Beatles before they finish the word.
///
/// Returns `None` when nothing searchable is left, which is how an empty query
/// asks for everything.
pub fn wanted(terms: &str) -> Option<String> {
    let words: Vec<String> = terms
        .split_whitespace()
        // A word of pure punctuation tokenises to nothing, and an FTS5 literal
        // of "" is a syntax error.
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect();

    if words.is_empty() {
        return None;
    }

    let last = words.len() - 1;
    Some(
        words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                if index == last {
                    format!("{word}*")
                } else {
                    word.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::wanted;

    #[test]
    fn each_word_becomes_a_quoted_literal() {
        assert_eq!(wanted("abbey road").as_deref(), Some(r#""abbey" "road"*"#));
        assert_eq!(wanted("queen").as_deref(), Some(r#""queen"*"#));
    }

    /// Words that mean something to FTS5 must not be obeyed. Unquoted, "and"
    /// and "or" are operators and "NEAR" starts a function call.
    #[test]
    fn operator_words_are_searched_for_rather_than_obeyed() {
        assert_eq!(
            wanted("rock and roll").as_deref(),
            Some(r#""rock" "and" "roll"*"#)
        );
        assert_eq!(wanted("NEAR").as_deref(), Some(r#""NEAR"*"#));
        assert_eq!(wanted("a OR b").as_deref(), Some(r#""a" "OR" "b"*"#));
    }

    /// An unbalanced quote in a search box used to be a syntax error and a five
    /// hundred. Doubling it makes it a literal.
    #[test]
    fn quotes_cannot_break_the_expression() {
        assert_eq!(
            wanted(r#"say "hello"#).as_deref(),
            Some(r#""say" """hello"*"#)
        );
        assert_eq!(wanted(r#"""#), None, "a lone quote has nothing to search");
    }

    #[test]
    fn punctuation_alone_is_not_a_search_term() {
        assert_eq!(wanted("-").as_deref(), None);
        assert_eq!(wanted("!!! queen").as_deref(), Some(r#""queen"*"#));
        // But punctuation attached to a word travels with it.
        assert_eq!(wanted("AC/DC").as_deref(), Some(r#""AC/DC"*"#));
    }

    #[test]
    fn an_empty_query_asks_for_everything() {
        assert_eq!(wanted(""), None);
        assert_eq!(wanted("   "), None);
    }

    /// Both come back as None from the escaping, and the caller tells them apart
    /// by whether anything was typed: nothing typed means everything, something
    /// unsearchable means nothing.
    #[test]
    fn an_unsearchable_query_is_not_the_same_as_no_query() {
        assert_eq!(wanted(r#"""#), None);
        assert!(!r#"""#.trim().is_empty(), "but something was typed");
        assert!("".trim().is_empty(), "whereas here nothing was");
    }

    #[test]
    fn accents_and_other_alphabets_are_kept() {
        assert_eq!(wanted("Björk").as_deref(), Some(r#""Björk"*"#));
        assert_eq!(wanted("日本").as_deref(), Some(r#""日本"*"#));
    }
}
