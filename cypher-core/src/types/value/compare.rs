use super::Value;

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b,
            // Int/Float cross-type: use safe comparison to avoid precision
            // loss for integers beyond 2^53.
            (Self::Int(a), Self::Float(b)) => {
                let af = *a as f64;
                af as i64 == *a && af == *b
            }
            (Self::Float(a), Self::Int(b)) => {
                let bf = *b as f64;
                bf as i64 == *b && *a == bf
            }
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::LocalTime(a), Self::LocalTime(b)) => a == b,
            (Self::Time(at, ao), Self::Time(bt, bo)) => at == bt && ao == bo,
            (Self::LocalDateTime(a), Self::LocalDateTime(b)) => a == b,
            (Self::DateTime(a), Self::DateTime(b)) => a == b,
            // CypherDuration: structural field equality
            (Self::Duration(a), Self::Duration(b)) => a == b,
            (Self::Point(a), Self::Point(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Map(a), Self::Map(b)) => a == b,
            (Self::Node(a), Self::Node(b)) => a == b,
            (Self::Edge(a), Self::Edge(b)) => a == b,
            (
                Self::Path {
                    nodes: n1,
                    edges: e1,
                },
                Self::Path {
                    nodes: n2,
                    edges: e2,
                },
            ) => n1 == n2 && e1 == e2,
            _ => false,
        }
    }
}

impl Value {
    /// Compare two values for Cypher equality semantics.
    /// Returns `None` when either operand is `Null` (Cypher three-valued logic).
    ///
    /// Handles:
    /// - NULL propagation: `null = X` → `None`
    /// - Int/Float coercion: `1 = 1.0` → `Some(true)`
    /// - List/Map null propagation: `[null] = [1]` → `None`
    pub fn cypher_eq(&self, other: &Self) -> Option<bool> {
        match (self, other) {
            // NULL = anything is NULL
            (Self::Null, _) | (_, Self::Null) => None,

            // Int/Float cross-type coercion: use safe comparison to avoid
            // precision loss for integers beyond 2^53.
            (Self::Int(a), Self::Float(b)) => {
                let af = *a as f64;
                Some(af as i64 == *a && af == *b)
            }
            (Self::Float(a), Self::Int(b)) => {
                let bf = *b as f64;
                Some(*a == bf && bf as i64 == *b)
            }

            // List equality with null propagation
            (Self::List(a), Self::List(b)) => {
                if a.len() != b.len() {
                    return Some(false);
                }
                let mut has_null = false;
                for (av, bv) in a.iter().zip(b.iter()) {
                    match av.cypher_eq(bv) {
                        Some(false) => return Some(false),
                        None => has_null = true,
                        Some(true) => {}
                    }
                }
                if has_null {
                    None
                } else {
                    Some(true)
                }
            }

            // Map equality with null propagation
            (Self::Map(a), Self::Map(b)) => {
                // Different key sets → false (even with nulls)
                if a.len() != b.len() {
                    return Some(false);
                }
                let mut has_null = false;
                for (k, av) in a.iter() {
                    match b.get(k.as_ref()) {
                        None => return Some(false), // key missing
                        Some(bv) => match av.cypher_eq(bv) {
                            Some(false) => return Some(false),
                            None => has_null = true,
                            Some(true) => {}
                        },
                    }
                }
                if has_null {
                    None
                } else {
                    Some(true)
                }
            }

            // Duration: direct field comparison
            (Self::Duration(a), Self::Duration(b)) => Some(*a == *b),

            // Bytes: direct comparison
            (Self::Bytes(a), Self::Bytes(b)) => Some(a == b),

            // All other types: delegate to PartialEq
            _ => Some(self == other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn null_equality_returns_none() {
        assert_eq!(Value::Null.cypher_eq(&Value::Int(1)), None);
        assert_eq!(Value::Int(1).cypher_eq(&Value::Null), None);
        assert_eq!(Value::Null.cypher_eq(&Value::Null), None);
    }

    #[test]
    fn int_equality() {
        assert_eq!(Value::Int(42).cypher_eq(&Value::Int(42)), Some(true));
        assert_eq!(Value::Int(42).cypher_eq(&Value::Int(43)), Some(false));
    }

    #[test]
    fn string_number_cross_type_equality_is_false() {
        assert_eq!(
            Value::Str("1".into()).cypher_eq(&Value::Int(1)),
            Some(false)
        );
        assert_eq!(
            Value::Int(1).cypher_eq(&Value::Str("1".into())),
            Some(false)
        );
        assert_eq!(
            Value::Str("1.0".into()).cypher_eq(&Value::Float(1.0)),
            Some(false)
        );
    }
}
