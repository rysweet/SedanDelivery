//! Case-insensitive, allocation-free word and phrase searches used by the
//! GQL metadata extractor.

/// Case-insensitive word-boundary check without allocating.
pub(super) fn contains_word_ci(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let wbytes = word.as_bytes();
    if wbytes.len() > bytes.len() {
        return false;
    }
    for start in 0..=(bytes.len() - wbytes.len()) {
        if bytes[start..start + wbytes.len()].eq_ignore_ascii_case(wbytes) {
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let after_ok = start + wbytes.len() == bytes.len()
                || !bytes[start + wbytes.len()].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

/// Case-insensitive phrase search (words separated by whitespace).
pub(super) fn contains_phrase_ci(haystack: &str, phrase: &str) -> bool {
    // Split phrase into words and search for them in sequence
    let phrase_words: Vec<&str> = phrase.split_whitespace().collect();
    if phrase_words.is_empty() {
        return false;
    }
    // Simple approach: find first word, then check if second follows
    let bytes = haystack.as_bytes();
    let first = phrase_words[0].as_bytes();
    if first.len() > bytes.len() {
        return false;
    }
    for start in 0..=(bytes.len() - first.len()) {
        if bytes[start..start + first.len()].eq_ignore_ascii_case(first) {
            // Word boundary check before the first word so we don't match
            // e.g. "ANY SHORTEST" inside "MANY SHORTEST".
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            if !before_ok {
                continue;
            }
            // Check remaining words follow (with whitespace between)
            let mut pos = start + first.len();
            // The character immediately after the first word must also be a
            // word boundary so that the first phrase word is matched as a
            // whole token (e.g. "ANY" not "ANYTHING").
            if pos < bytes.len() && bytes[pos].is_ascii_alphanumeric() {
                continue;
            }
            let mut matched = true;
            for word in &phrase_words[1..] {
                // Skip whitespace
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                let wb = word.as_bytes();
                if pos + wb.len() > bytes.len()
                    || !bytes[pos..pos + wb.len()].eq_ignore_ascii_case(wb)
                {
                    matched = false;
                    break;
                }
                pos += wb.len();
            }
            if matched {
                // Trailing word boundary check on the last phrase word so that
                // "ALL SHORTEST" doesn't match inside "ALL SHORTESTPATH".
                let after_ok = pos == bytes.len() || !bytes[pos].is_ascii_alphanumeric();
                if after_ok {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod word_boundary_regression {
    use super::super::metadata::contains_match_modifier_ci;
    use super::contains_phrase_ci;

    // Regression for the word-boundary bug in `contains_phrase_ci`: matching a
    // phrase like "ANY SHORTEST" must not succeed inside a longer identifier
    // such as "MANY SHORTEST" or "ALL SHORTESTPATH".
    #[test]
    fn phrase_ci_requires_leading_word_boundary() {
        assert!(!contains_phrase_ci("MATCH MANY SHORTEST", "ANY SHORTEST"));
        assert!(!contains_phrase_ci("MATCH BALL SHORTEST", "ALL SHORTEST"));
        assert!(contains_phrase_ci("MATCH ANY SHORTEST", "ANY SHORTEST"));
        assert!(contains_phrase_ci("match any shortest", "ANY SHORTEST"));
    }

    #[test]
    fn phrase_ci_requires_trailing_word_boundary() {
        assert!(!contains_phrase_ci(
            "MATCH ALL SHORTESTPATH",
            "ALL SHORTEST"
        ));
        assert!(contains_phrase_ci("MATCH ALL SHORTEST (p)", "ALL SHORTEST"));
    }

    // Regression for the word-boundary bug in `contains_match_modifier_ci`:
    // an identifier like "REMATCH" must not satisfy the "MATCH ..." prefix.
    #[test]
    fn match_modifier_ci_requires_leading_word_boundary() {
        assert!(!contains_match_modifier_ci("REMATCH ANY (p)", "ANY"));
        assert!(!contains_match_modifier_ci("PREMATCH ALL (p)", "ALL"));
        assert!(contains_match_modifier_ci("MATCH ANY (p)", "ANY"));
        assert!(contains_match_modifier_ci("match all (p)", "ALL"));
    }

    // Regression for cycle-2 finding: "MATCH" without trailing whitespace must
    // not match the modifier even if the next chars look like the keyword.
    #[test]
    fn match_modifier_ci_requires_trailing_word_boundary_on_match() {
        assert!(!contains_match_modifier_ci("MATCHANY (p)", "ANY"));
        assert!(!contains_match_modifier_ci("MATCHALL (p)", "ALL"));
        assert!(!contains_match_modifier_ci("MATCHED ANY (p)", "ANY"));
    }
}
