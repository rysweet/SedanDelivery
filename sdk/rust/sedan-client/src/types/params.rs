//! Cypher query parameter substitution.

use super::value::SedanValue;

/// Substitute `$key` parameters in a Cypher query string with escaped values.
///
/// Only keys present in `params` are substituted; unknown `$` references are
/// left as-is so the server can report the error.
///
/// Uses a single-pass scan to prevent re-substitution (a replaced value
/// containing `$other` will NOT be replaced again). At each `$` position,
/// the longest matching parameter name is selected to avoid substring
/// collisions (e.g., `$name` vs `$nameLong`).
pub fn substitute_params(cypher: &str, params: &[(&str, SedanValue)]) -> String {
    if params.is_empty() {
        return cypher.to_string();
    }
    // Sort params longest-name-first so we try the longest match first at each $.
    let mut sorted: Vec<(&str, &SedanValue)> = params.iter().map(|(k, v)| (*k, v)).collect();
    sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    let mut result = String::with_capacity(cypher.len());
    let bytes = cypher.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // Try to match the longest parameter name at this position.
            let rest = &cypher[i + 1..];
            let mut matched = false;
            for (name, value) in &sorted {
                if rest.starts_with(name) {
                    // Word-boundary check: the char after the param name must not
                    // be alphanumeric or underscore.
                    let after_idx = i + 1 + name.len();
                    let at_boundary = cypher[after_idx..]
                        .chars()
                        .next()
                        .map_or(true, |c| !c.is_alphanumeric() && c != '_');
                    if at_boundary {
                        result.push_str(&value.to_cypher());
                        i = after_idx;
                        matched = true;
                        break;
                    }
                }
            }
            if !matched {
                result.push('$');
                i += 1;
            }
        } else {
            let ch = cypher[i..]
                .chars()
                .next()
                .expect("byte index is at valid UTF-8 boundary");
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_params_basic() {
        let result = substitute_params(
            "MATCH (n {name: $name})",
            &[("name", SedanValue::Str("Alice".into()))],
        );
        assert_eq!(result, "MATCH (n {name: 'Alice'})");
    }

    #[test]
    fn test_substitute_params_multiple() {
        let result = substitute_params(
            "WHERE n.age > $age AND n.active = $active",
            &[
                ("age", SedanValue::Int(21)),
                ("active", SedanValue::Bool(true)),
            ],
        );
        assert_eq!(result, "WHERE n.age > 21 AND n.active = true");
    }

    #[test]
    fn test_substitute_params_unknown_left_intact() {
        let result = substitute_params("$known $unknown", &[("known", SedanValue::Int(42))]);
        assert_eq!(result, "42 $unknown");
    }

    #[test]
    fn test_substitute_params_empty() {
        let result = substitute_params("MATCH (n) RETURN n", &[]);
        assert_eq!(result, "MATCH (n) RETURN n");
    }

    #[test]
    fn test_substitute_params_no_substring_collision() {
        // $name must NOT match inside $nameLong
        let result = substitute_params(
            "WHERE n.name = $name AND n.nameLong = $nameLong",
            &[
                ("name", SedanValue::Str("Alice".into())),
                ("nameLong", SedanValue::Str("Bob".into())),
            ],
        );
        assert_eq!(result, "WHERE n.name = 'Alice' AND n.nameLong = 'Bob'");
    }

    #[test]
    fn test_substitute_params_no_resubstitution() {
        // A replaced value containing $other must NOT be re-replaced
        let result = substitute_params(
            "RETURN $a, $b",
            &[
                ("a", SedanValue::Str("$b".into())),
                ("b", SedanValue::Int(99)),
            ],
        );
        assert_eq!(result, "RETURN '$b', 99");
    }
}
