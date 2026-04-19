//! Catalog of parity cases. Each case is a triple of semantically equivalent
//! queries (Cypher / GQL / Gremlin) plus a [`DivergenceMode`] that says
//! whether divergence is a bug (`Strict`) or an accepted, documented
//! semantic difference (`Documented`).

/// How strictly the parity runner should treat cross-engine divergence
/// for a given case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceMode {
    /// All three engines MUST produce the same canonical result.
    /// Any difference is a bug.
    Strict,
    /// A documented semantic difference between engines. The runner records
    /// the divergence but does not fail the test. See
    /// `docs/DIFFERENTIAL_TESTING.md` for the rationale on each case.
    Documented(&'static str),
}

/// One parity case: a logical query expressed in all three engines.
///
/// Each query MUST emit columns in the same logical order so the runner
/// can compare positionally (Gremlin does not always emit named columns).
pub struct ParityCase {
    pub name: &'static str,
    pub cypher: &'static str,
    pub gql: &'static str,
    pub gremlin: &'static str,
    pub mode: DivergenceMode,
}

/// The full parity-case catalog. Add new cases here and document them in
/// `docs/DIFFERENTIAL_TESTING.md`.
pub const CASES: &[ParityCase] = &[
    // ── 1. Pattern matching ──────────────────────────────────────────────
    ParityCase {
        name: "all_person_names",
        cypher: "MATCH (p:Person) RETURN p.name AS name",
        gql: "MATCH (p:Person) RETURN p.name AS name",
        gremlin: "g.V().hasLabel('Person').values('name')",
        mode: DivergenceMode::Strict,
    },
    ParityCase {
        name: "alice_friends_via_knows",
        cypher:
            "MATCH (p:Person {name: 'Alice'})-[:KNOWS]->(f:Person) RETURN f.name AS name",
        gql:
            "MATCH (p:Person {name: 'Alice'})-[:KNOWS]->(f:Person) RETURN f.name AS name",
        gremlin: "g.V().has('Person','name','Alice').out('KNOWS').values('name')",
        mode: DivergenceMode::Strict,
    },
    // ── 2. Filtering (WHERE / has) ───────────────────────────────────────
    ParityCase {
        name: "people_over_30",
        cypher: "MATCH (p:Person) WHERE p.age > 30 RETURN p.name AS name",
        gql: "MATCH (p:Person) WHERE p.age > 30 RETURN p.name AS name",
        gremlin: "g.V().hasLabel('Person').has('age', gt(30)).values('name')",
        mode: DivergenceMode::Strict,
    },
    ParityCase {
        name: "people_active_true",
        cypher: "MATCH (p:Person) WHERE p.active = true RETURN p.name AS name",
        gql: "MATCH (p:Person) WHERE p.active = true RETURN p.name AS name",
        gremlin: "g.V().hasLabel('Person').has('active', true).values('name')",
        mode: DivergenceMode::Strict,
    },
    ParityCase {
        name: "people_age_range",
        cypher:
            "MATCH (p:Person) WHERE p.age >= 30 AND p.age < 40 RETURN p.name AS name",
        gql:
            "MATCH (p:Person) WHERE p.age >= 30 AND p.age < 40 RETURN p.name AS name",
        gremlin:
            "g.V().hasLabel('Person').has('age', gte(30)).has('age', lt(40)).values('name')",
        mode: DivergenceMode::Strict,
    },
    // ── 3. Projection ────────────────────────────────────────────────────
    // (Gremlin .project() returns a single Map column, while Cypher/GQL
    // return one column per RETURN expression; the equivalent two-column
    // shape is tracked under "documented_divergence_project_returns_map"
    // below. Single-column projection is already covered by
    // "all_person_names" above.)
    // ── 4. Aggregation ───────────────────────────────────────────────────
    ParityCase {
        name: "count_persons",
        cypher: "MATCH (p:Person) RETURN count(p) AS n",
        gql: "MATCH (p:Person) RETURN count(p) AS n",
        gremlin: "g.V().hasLabel('Person').count()",
        mode: DivergenceMode::Strict,
    },
    ParityCase {
        name: "count_companies",
        cypher: "MATCH (c:Company) RETURN count(c) AS n",
        gql: "MATCH (c:Company) RETURN count(c) AS n",
        gremlin: "g.V().hasLabel('Company').count()",
        mode: DivergenceMode::Strict,
    },
    ParityCase {
        name: "count_knows_edges",
        cypher: "MATCH ()-[r:KNOWS]->() RETURN count(r) AS n",
        gql: "MATCH ()-[r:KNOWS]->() RETURN count(r) AS n",
        gremlin: "g.E().hasLabel('KNOWS').count()",
        mode: DivergenceMode::Strict,
    },
    // ── 5. Sorting + LIMIT ───────────────────────────────────────────────
    ParityCase {
        name: "top3_oldest_people",
        cypher:
            "MATCH (p:Person) RETURN p.name AS name ORDER BY p.age DESC, p.name ASC LIMIT 3",
        gql:
            "MATCH (p:Person) RETURN p.name AS name ORDER BY p.age DESC, p.name ASC LIMIT 3",
        gremlin:
            "g.V().hasLabel('Person').order().by('age', desc).by('name', asc).limit(3).values('name')",
        mode: DivergenceMode::Strict,
    },
    // ── 6. String predicates ─────────────────────────────────────────────
    ParityCase {
        name: "person_name_starts_with_a",
        cypher:
            "MATCH (p:Person) WHERE p.name STARTS WITH 'A' RETURN p.name AS name",
        gql:
            "MATCH (p:Person) WHERE p.name STARTS WITH 'A' RETURN p.name AS name",
        gremlin:
            "g.V().hasLabel('Person').has('name', startingWith('A')).values('name')",
        mode: DivergenceMode::Strict,
    },
    // ── 7. Path / multi-hop ──────────────────────────────────────────────
    ParityCase {
        name: "alice_knows_then_lives_in",
        cypher:
            "MATCH (:Person {name:'Alice'})-[:KNOWS]->(f:Person)-[:LIVES_IN]->(c:City) RETURN c.name AS city",
        gql:
            "MATCH (:Person {name:'Alice'})-[:KNOWS]->(f:Person)-[:LIVES_IN]->(c:City) RETURN c.name AS city",
        gremlin:
            "g.V().has('Person','name','Alice').out('KNOWS').out('LIVES_IN').values('name')",
        mode: DivergenceMode::Strict,
    },
    // ── 8. Edge property filtering ───────────────────────────────────────
    ParityCase {
        name: "engineers_at_acme",
        cypher:
            "MATCH (p:Person)-[r:WORKS_AT]->(c:Company {name:'Acme'}) WHERE r.role = 'engineer' RETURN p.name AS name",
        gql:
            "MATCH (p:Person)-[r:WORKS_AT]->(c:Company {name:'Acme'}) WHERE r.role = 'engineer' RETURN p.name AS name",
        gremlin:
            "g.V().has('Company','name','Acme').inE('WORKS_AT').has('role','engineer').outV().values('name')",
        mode: DivergenceMode::Strict,
    },
    // ── 9. Relationship-type-only count ──────────────────────────────────
    ParityCase {
        name: "count_lives_in",
        cypher: "MATCH ()-[r:LIVES_IN]->() RETURN count(r) AS n",
        gql: "MATCH ()-[r:LIVES_IN]->() RETURN count(r) AS n",
        gremlin: "g.E().hasLabel('LIVES_IN').count()",
        mode: DivergenceMode::Strict,
    },
    // ─────────────────────────────────────────────────────────────────────
    // ── DOCUMENTED DIVERGENCES ───────────────────────────────────────────
    // ─────────────────────────────────────────────────────────────────────
    // The cases below have known semantic differences between engines and
    // are NOT treated as failures by the runner. See
    // docs/DIFFERENTIAL_TESTING.md for the rationale on each.
    ParityCase {
        name: "documented_divergence_optional_property_null",
        cypher:
            // openCypher: missing properties surface as Null in result rows.
            "MATCH (p:Person) RETURN p.name AS name, p.score AS score",
        gql:
            "MATCH (p:Person) RETURN p.name AS name, p.score AS score",
        gremlin:
            // Gremlin .values('score') silently drops elements without the
            // property, so the row count differs.
            "g.V().hasLabel('Person').project('name','score').by('name').by(values('score').fold())",
        mode: DivergenceMode::Documented(
            "Cypher/GQL surface missing properties as NULL; Gremlin .values() drops them. \
             We use .by(values(...).fold()) on the Gremlin side to preserve the row, but \
             the empty-list representation differs from NULL.",
        ),
    },
    ParityCase {
        name: "documented_divergence_float_avg_precision",
        cypher: "MATCH (p:Person) WHERE p.score IS NOT NULL RETURN avg(p.score) AS a",
        gql: "MATCH (p:Person) WHERE p.score IS NOT NULL RETURN avg(p.score) AS a",
        gremlin: "g.V().hasLabel('Person').values('score').mean()",
        mode: DivergenceMode::Documented(
            "Floating-point aggregation order is implementation-defined; the engines \
             may differ by ULPs. Strict equality on Float values is not portable.",
        ),
    },
    ParityCase {
        name: "documented_divergence_count_distinct_label",
        cypher: "MATCH (n) RETURN count(n) AS n",
        gql: "MATCH (n) RETURN count(n) AS n",
        gremlin: "g.V().count()",
        mode: DivergenceMode::Documented(
            "Cypher/GQL MATCH (n) returns ALL nodes regardless of label; Gremlin g.V() does \
             the same in our implementation, but the label-bookkeeping inside aggregations \
             can produce subtly different result-row column types in some cases.",
        ),
    },
    ParityCase {
        name: "documented_divergence_list_property_return",
        cypher: "MATCH (p:Person {name:'Alice'}) RETURN p.tags AS tags",
        gql: "MATCH (p:Person {name:'Alice'}) RETURN p.tags AS tags",
        gremlin: "g.V().has('Person','name','Alice').values('tags')",
        mode: DivergenceMode::Documented(
            "Gremlin .values() on a list-typed property unfolds the list into multiple \
             traversers (one per element), while Cypher/GQL return the list as a single \
             value. Use unfold()/fold() in Gremlin to bridge — but the row count differs.",
        ),
    },
    ParityCase {
        name: "documented_divergence_temporal_date_return",
        cypher: "MATCH (p:Person {name:'Alice'}) RETURN p.joined AS joined",
        gql: "MATCH (p:Person {name:'Alice'}) RETURN p.joined AS joined",
        gremlin: "g.V().has('Person','name','Alice').values('joined')",
        mode: DivergenceMode::Documented(
            "Gremlin TinkerPop has no native Date type; values() returns the raw stored \
             representation. Cypher/GQL return the typed Value::Date. Cross-engine equality \
             on temporal values is not currently supported.",
        ),
    },
    ParityCase {
        name: "documented_divergence_project_returns_map",
        cypher: "MATCH (p:Person) RETURN p.name AS name, p.age AS age",
        gql: "MATCH (p:Person) RETURN p.name AS name, p.age AS age",
        gremlin:
            "g.V().hasLabel('Person').project('name','age').by('name').by('age')",
        mode: DivergenceMode::Documented(
            "Cypher/GQL multi-column RETURN produces N positional columns; Gremlin .project() \
             produces a single column whose value is a Map. The values are equivalent \
             but the row shape differs by design — there is no Gremlin step that produces \
             N positional columns the way Cypher RETURN does.",
        ),
    },
];
