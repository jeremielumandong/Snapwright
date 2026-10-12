//! Turning what people type into an FTS5 query.

/// Builds an FTS5 query from free text: every word must match the start of a word in some
/// field. Quotes keep FTS5 syntax characters in the input from being read as operators.
/// Returns `None` when the text has no words.
pub fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_become_quoted_prefix_terms() {
        assert_eq!(
            fts_query("Figma login").as_deref(),
            Some("\"figma\"* AND \"login\"*")
        );
    }

    #[test]
    fn syntax_characters_are_separators() {
        assert_eq!(
            fts_query("a\"b OR (c*) NEAR -d").as_deref(),
            Some("\"a\"* AND \"b\"* AND \"or\"* AND \"c\"* AND \"near\"* AND \"d\"*")
        );
        assert_eq!(fts_query("  ** \"\" ()  "), None);
    }

    #[test]
    fn non_ascii_words_survive() {
        assert_eq!(fts_query("Café"), Some("\"café\"*".into()));
    }
}
