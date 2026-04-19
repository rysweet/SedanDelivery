//! Curated graph fixture shared by every parity case.
//!
//! The fixture mixes labels, relationship types, and property datatypes
//! (strings, ints, floats, bools, lists, maps, dates, datetimes, nulls)
//! to exercise each engine's value-handling path.
//!
//! Schema summary:
//!
//! | Label    | Count | Notable properties                                    |
//! |----------|-------|-------------------------------------------------------|
//! | Person   | 20    | name, age, score, active, tags (list), joined (date)  |
//! | City     | 5     | name, country, population                             |
//! | Company  | 5     | name, founded (int), public (bool)                    |
//! | Project  | 10    | name, budget (float), tags (list)                     |
//! | Tag      | 10    | name (rare props omitted to test optional handling)   |
//!
//! Edges: KNOWS, LIVES_IN, WORKS_AT, OWNS, TAGGED, COLLAB
//!
//! Total: 50 nodes, ~100 edges (subject to small fluctuations as the
//! fixture evolves; the exact counts are asserted in the `fixture_shape`
//! test below so it stays stable).

use indexmap::IndexMap;
use jiff::civil;
use sedan_cypher_embedded::{GraphStore, InMemoryGraphStore, Value};

/// Graph name shared by all engines for the parity suite.
pub const GRAPH: &str = "diff";

/// Build the fixture graph and return the in-memory store.
///
/// The same store implementation (`InMemoryGraphStore`) is used by all three
/// engines, so the only variable across runs is the engine itself — exactly
/// what we want for differential testing.
pub fn build_fixture() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph(GRAPH).expect("create graph");

    let mut city_id = IndexMap::new();
    for (name, country, pop) in [
        ("Seattle", "USA", 750_000_i64),
        ("Portland", "USA", 650_000),
        ("Vancouver", "Canada", 675_000),
        ("Berlin", "Germany", 3_700_000),
        ("Tokyo", "Japan", 13_900_000),
    ] {
        let id = store
            .create_node(
                GRAPH,
                vec!["City".into()],
                IndexMap::from([
                    ("name".into(), Value::Str(name.into())),
                    ("country".into(), Value::Str(country.into())),
                    ("population".into(), Value::Int(pop)),
                ]),
            )
            .expect("create city");
        city_id.insert(name, id);
    }

    let mut company_id = IndexMap::new();
    for (name, founded, public) in [
        ("Acme", 1985_i64, true),
        ("Globex", 1999, false),
        ("Initech", 2005, false),
        ("Hooli", 2010, true),
        ("Pied Piper", 2014, false),
    ] {
        let id = store
            .create_node(
                GRAPH,
                vec!["Company".into()],
                IndexMap::from([
                    ("name".into(), Value::Str(name.into())),
                    ("founded".into(), Value::Int(founded)),
                    ("public".into(), Value::Bool(public)),
                ]),
            )
            .expect("create company");
        company_id.insert(name, id);
    }

    let mut tag_id = IndexMap::new();
    for name in [
        "rust", "graph", "db", "ai", "ml", "cloud", "ops", "web", "mobile", "data",
    ] {
        let id = store
            .create_node(
                GRAPH,
                vec!["Tag".into()],
                IndexMap::from([("name".into(), Value::Str(name.into()))]),
            )
            .expect("create tag");
        tag_id.insert(name, id);
    }

    // 20 people. The `score` and `joined` properties are intentionally
    // omitted on a few rows so optional-property handling can be tested.
    let people = [
        (
            "Alice",
            30_i64,
            Some(91.5_f64),
            "Seattle",
            true,
            civil::date(2018, 3, 14),
        ),
        (
            "Bob",
            25,
            Some(72.0),
            "Seattle",
            true,
            civil::date(2020, 6, 1),
        ),
        (
            "Carol",
            40,
            Some(88.25),
            "Portland",
            false,
            civil::date(2015, 1, 20),
        ),
        (
            "Dave",
            35,
            Some(64.5),
            "Portland",
            true,
            civil::date(2017, 11, 5),
        ),
        (
            "Eve",
            28,
            Some(95.0),
            "Vancouver",
            true,
            civil::date(2019, 8, 30),
        ),
        (
            "Frank",
            45,
            Some(58.75),
            "Seattle",
            false,
            civil::date(2012, 2, 14),
        ),
        (
            "Grace",
            33,
            Some(79.5),
            "Portland",
            true,
            civil::date(2016, 4, 10),
        ),
        (
            "Heidi",
            27,
            Some(82.0),
            "Vancouver",
            true,
            civil::date(2021, 9, 2),
        ),
        (
            "Ivan",
            50,
            Some(67.0),
            "Seattle",
            false,
            civil::date(2010, 12, 1),
        ),
        (
            "Judy",
            22,
            Some(99.0),
            "Vancouver",
            true,
            civil::date(2022, 5, 17),
        ),
        (
            "Karl",
            38,
            Some(70.0),
            "Berlin",
            true,
            civil::date(2014, 7, 22),
        ),
        ("Liam", 31, None, "Berlin", true, civil::date(2019, 3, 3)),
        (
            "Mia",
            29,
            Some(85.5),
            "Tokyo",
            true,
            civil::date(2020, 1, 15),
        ),
        (
            "Niko",
            42,
            Some(60.0),
            "Tokyo",
            false,
            civil::date(2013, 10, 10),
        ),
        (
            "Olga",
            36,
            Some(77.5),
            "Berlin",
            true,
            civil::date(2017, 6, 18),
        ),
        (
            "Pat",
            24,
            Some(89.0),
            "Seattle",
            true,
            civil::date(2023, 2, 1),
        ),
        (
            "Quinn",
            47,
            None,
            "Portland",
            false,
            civil::date(2011, 9, 9),
        ),
        (
            "Ruth",
            32,
            Some(74.25),
            "Vancouver",
            true,
            civil::date(2018, 12, 12),
        ),
        (
            "Sam",
            26,
            Some(81.0),
            "Tokyo",
            true,
            civil::date(2021, 4, 4),
        ),
        (
            "Tina",
            39,
            Some(66.0),
            "Berlin",
            true,
            civil::date(2016, 8, 8),
        ),
    ];
    let mut person_id = IndexMap::new();
    for (name, age, score, city, active, joined) in people {
        let mut props: IndexMap<String, Value> = IndexMap::from([
            ("name".into(), Value::Str(name.into())),
            ("age".into(), Value::Int(age)),
            ("active".into(), Value::Bool(active)),
            ("joined".into(), Value::Date(joined)),
            (
                "tags".into(),
                Value::List(vec![
                    Value::Str("person".into()),
                    Value::Str(name.to_lowercase().into()),
                ]),
            ),
        ]);
        if let Some(s) = score {
            props.insert("score".into(), Value::Float(s));
        }
        let id = store
            .create_node(GRAPH, vec!["Person".into()], props)
            .expect("create person");
        person_id.insert(name, id);

        store
            .create_edge(GRAPH, id, city_id[city], "LIVES_IN".into(), IndexMap::new())
            .expect("LIVES_IN");
    }

    // 10 projects with budget (float) and tag list.
    let projects = [
        ("Apollo", 250_000.0_f64, vec!["rust", "db"]),
        ("Borealis", 120_000.5, vec!["cloud", "ops"]),
        ("Cygnus", 80_000.0, vec!["web"]),
        ("Draco", 500_000.75, vec!["ai", "ml"]),
        ("Eridanus", 60_000.0, vec!["mobile"]),
        ("Fornax", 175_000.25, vec!["graph", "db"]),
        ("Gemini", 95_000.0, vec!["data"]),
        ("Hydra", 310_000.5, vec!["rust", "graph"]),
        ("Indus", 42_000.0, vec!["web", "mobile"]),
        ("Janus", 220_000.0, vec!["cloud", "ai"]),
    ];
    let mut project_id = IndexMap::new();
    for (name, budget, tags) in &projects {
        let id = store
            .create_node(
                GRAPH,
                vec!["Project".into()],
                IndexMap::from([
                    ("name".into(), Value::Str((*name).into())),
                    ("budget".into(), Value::Float(*budget)),
                    (
                        "tags".into(),
                        Value::List(tags.iter().map(|t| Value::Str((*t).into())).collect()),
                    ),
                ]),
            )
            .expect("create project");
        project_id.insert(*name, id);

        for tag in tags {
            store
                .create_edge(GRAPH, id, tag_id[*tag], "TAGGED".into(), IndexMap::new())
                .expect("TAGGED");
        }
    }

    // Friendships (KNOWS), with a `since` int property so KNOWS edges have
    // non-trivial property reads available.
    let friendships = [
        ("Alice", "Bob", 2018),
        ("Alice", "Carol", 2017),
        ("Alice", "Dave", 2019),
        ("Bob", "Eve", 2020),
        ("Bob", "Frank", 2019),
        ("Carol", "Grace", 2016),
        ("Dave", "Heidi", 2018),
        ("Eve", "Ivan", 2021),
        ("Frank", "Judy", 2022),
        ("Grace", "Alice", 2017),
        ("Heidi", "Bob", 2021),
        ("Ivan", "Carol", 2014),
        ("Karl", "Liam", 2019),
        ("Karl", "Olga", 2017),
        ("Liam", "Tina", 2020),
        ("Mia", "Niko", 2018),
        ("Mia", "Sam", 2020),
        ("Niko", "Sam", 2021),
        ("Olga", "Tina", 2016),
        ("Pat", "Alice", 2023),
        ("Pat", "Bob", 2023),
        ("Quinn", "Carol", 2012),
        ("Ruth", "Eve", 2020),
        ("Sam", "Mia", 2020),
        ("Tina", "Karl", 2017),
    ];
    for (from, to, since) in friendships {
        store
            .create_edge(
                GRAPH,
                person_id[from],
                person_id[to],
                "KNOWS".into(),
                IndexMap::from([("since".into(), Value::Int(since))]),
            )
            .expect("KNOWS");
    }

    // Employment edges WORKS_AT (Person -> Company), with role + salary.
    let employment = [
        ("Alice", "Acme", "engineer", 130_000_i64),
        ("Bob", "Acme", "engineer", 110_000),
        ("Carol", "Globex", "manager", 150_000),
        ("Dave", "Globex", "engineer", 125_000),
        ("Eve", "Initech", "designer", 100_000),
        ("Frank", "Initech", "engineer", 115_000),
        ("Grace", "Hooli", "manager", 165_000),
        ("Heidi", "Hooli", "engineer", 120_000),
        ("Ivan", "Pied Piper", "engineer", 135_000),
        ("Judy", "Pied Piper", "designer", 95_000),
        ("Karl", "Acme", "engineer", 128_000),
        ("Liam", "Globex", "engineer", 112_000),
        ("Mia", "Hooli", "designer", 105_000),
        ("Niko", "Initech", "manager", 145_000),
        ("Olga", "Pied Piper", "engineer", 122_000),
    ];
    for (person, company, role, salary) in employment {
        store
            .create_edge(
                GRAPH,
                person_id[person],
                company_id[company],
                "WORKS_AT".into(),
                IndexMap::from([
                    ("role".into(), Value::Str(role.into())),
                    ("salary".into(), Value::Int(salary)),
                ]),
            )
            .expect("WORKS_AT");
    }

    // Ownership: companies own projects (Company -> OWNS -> Project).
    let ownership = [
        ("Acme", "Apollo"),
        ("Acme", "Borealis"),
        ("Globex", "Cygnus"),
        ("Globex", "Draco"),
        ("Initech", "Eridanus"),
        ("Initech", "Fornax"),
        ("Hooli", "Gemini"),
        ("Hooli", "Hydra"),
        ("Pied Piper", "Indus"),
        ("Pied Piper", "Janus"),
    ];
    for (company, project) in ownership {
        store
            .create_edge(
                GRAPH,
                company_id[company],
                project_id[project],
                "OWNS".into(),
                IndexMap::new(),
            )
            .expect("OWNS");
    }

    // Collaboration: a few Person -> COLLAB -> Project edges.
    let collaboration = [
        ("Alice", "Apollo"),
        ("Bob", "Apollo"),
        ("Karl", "Apollo"),
        ("Carol", "Cygnus"),
        ("Eve", "Eridanus"),
        ("Grace", "Gemini"),
        ("Mia", "Hydra"),
        ("Niko", "Janus"),
    ];
    for (person, project) in collaboration {
        store
            .create_edge(
                GRAPH,
                person_id[person],
                project_id[project],
                "COLLAB".into(),
                IndexMap::new(),
            )
            .expect("COLLAB");
    }

    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use sedan_cypher_embedded::CypherEngine;

    /// Locks the fixture shape so casual edits to `build_fixture` are caught.
    /// Update these counts deliberately when you intentionally change the graph.
    #[test]
    fn fixture_shape() {
        let store = build_fixture();
        let engine = CypherEngine::new();

        let n = engine
            .execute(&store, GRAPH, "MATCH (n) RETURN count(n) AS c")
            .unwrap();
        assert_eq!(n.rows[0].get("c"), Some(&Value::Int(50)));

        let e = engine
            .execute(&store, GRAPH, "MATCH ()-[r]->() RETURN count(r) AS c")
            .unwrap();
        // 20 LIVES_IN + 25 KNOWS + 15 WORKS_AT + 10 OWNS + 8 COLLAB
        // + tag edges (Apollo:2 + Borealis:2 + Cygnus:1 + Draco:2 + Eridanus:1
        //   + Fornax:2 + Gemini:1 + Hydra:2 + Indus:2 + Janus:2) = 17
        // total = 95
        assert_eq!(e.rows[0].get("c"), Some(&Value::Int(95)));
    }
}
