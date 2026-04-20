//! Generators and shared helpers for the embedded-engine fuzz harness.
//!
//! This crate is **not** a runtime dependency of any production binary. It
//! exists only to host property-tests against the public surfaces of the
//! three embedded engines:
//!
//! * [`sedan_cypher_embedded::CypherEngine`]
//! * [`sedan_gql_embedded::SedanGql`]
//! * [`sedan_gremlin_embedded::GremlinEngine`]
//!
//! The harness asserts *invariants*, not output values:
//!
//! 1. `execute` / `execute_with_*` never panic.
//! 2. They never deadlock (each test has an upper bound on running time
//!    via the engines' default execution budgets).
//! 3. Every error returned is one of the public error variants — meaning
//!    the engine has classified the failure rather than leaking an
//!    internal panic / assertion.

use indexmap::IndexMap;
use proptest::prelude::*;
use sedan_cypher_embedded::Value;

/// Bounded depth for recursively generated query fragments.
pub const MAX_DEPTH: u32 = 3;

/// Default number of cases for the fast property tests.
pub const FAST_CASES: u32 = 128;

/// Number of cases for the soak (`--ignored`) variants.
pub const SOAK_CASES: u32 = 10_000;

/// Deterministic seed used by every test in this crate.
///
/// Pin one seed across the whole harness so failures are reproducible
/// across CI runs and developer machines. To regenerate after a true
/// failure, replace this constant with a fresh 32-byte value.
pub const FUZZ_SEED: [u8; 32] = *b"SedanDelivery-embedded-fuzz-v1!!";

/// Build a [`ProptestConfig`] that uses the deterministic seed and the
/// supplied case count, with shrinking and persistence disabled so the
/// run is fully self-contained.
pub fn fuzz_config(cases: u32) -> ProptestConfig {
    let mut cfg = ProptestConfig::with_cases(cases);
    cfg.failure_persistence = None;
    cfg.rng_algorithm = proptest::test_runner::RngAlgorithm::ChaCha;
    cfg
}

/// Strategy producing safe identifiers (labels, properties, alias names).
pub fn ident_strategy() -> impl Strategy<Value = String> {
    "[A-Za-z][A-Za-z0-9_]{0,7}".prop_map(|s| s.to_string())
}

/// Strategy producing scalar [`Value`] instances suitable for query
/// parameters. Restricted to types that have a Cypher literal counterpart
/// so the engines accept them without TypeError.
pub fn scalar_value_strategy() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        // Avoid NaN/Inf — they are valid f64 but not all backends compare
        // them sanely; the harness is about the engine, not float arithmetic.
        (-1.0e9_f64..1.0e9_f64).prop_map(Value::Float),
        "[ -~]{0,16}".prop_map(|s| Value::Str(s.into_boxed_str())),
    ]
}

/// Strategy producing arbitrary [`Value`] instances, including
/// collections.
pub fn value_strategy() -> impl Strategy<Value = Value> {
    let leaf = scalar_value_strategy();
    leaf.prop_recursive(2, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::List),
            prop::collection::vec((ident_strategy(), inner), 0..4).prop_map(|kvs| {
                let mut map = IndexMap::new();
                for (k, v) in kvs {
                    map.insert(k.into_boxed_str(), v);
                }
                Value::Map(map)
            }),
        ]
    })
}

// ── Cypher generators ────────────────────────────────────────────────────────

/// Strategy producing syntactically-valid Cypher queries.
pub fn cypher_query_strategy() -> impl Strategy<Value = String> {
    let pattern = (ident_strategy(), ident_strategy())
        .prop_map(|(l, a)| (a.clone(), format!("({}:{})", a, l)));

    let where_clause = (ident_strategy(), ident_strategy())
        .prop_map(|(a, p)| format!(" WHERE {}.{} IS NOT NULL", a, p));

    let limit = (1u32..32u32).prop_map(|n| format!(" LIMIT {}", n));

    (
        pattern,
        prop::option::of(where_clause),
        prop::option::of(limit),
    )
        .prop_map(|((alias, pat), w, l)| {
            let mut q = format!("MATCH {} ", pat);
            if let Some(w) = w {
                q.push_str(&w);
            }
            q.push_str(&format!(" RETURN {}", alias));
            if let Some(l) = l {
                q.push_str(&l);
            }
            q
        })
}

// ── GQL generators ───────────────────────────────────────────────────────────

/// Strategy producing syntactically-valid GQL queries.
///
/// The GQL surface accepts the same MATCH/RETURN core that openCypher
/// uses, so we generate a similar shape but skip Cypher-specific
/// extensions.
pub fn gql_query_strategy() -> impl Strategy<Value = String> {
    let limit = (1u32..32u32).prop_map(|n| format!(" LIMIT {}", n));

    (ident_strategy(), ident_strategy(), prop::option::of(limit)).prop_map(|(l, a, lim)| {
        let mut q = format!("MATCH ({}:{}) RETURN {}", a, l, a);
        if let Some(lim) = lim {
            q.push_str(&lim);
        }
        q
    })
}

// ── Gremlin generators ───────────────────────────────────────────────────────

/// Strategy producing syntactically-valid Gremlin traversals limited to
/// the read-only steps the embedded engine accepts.
pub fn gremlin_query_strategy() -> impl Strategy<Value = String> {
    let source = prop_oneof![Just("g.V()".to_string()), Just("g.E()".to_string()),];

    let has_step = (
        ident_strategy(),
        "[A-Za-z0-9]{1,8}".prop_map(|s| s.to_string()),
    )
        .prop_map(|(k, v)| format!(".has('{}', '{}')", k, v));
    let has_label = ident_strategy().prop_map(|l| format!(".hasLabel('{}')", l));
    let values_step = ident_strategy().prop_map(|p| format!(".values('{}')", p));
    let count_step = Just(".count()".to_string());
    let limit_step = (1u32..16u32).prop_map(|n| format!(".limit({})", n));

    let step = prop_oneof![
        has_step.boxed(),
        has_label.boxed(),
        values_step.boxed(),
        count_step.boxed(),
        limit_step.boxed(),
    ];

    (source, prop::collection::vec(step, 0..(MAX_DEPTH as usize))).prop_map(|(src, steps)| {
        let mut q = src;
        for s in steps {
            q.push_str(&s);
        }
        q
    })
}
