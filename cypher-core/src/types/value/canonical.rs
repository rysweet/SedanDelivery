//! Canonical hashing and key-formatting for [`Value`].
//!
//! These provide deterministic, equality-consistent representations used by
//! GROUP BY, DISTINCT, and UNION operators.

use super::Value;

impl Value {
    /// Feed a deterministic, equality-consistent representation into a
    /// [`Hasher`](std::hash::Hasher), mirroring [`canonical_key`](Self::canonical_key)
    /// without allocating intermediate `String`s.
    ///
    /// Invariant: if `a == b` (per `PartialEq`), then `a` and `b` produce the
    /// same hash sequence.
    pub fn canonical_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Null => {}
            Self::Bool(b) => b.hash(state),
            Self::Int(i) => i.hash(state),
            Self::Float(f) => {
                let bits = if *f == 0.0 {
                    0.0_f64.to_bits()
                } else if f.is_nan() {
                    f64::NAN.to_bits()
                } else {
                    f.to_bits()
                };
                bits.hash(state);
            }
            Self::Str(s) => s.hash(state),
            Self::Date(d) => {
                d.year().hash(state);
                d.month().hash(state);
                d.day().hash(state);
            }
            Self::LocalTime(t) => {
                t.hour().hash(state);
                t.minute().hash(state);
                t.second().hash(state);
                t.subsec_nanosecond().hash(state);
            }
            Self::Time(t, offset) => {
                t.hour().hash(state);
                t.minute().hash(state);
                t.second().hash(state);
                t.subsec_nanosecond().hash(state);
                offset.hash(state);
            }
            Self::LocalDateTime(dt) => {
                dt.year().hash(state);
                dt.month().hash(state);
                dt.day().hash(state);
                dt.hour().hash(state);
                dt.minute().hash(state);
                dt.second().hash(state);
                dt.subsec_nanosecond().hash(state);
            }
            Self::DateTime(z) => {
                z.timestamp().as_nanosecond().hash(state);
            }
            Self::Duration(d) => d.hash(state),
            Self::Point(p) => {
                let nx = if p.x == 0.0 { 0.0_f64 } else { p.x };
                let ny = if p.y == 0.0 { 0.0_f64 } else { p.y };
                nx.to_bits().hash(state);
                ny.to_bits().hash(state);
                match p.z {
                    Some(z) => {
                        true.hash(state);
                        let nz = if z == 0.0 { 0.0_f64 } else { z };
                        nz.to_bits().hash(state);
                    }
                    None => false.hash(state),
                }
                std::mem::discriminant(&p.crs).hash(state);
            }
            Self::Bytes(b) => b.hash(state),
            Self::List(items) => {
                items.len().hash(state);
                for v in items {
                    v.canonical_hash(state);
                }
            }
            Self::Map(m) => {
                m.len().hash(state);
                let mut keys: Vec<&str> = m.keys().map(|k| k.as_ref()).collect();
                keys.sort();
                for k in keys {
                    k.hash(state);
                    // safe: k was obtained from m.keys() so get() always succeeds
                    m.get(k).unwrap().canonical_hash(state);
                }
            }
            Self::Node(id) => id.hash(state),
            Self::Edge(id) => id.hash(state),
            Self::Path { nodes, edges } => {
                nodes.hash(state);
                edges.hash(state);
            }
        }
    }

    /// Produce a deterministic, equality-consistent string key for use in
    /// hash-based grouping (aggregation GROUP BY, DISTINCT dedup, UNION dedup).
    ///
    /// Invariant: if `a == b` (per `PartialEq`), then
    /// `a.canonical_key() == b.canonical_key()`.
    ///
    /// The default `Debug` format violates this for:
    /// - `Float`: `-0.0` vs `0.0` (equal per IEEE 754 but different Debug)
    /// - `Map`: insertion-order-dependent Debug output
    /// - `DateTime`: same instant in different time zones
    pub fn canonical_key(&self) -> String {
        match self {
            Self::Float(f) => {
                let v = if *f == 0.0 {
                    // Normalize -0.0 to 0.0 (IEEE 754: -0.0 == 0.0)
                    0.0_f64
                } else if f.is_nan() {
                    // All NaN bit patterns map to the same key
                    f64::NAN
                } else {
                    *f
                };
                format!("Float({:?})", v)
            }
            Self::Map(m) => {
                // Sort entries by key so insertion order does not affect the key.
                let mut entries: Vec<(&str, String)> = m
                    .iter()
                    .map(|(k, v)| (k.as_ref(), v.canonical_key()))
                    .collect();
                entries.sort_by_key(|(k, _)| *k);
                let inner: Vec<String> =
                    entries.iter().map(|(k, v)| format!("{k:?}: {v}")).collect();
                format!("Map({{{}}})", inner.join(", "))
            }
            Self::DateTime(z) => {
                // Compare by UTC instant (epoch nanoseconds), not by
                // civil-time-plus-timezone representation.
                let nanos = z.timestamp().as_nanosecond();
                format!("DateTime(utc_ns={})", nanos)
            }
            Self::List(items) => {
                // Recurse so nested Float/Map/DateTime values are also canonical.
                let inner: Vec<String> = items.iter().map(|v| v.canonical_key()).collect();
                format!("List([{}])", inner.join(", "))
            }
            // All other variants: Debug format is already equality-consistent.
            other => format!("{:?}", other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use jiff::civil;

    // ── canonical_key tests ──────────────────────────────────────────────────

    #[test]
    fn canonical_key_float_neg_zero_equals_pos_zero() {
        let a = Value::Float(0.0);
        let b = Value::Float(-0.0);
        assert_eq!(a, b, "PartialEq says 0.0 == -0.0");
        assert_eq!(
            a.canonical_key(),
            b.canonical_key(),
            "canonical_key must agree with PartialEq"
        );
    }

    #[test]
    fn canonical_key_float_nan_consistent() {
        let a = Value::Float(f64::NAN);
        let b = Value::Float(f64::NAN);
        // NaN != NaN per IEEE 754 / PartialEq, but for hashing we need a
        // stable key so all NaN values land in the same bucket.
        assert_eq!(a.canonical_key(), b.canonical_key());
    }

    #[test]
    fn canonical_key_float_distinct_values() {
        let a = Value::Float(1.0);
        let b = Value::Float(2.0);
        assert_ne!(a.canonical_key(), b.canonical_key());
    }

    #[test]
    fn canonical_key_map_insertion_order_independent() {
        let mut m1 = IndexMap::new();
        m1.insert(Box::<str>::from("a"), Value::Int(1));
        m1.insert(Box::<str>::from("b"), Value::Int(2));

        let mut m2 = IndexMap::new();
        m2.insert(Box::<str>::from("b"), Value::Int(2));
        m2.insert(Box::<str>::from("a"), Value::Int(1));

        let v1 = Value::Map(m1);
        let v2 = Value::Map(m2);
        assert_eq!(v1, v2, "PartialEq says maps with same k/v are equal");
        assert_eq!(
            v1.canonical_key(),
            v2.canonical_key(),
            "canonical_key must agree with PartialEq"
        );
    }

    #[test]
    fn canonical_key_map_different_values_differ() {
        let mut m1 = IndexMap::new();
        m1.insert(Box::<str>::from("a"), Value::Int(1));

        let mut m2 = IndexMap::new();
        m2.insert(Box::<str>::from("a"), Value::Int(2));

        assert_ne!(
            Value::Map(m1).canonical_key(),
            Value::Map(m2).canonical_key()
        );
    }

    #[test]
    fn canonical_key_datetime_same_instant_different_tz() {
        use jiff::tz::TimeZone;

        // 2024-06-15T12:00:00 UTC
        let utc_tz = TimeZone::UTC;
        let zdt_utc = civil::DateTime::new(2024, 6, 15, 12, 0, 0, 0)
            .expect("valid")
            .to_zoned(utc_tz)
            .expect("valid");

        // Same instant expressed as 2024-06-15T14:00:00+02:00
        let plus2 = TimeZone::fixed(jiff::tz::Offset::from_seconds(7200).unwrap());
        let zdt_plus2 = civil::DateTime::new(2024, 6, 15, 14, 0, 0, 0)
            .expect("valid")
            .to_zoned(plus2)
            .expect("valid");

        assert_eq!(zdt_utc, zdt_plus2, "jiff::Zoned compares instants");

        let v1 = Value::DateTime(Box::new(zdt_utc));
        let v2 = Value::DateTime(Box::new(zdt_plus2));
        assert_eq!(v1, v2, "Value::DateTime delegates to Zoned PartialEq");
        assert_eq!(
            v1.canonical_key(),
            v2.canonical_key(),
            "canonical_key must agree with PartialEq"
        );
    }

    #[test]
    fn canonical_key_list_with_nested_float() {
        let a = Value::List(vec![Value::Float(0.0), Value::Int(1)]);
        let b = Value::List(vec![Value::Float(-0.0), Value::Int(1)]);
        assert_eq!(a, b);
        assert_eq!(a.canonical_key(), b.canonical_key());
    }

    #[test]
    fn canonical_key_simple_types_match_debug() {
        // For non-special types, canonical_key uses Debug format
        let vals = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(42),
            Value::Str("hello".into()),
        ];
        for v in &vals {
            assert_eq!(v.canonical_key(), format!("{:?}", v));
        }
    }

    // ── canonical_hash tests ─────────────────────────────────────────────────

    fn hash_value(v: &Value) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut h = DefaultHasher::new();
        v.canonical_hash(&mut h);
        h.finish()
    }

    #[test]
    fn canonical_hash_float_neg_zero() {
        assert_eq!(
            hash_value(&Value::Float(0.0)),
            hash_value(&Value::Float(-0.0))
        );
    }

    #[test]
    fn canonical_hash_float_nan() {
        assert_eq!(
            hash_value(&Value::Float(f64::NAN)),
            hash_value(&Value::Float(f64::NAN))
        );
    }

    #[test]
    fn canonical_hash_float_distinct() {
        assert_ne!(
            hash_value(&Value::Float(1.0)),
            hash_value(&Value::Float(2.0))
        );
    }

    #[test]
    fn canonical_hash_map_order_independent() {
        let mut m1 = IndexMap::new();
        m1.insert(Box::<str>::from("a"), Value::Int(1));
        m1.insert(Box::<str>::from("b"), Value::Int(2));

        let mut m2 = IndexMap::new();
        m2.insert(Box::<str>::from("b"), Value::Int(2));
        m2.insert(Box::<str>::from("a"), Value::Int(1));

        assert_eq!(hash_value(&Value::Map(m1)), hash_value(&Value::Map(m2)));
    }

    #[test]
    fn canonical_hash_datetime_same_instant() {
        use jiff::tz::TimeZone;
        let utc_tz = TimeZone::UTC;
        let zdt_utc = civil::DateTime::new(2024, 6, 15, 12, 0, 0, 0)
            .expect("valid")
            .to_zoned(utc_tz)
            .expect("valid");
        let plus2 = TimeZone::fixed(jiff::tz::Offset::from_seconds(7200).unwrap());
        let zdt_plus2 = civil::DateTime::new(2024, 6, 15, 14, 0, 0, 0)
            .expect("valid")
            .to_zoned(plus2)
            .expect("valid");
        assert_eq!(
            hash_value(&Value::DateTime(Box::new(zdt_utc))),
            hash_value(&Value::DateTime(Box::new(zdt_plus2)))
        );
    }

    #[test]
    fn canonical_hash_list_with_nested_float() {
        let a = Value::List(vec![Value::Float(0.0), Value::Int(1)]);
        let b = Value::List(vec![Value::Float(-0.0), Value::Int(1)]);
        assert_eq!(hash_value(&a), hash_value(&b));
    }

    #[test]
    fn canonical_hash_different_types_differ() {
        // Int(0) vs Bool(false) should produce different hashes
        assert_ne!(hash_value(&Value::Int(0)), hash_value(&Value::Bool(false)));
        // Int(1) vs Float(1.0) should produce different hashes (different types)
        assert_ne!(hash_value(&Value::Int(1)), hash_value(&Value::Float(1.0)));
    }
}
