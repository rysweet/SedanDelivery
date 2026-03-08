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
}
