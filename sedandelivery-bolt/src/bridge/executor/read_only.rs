//! Lightweight read-only query detection used by replica routing.

/// Check whether a Cypher query is read-only by inspecting its text for
/// mutating keywords (CREATE, SET, DELETE, MERGE, REMOVE).
///
/// This is a lightweight pre-parse check used for read-replica routing.
/// False negatives (classifying a write as a read) are not dangerous because
/// replicas will reject writes; false positives just skip the replica pool.
pub fn is_read_only_query(query: &str) -> bool {
    // Strip quoted strings (single and double) before checking for write
    // keywords, so that e.g. RETURN "CREATE" is not a false positive.
    let stripped = strip_quoted_strings(query);
    let upper = stripped.to_uppercase();
    !["CREATE", "SET", "DELETE", "MERGE", "REMOVE"]
        .iter()
        .any(|kw| upper.contains(kw))
}

/// Remove single- and double-quoted string literals from a Cypher query,
/// replacing their contents (and quotes) with spaces. This prevents write
/// keywords inside string literals from triggering false positives.
fn strip_quoted_strings(query: &str) -> String {
    let mut result = String::with_capacity(query.len());
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' || ch == '"' {
            let quote = ch;
            result.push(' ');
            loop {
                match chars.next() {
                    None => break,
                    Some('\\') => {
                        // Skip the escaped character.
                        chars.next();
                    }
                    Some(c) if c == quote => break,
                    Some(_) => {}
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_read_only_query_match_only() {
        assert!(is_read_only_query("MATCH (n) RETURN n"));
        assert!(is_read_only_query("match (n:Person) return n.name"));
        assert!(is_read_only_query("MATCH (a)-[r]->(b) RETURN a, r, b"));
    }

    #[test]
    fn is_read_only_query_with_create() {
        assert!(!is_read_only_query("CREATE (n:Person {name: 'Alice'})"));
        assert!(!is_read_only_query("MATCH (a) CREATE (a)-[:KNOWS]->(b)"));
    }

    #[test]
    fn is_read_only_query_with_set() {
        assert!(!is_read_only_query("MATCH (n) SET n.x = 1"));
    }

    #[test]
    fn is_read_only_query_with_delete() {
        assert!(!is_read_only_query("MATCH (n) DELETE n"));
        assert!(!is_read_only_query("MATCH (n) DETACH DELETE n"));
    }

    #[test]
    fn is_read_only_query_with_merge() {
        assert!(!is_read_only_query("MERGE (n:Person {name: 'Bob'})"));
    }

    #[test]
    fn is_read_only_query_with_remove() {
        assert!(!is_read_only_query("MATCH (n) REMOVE n.prop"));
        assert!(!is_read_only_query("MATCH (n) REMOVE n:Label"));
    }

    #[test]
    fn is_read_only_query_case_insensitive() {
        assert!(!is_read_only_query("match (n) set n.x = 1"));
        assert!(!is_read_only_query("Match (n) Delete n"));
    }

    #[test]
    fn is_read_only_query_return_only() {
        assert!(is_read_only_query("RETURN 1 AS x"));
    }
}
