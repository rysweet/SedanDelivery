//! Extracts GQL-specific metadata (mandatory match, path mode, path search)
//! from raw source before the keyword rewriter mutates it.

use super::super::{GqlMetadata, PathMode, PathSearch};
use super::text_search::{contains_phrase_ci, contains_word_ci};

/// Extract GQL-specific metadata from the source before normalisation.
///
/// Uses case-insensitive matching on the original source to avoid
/// allocating a full uppercase copy.
pub fn extract_gql_metadata(src: &str) -> GqlMetadata {
    let mut meta = GqlMetadata::default();

    // Check for MANDATORY MATCH
    if contains_word_ci(src, "MANDATORY") && contains_word_ci(src, "MATCH") {
        meta.mandatory_match = true;
    }

    // Check for path modes (e.g. MATCH TRAIL, MATCH WALK)
    for (keyword, mode) in &[
        ("WALK", PathMode::Walk),
        ("TRAIL", PathMode::Trail),
        ("SIMPLE", PathMode::Simple),
        ("ACYCLIC", PathMode::Acyclic),
    ] {
        if contains_word_ci(src, keyword) {
            meta.path_mode = Some(*mode);
            break;
        }
    }

    // Check for path search modifiers (order matters: check compound first)
    if contains_phrase_ci(src, "ANY SHORTEST") {
        meta.path_search = Some(PathSearch::AnyShortest);
    } else if contains_phrase_ci(src, "ALL SHORTEST") {
        meta.path_search = Some(PathSearch::AllShortest);
    } else if contains_match_modifier_ci(src, "ANY") {
        meta.path_search = Some(PathSearch::Any);
    } else if contains_match_modifier_ci(src, "ALL") {
        meta.path_search = Some(PathSearch::All);
    }

    meta
}

/// Case-insensitive check if a keyword appears as a MATCH modifier
/// (i.e. "MATCH ANY (" pattern). Does not allocate.
pub(super) fn contains_match_modifier_ci(src: &str, keyword: &str) -> bool {
    // Find "MATCH" case-insensitively, then check if keyword follows
    let src_bytes = src.as_bytes();
    let match_kw = b"MATCH";
    if src_bytes.len() < match_kw.len() {
        return false;
    }
    for start in 0..=(src_bytes.len() - match_kw.len()) {
        if src_bytes[start..start + 5].eq_ignore_ascii_case(match_kw) {
            // Word boundary check before MATCH to avoid matching e.g. "REMATCH".
            let before_ok = start == 0 || !src_bytes[start - 1].is_ascii_alphanumeric();
            if !before_ok {
                continue;
            }
            // Word boundary check after MATCH to avoid matching e.g. "MATCHANY".
            let pos_after_match = start + 5;
            if pos_after_match < src_bytes.len()
                && src_bytes[pos_after_match].is_ascii_alphanumeric()
            {
                continue;
            }
            let after_match = &src[start + 5..];
            let trimmed = after_match.trim_start();
            if trimmed.len() >= keyword.len()
                && trimmed.as_bytes()[..keyword.len()].eq_ignore_ascii_case(keyword.as_bytes())
            {
                let after_kw = &trimmed[keyword.len()..];
                let next_char = after_kw.as_bytes().first();
                if next_char.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                    return true;
                }
            }
        }
    }
    false
}
