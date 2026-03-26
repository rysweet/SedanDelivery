//! LDBC SNB Interactive Complex Read Queries (IC1–IC14).
//!
//! Each test runs one IC query against the synthetic dataset built by
//! `loader::build()` and verifies that:
//!  - The query parses, plans, and executes without error.
//!  - The result has the expected columns.
//!  - The result contains the known correct values given the dataset.
//!
//! Reference: <https://github.com/ldbc/ldbc_snb_interactive_v2_impls/tree/main/cypher>

use std::collections::HashMap;
use std::time::Duration;

use gql_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::{Operator, QueryPlanner};
use gql_core::storage::InMemoryGraphStore;
use gql_core::types::Value;

use super::loader::{build, GRAPH};

// ── Test helper ───────────────────────────────────────────────────────────────

fn run(store: &InMemoryGraphStore, cypher: &str) -> Vec<HashMap<String, Value>> {
    let ast = parse_query(cypher).expect("parse error");
    let planner = QueryPlanner::new(store, GRAPH);
    let plan = planner.plan(&ast).expect("plan error");
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(30), usize::MAX);
    executor
        .execute(GRAPH, &plan, &mut budget)
        .expect("exec error")
        .rows
}

fn names(rows: &[HashMap<String, Value>], col: &str) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r.get(col) {
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        })
        .collect()
}

fn ints(rows: &[HashMap<String, Value>], col: &str) -> Vec<i64> {
    rows.iter()
        .filter_map(|r| match r.get(col) {
            Some(Value::Int(i)) => Some(*i),
            _ => None,
        })
        .collect()
}

// ── IC1: Friends with given first name (BFS, up to 2 hops) ───────────────────

#[test]
fn ic1_friends_named_frank() {
    let (store, _) = build();
    // Alice -> Charlie -> Frank (2 hops).
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
         WHERE friend.firstName = 'Frank'
         RETURN DISTINCT friend.firstName AS firstName, friend.lastName AS lastName
         ORDER BY lastName",
    );
    assert!(!rows.is_empty(), "should find Frank");
    assert_eq!(rows[0]["firstName"], Value::Str("Frank".into()));
}

#[test]
fn ic1_returns_correct_columns() {
    let (store, _) = build();
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
         WHERE friend.firstName = 'Frank'
         RETURN DISTINCT friend.firstName AS firstName, friend.lastName AS lastName",
    );
    assert!(!rows.is_empty());
    assert!(
        rows[0].contains_key("firstName"),
        "column firstName missing"
    );
    assert!(rows[0].contains_key("lastName"), "column lastName missing");
}

// ── IC2: Recent messages by friends (traversal + temporal sort) ───────────────

#[test]
fn ic2_recent_posts_by_friends() {
    let (store, _) = build();
    // Alice knows Bob and Charlie; Bob has post2 (date 20220201), post6 (20220601);
    // Charlie has post3 (20220301).
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS]->(friend:Person)
              <-[:HAS_CREATOR]-(post:Post)
         RETURN friend.firstName AS friendFirst,
                post.content AS content,
                post.creationDate AS date
         ORDER BY date DESC
         LIMIT 10",
    );
    assert!(
        rows.len() >= 3,
        "expected ≥3 posts by Bob and Charlie, got {}",
        rows.len()
    );
    // Most recent post (Bob's post6) should come first.
    assert_eq!(rows[0]["date"], Value::Int(20_220_601));
}

// ── IC3: Friends in a specific country (multi-hop + geographic filter) ─────────

#[test]
fn ic3_friends_in_germany() {
    let (store, _) = build();
    // Alice knows Bob (Berlin, Germany) directly,
    // and Charlie -> Frank (Berlin, Germany) at 2 hops.
    // Diana (Munich, Germany) is 2 hops via Bob.
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
              -[:IS_LOCATED_IN]->(city:Place)-[:IS_PART_OF]->(country:Place)
         WHERE country.name = 'Germany' AND friend.firstName <> 'Alice'
         RETURN DISTINCT friend.firstName AS firstName, country.name AS country
         ORDER BY firstName",
    );
    assert!(!rows.is_empty(), "should find friends in Germany");
    let first_names = names(&rows, "firstName");
    assert!(
        first_names.contains(&"Bob".to_string()),
        "Bob should be in Germany; got {first_names:?}"
    );
}

// ── IC4: Popular topics among friends (aggregation) ────────────────────────────

#[test]
fn ic4_popular_tags_among_friends() {
    let (store, _) = build();
    // Alice's friends: Bob (post2→sports, post6→music), Charlie (post3→technology).
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS]->(friend:Person)
              <-[:HAS_CREATOR]-(post:Post)-[:HAS_TAG]->(tag:Tag)
         RETURN tag.name AS tagName, count(DISTINCT post) AS postCount
         ORDER BY postCount DESC, tagName ASC",
    );
    assert!(!rows.is_empty(), "should find tags from friends' posts");
    let tag_names = names(&rows, "tagName");
    assert!(
        tag_names.contains(&"sports".to_string())
            || tag_names.contains(&"music".to_string())
            || tag_names.contains(&"technology".to_string()),
        "expected tags from friends' posts; got {tag_names:?}"
    );
}

// ── IC5: New groups (forums) from friends (multi-hop + forum traversal) ─────────

#[test]
fn ic5_forums_joined_by_friends() {
    let (store, _) = build();
    // SportsForum contains post2 (by Bob); Bob is Alice's friend.
    // TechForum contains post3 (by Charlie); Charlie is Alice's friend.
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
              <-[:HAS_CREATOR]-(post:Post)<-[:CONTAINER_OF]-(forum:Forum)
         RETURN DISTINCT forum.title AS forumTitle, count(DISTINCT friend) AS memberCount
         ORDER BY memberCount DESC, forumTitle",
    );
    assert!(
        !rows.is_empty(),
        "should find forums containing friends' posts"
    );
    let titles = names(&rows, "forumTitle");
    assert!(
        titles.contains(&"SportsForum".to_string()) || titles.contains(&"TechForum".to_string()),
        "expected SportsForum or TechForum; got {titles:?}"
    );
}

// ── IC6: Tag co-occurrence (friends-of-friends) ───────────────────────────────

#[test]
fn ic6_tag_cooccurrence_excluding_technology() {
    let (store, _) = build();
    // Alice's friends: Bob (sports, music tags), Charlie (technology tag).
    // Tags other than 'technology': sports, music.
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
              <-[:HAS_CREATOR]-(post:Post)-[:HAS_TAG]->(tag:Tag)
         WHERE tag.name <> 'technology'
         RETURN tag.name AS tagName, count(DISTINCT post) AS postCount
         ORDER BY postCount DESC, tagName ASC
         LIMIT 10",
    );
    assert!(!rows.is_empty(), "should find co-occurring tags");
    let tag_names = names(&rows, "tagName");
    assert!(
        tag_names.iter().all(|t| t != "technology"),
        "no technology tag should appear; got {tag_names:?}"
    );
}

// ── IC7: Recent likers (1-hop traversal) ──────────────────────────────────────

#[test]
fn ic7_likers_of_alice_posts() {
    let (store, _) = build();
    // Bob and Charlie liked post1 (by Alice).
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})<-[:HAS_CREATOR]-(post:Post)
              <-[:LIKES]-(liker:Person)
         RETURN DISTINCT liker.firstName AS firstName, liker.lastName AS lastName
         ORDER BY firstName",
    );
    assert!(
        rows.len() >= 2,
        "expected ≥2 likers of Alice's post; got {}",
        rows.len()
    );
    let first_names = names(&rows, "firstName");
    assert!(
        first_names.contains(&"Bob".to_string()),
        "Bob should have liked Alice's post; got {first_names:?}"
    );
    assert!(
        first_names.contains(&"Charlie".to_string()),
        "Charlie should have liked Alice's post; got {first_names:?}"
    );
}

// ── IC8: Recent replies to Alice's posts ─────────────────────────────────────

#[test]
fn ic8_replies_to_alice_posts() {
    let (store, _) = build();
    // comment1 (by Bob) and comment2 (by Charlie) are REPLY_OF post1 (by Alice).
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})<-[:HAS_CREATOR]-(post:Post)
              <-[:REPLY_OF]-(comment:Comment)-[:HAS_CREATOR]->(commenter:Person)
         RETURN commenter.firstName AS firstName, comment.content AS content
         ORDER BY firstName",
    );
    assert_eq!(rows.len(), 2, "expected 2 replies to Alice's posts");
    let commenters = names(&rows, "firstName");
    assert!(
        commenters.contains(&"Bob".to_string()),
        "Bob should be a commenter"
    );
    assert!(
        commenters.contains(&"Charlie".to_string()),
        "Charlie should be a commenter"
    );
}

// ── IC9: Recent messages by friends-of-friends (2-hop) ────────────────────────

#[test]
fn ic9_posts_by_friends_of_friends() {
    let (store, _) = build();
    // 2-hop from Alice: Diana (post-less here but via Bob), Eve (post4), Frank (post5).
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
              <-[:HAS_CREATOR]-(post:Post)
         WHERE friend.firstName <> 'Alice'
         RETURN DISTINCT friend.firstName AS friendFirst,
                post.content AS content,
                post.creationDate AS date
         ORDER BY date DESC
         LIMIT 10",
    );
    assert!(!rows.is_empty(), "should find posts by friends-of-friends");
}

// ── IC10: Friend recommendation (2-hop scoring) ────────────────────────────────

#[test]
fn ic10_friend_recommendation_for_alice() {
    let (store, _) = build();
    // 2-hop friends Alice doesn't directly know: Diana, Eve (via Bob), Frank (via Charlie).
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS]->(friend1:Person)
              -[:KNOWS]->(rec:Person)
         WHERE rec.firstName <> 'Alice'
           AND NOT (root)-[:KNOWS]->(rec)
         RETURN DISTINCT rec.firstName AS firstName,
                rec.lastName AS lastName,
                count(DISTINCT friend1) AS commonCount
         ORDER BY commonCount DESC
         LIMIT 10",
    );
    assert!(!rows.is_empty(), "should find friend recommendations");
    let rec_names = names(&rows, "firstName");
    assert!(
        rec_names.contains(&"Diana".to_string())
            || rec_names.contains(&"Eve".to_string())
            || rec_names.contains(&"Frank".to_string()),
        "expected Diana/Eve/Frank as recommendations; got {rec_names:?}"
    );
}

// ── IC11: Friend works at company in country ───────────────────────────────────

#[test]
fn ic11_friends_working_in_germany() {
    let (store, _) = build();
    // Alice and Bob both work at TechCorp (in Germany).
    // Alice's 1-hop friend Bob → WORK_AT TechCorp → IS_LOCATED_IN Germany.
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
              -[:WORK_AT]->(org:Organisation)-[:IS_LOCATED_IN]->(country:Place)
         WHERE country.name = 'Germany' AND friend.firstName <> 'Alice'
         RETURN DISTINCT friend.firstName AS firstName, org.name AS orgName
         ORDER BY firstName",
    );
    assert!(!rows.is_empty(), "should find friends working in Germany");
    let first_names = names(&rows, "firstName");
    assert!(
        first_names.contains(&"Bob".to_string()),
        "Bob works at TechCorp in Germany; got {first_names:?}"
    );
}

// ── IC12: Expert friends by tag class (hierarchical) ──────────────────────────

#[test]
fn ic12_friends_with_tech_tagged_comments() {
    let (store, _) = build();
    // comment2 is by Charlie (Alice's friend) and has HAS_TAG → technology → HAS_TYPE TopicClass.
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS]->(friend:Person)
              <-[:HAS_CREATOR]-(comment:Comment)
              -[:HAS_TAG]->(tag:Tag)-[:HAS_TYPE]->(tc:TagClass)
         WHERE tc.name = 'TopicClass'
         RETURN DISTINCT friend.firstName AS firstName,
                collect(DISTINCT tag.name) AS tagNames
         ORDER BY firstName",
    );
    assert!(
        !rows.is_empty(),
        "should find friends with TopicClass-tagged comments"
    );
    let first_names = names(&rows, "firstName");
    assert!(
        first_names.contains(&"Charlie".to_string()),
        "Charlie should have technology (TopicClass) comment; got {first_names:?}"
    );
}

// ── IC13: Shortest path between persons ──────────────────────────────────────
//
// NOTE: The Cypher text syntax `MATCH p = shortestPath(...)` is not yet
// supported by the parser.  IC13 tests exercise the ShortestPath *executor*
// operator directly via the operator tree API, which is fully implemented.
//
// The operator tree uses NodeScan + Filter to select specific src/dst nodes:
//   ProduceResults {
//     Apply {
//       outer: Filter(src.firstName = 'Alice', NodeScan(src:Person)),
//       inner: Apply {
//         outer: NodeScan(dst:Person),   ← NodeScan merges outer context into row
//         inner: Filter(dst.firstName = 'Frank', ShortestPath)
//       }
//     }
//   }
//
// NodeScan (unlike NodeByIdSeek) propagates the outer row into each result row,
// so both 'src' and 'dst' are present when ShortestPath executes.

fn first_name_eq(alias: &str, first_name: &str) -> Expr {
    Expr::BinaryOp {
        op: BinaryOp::Eq,
        left: Box::new(Expr::Property(
            Box::new(Expr::Variable(Identifier::new(alias))),
            "firstName".into(),
        )),
        right: Box::new(Expr::Str(first_name.into())),
    }
}

fn person_named(alias: &str, first_name: &str) -> Operator {
    Operator::Filter {
        predicate: Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(Identifier::new(alias))),
                "firstName".into(),
            )),
            right: Box::new(Expr::Str(first_name.into())),
        },
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new(alias),
        }),
    }
}

#[test]
fn ic13_shortest_path_alice_to_frank() {
    let (store, ids) = build();
    // Alice --KNOWS--> Charlie --KNOWS--> Frank: shortest path has 2 edges.

    let plan = Operator::ProduceResults {
        columns: vec![("p".to_string(), Expr::Variable(Identifier::new("p")))],
        child: Box::new(Operator::Apply {
            outer: Box::new(person_named("src", "Alice")),
            inner: Box::new(Operator::Filter {
                predicate: first_name_eq("dst", "Frank"),
                child: Box::new(Operator::Apply {
                    outer: Box::new(Operator::NodeScan {
                        label: Some(Label::new("Person")),
                        alias: Identifier::new("dst"),
                    }),
                    inner: Box::new(Operator::ShortestPath {
                        src_alias: Identifier::new("src"),
                        dst_alias: Identifier::new("dst"),
                        rel_types: vec![RelType::new("KNOWS")],
                        direction: Direction::Undirected,
                        min: 1,
                        max: 30,
                        path_alias: Identifier::new("p"),
                        all_paths: false,
                    }),
                }),
            }),
        }),
    };

    let executor = QueryExecutor::new(&store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(30), usize::MAX);
    let result = executor.execute(GRAPH, &plan, &mut budget).unwrap();

    // Expect exactly one row (one shortest path Alice→Charlie→Frank).
    let path_rows: Vec<_> = result
        .rows
        .iter()
        .filter(|r| matches!(r.get("p"), Some(Value::Path { .. })))
        .collect();
    assert!(
        !path_rows.is_empty(),
        "should find at least one shortest path"
    );

    match path_rows[0].get("p").unwrap() {
        Value::Path { nodes, edges } => {
            assert_eq!(
                edges.len(),
                2,
                "Alice→Charlie→Frank: 2 edges; got {}",
                edges.len()
            );
            assert_eq!(nodes.len(), 3, "path should have 3 nodes");
            assert_eq!(nodes[0], ids.alice, "path starts at Alice");
            assert_eq!(nodes[2], ids.frank, "path ends at Frank");
        }
        other => panic!("expected Path value, got {:?}", other),
    }
}

#[test]
fn ic13_frank_reachable_from_alice_in_two_hops() {
    let (store, _) = build();
    // Verify via variable-length path query (avoids shortestPath() parser limitation).
    let rows = run(
        &store,
        "MATCH (a:Person {firstName:'Alice'})-[:KNOWS*2..2]-(b:Person {firstName:'Frank'})
         RETURN count(*) AS c",
    );
    assert_eq!(rows.len(), 1);
    match rows[0]["c"] {
        Value::Int(n) => assert!(n >= 1, "Frank must be reachable in exactly 2 hops; got {n}"),
        _ => panic!("expected Int"),
    }
}

// ── IC14: All shortest paths (variable-length + scoring) ──────────────────────

#[test]
fn ic14_all_shortest_paths_alice_to_frank() {
    let (store, ids) = build();
    // There is exactly one shortest path: Alice→Charlie→Frank (2 hops).

    let plan = Operator::ProduceResults {
        columns: vec![("p".to_string(), Expr::Variable(Identifier::new("p")))],
        child: Box::new(Operator::Apply {
            outer: Box::new(person_named("src", "Alice")),
            inner: Box::new(Operator::Filter {
                predicate: first_name_eq("dst", "Frank"),
                child: Box::new(Operator::Apply {
                    outer: Box::new(Operator::NodeScan {
                        label: Some(Label::new("Person")),
                        alias: Identifier::new("dst"),
                    }),
                    inner: Box::new(Operator::ShortestPath {
                        src_alias: Identifier::new("src"),
                        dst_alias: Identifier::new("dst"),
                        rel_types: vec![RelType::new("KNOWS")],
                        direction: Direction::Undirected,
                        min: 1,
                        max: 30,
                        path_alias: Identifier::new("p"),
                        all_paths: true,
                    }),
                }),
            }),
        }),
    };

    let executor = QueryExecutor::new(&store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(30), usize::MAX);
    let result = executor.execute(GRAPH, &plan, &mut budget).unwrap();

    let path_rows: Vec<_> = result
        .rows
        .iter()
        .filter(|r| matches!(r.get("p"), Some(Value::Path { .. })))
        .collect();
    assert!(
        !path_rows.is_empty(),
        "allShortestPaths must find at least one path"
    );

    match path_rows[0].get("p").unwrap() {
        Value::Path { nodes, edges } => {
            assert_eq!(edges.len(), 2, "shortest Alice→Frank path has 2 edges");
            assert_eq!(nodes.len(), 3, "path through 3 nodes");
            assert_eq!(nodes[0], ids.alice, "path starts at Alice");
            assert_eq!(nodes[2], ids.frank, "path ends at Frank");
        }
        other => panic!("expected Path value, got {:?}", other),
    }
}

// ── Result count and column sanity checks ─────────────────────────────────────

#[test]
fn all_ic_queries_return_bounded_results() {
    let (store, _) = build();
    // Run all IC queries and verify they return a finite number of rows
    // (none exceed the size of our synthetic dataset).
    let queries: &[&str] = &[
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person) WHERE friend.firstName = 'Frank' RETURN DISTINCT friend.firstName LIMIT 20",
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS]->(friend:Person)<-[:HAS_CREATOR]-(post:Post) RETURN friend.firstName, post.content LIMIT 20",
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)-[:IS_LOCATED_IN]->(city:Place)-[:IS_PART_OF]->(country:Place) WHERE country.name = 'Germany' RETURN DISTINCT friend.firstName LIMIT 20",
    ];
    for q in queries {
        let rows = run(&store, q);
        assert!(
            rows.len() <= 20,
            "query returned unexpectedly many rows: {}",
            rows.len()
        );
    }
}

// ── Negative case: LIMIT 0 returns empty ──────────────────────────────────────

#[test]
fn ic_with_limit_zero_returns_empty() {
    let (store, _) = build();
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
         RETURN DISTINCT friend.firstName
         LIMIT 0",
    );
    assert_eq!(rows.len(), 0, "LIMIT 0 must return no rows");
}

// ── IC stat: count total friends at 2 hops ────────────────────────────────────

#[test]
fn ic_count_two_hop_friends_of_alice() {
    let (store, _) = build();
    let rows = run(
        &store,
        "MATCH (root:Person {firstName:'Alice'})-[:KNOWS*1..2]-(friend:Person)
         WHERE friend.firstName <> 'Alice'
         RETURN count(DISTINCT friend) AS cnt",
    );
    assert_eq!(rows.len(), 1);
    let cnt = match rows[0]["cnt"] {
        Value::Int(n) => n,
        _ => panic!("expected Int for count"),
    };
    // Direct friends: Bob, Charlie (2).
    // 2-hop friends: Diana, Eve (via Bob), Frank (via Charlie), Grace (via Bob→Diana), Henry (via Bob→Eve).
    assert!(
        cnt >= 5,
        "Alice should have at least 5 friends within 2 hops; got {cnt}"
    );
}

// ── Temporal ORDER BY ─────────────────────────────────────────────────────────

#[test]
fn ic_posts_ordered_by_creation_date_desc() {
    let (store, _) = build();
    let rows = run(
        &store,
        "MATCH (p:Post) RETURN p.creationDate AS d ORDER BY d DESC LIMIT 6",
    );
    let dates = ints(&rows, "d");
    for i in 0..dates.len().saturating_sub(1) {
        assert!(
            dates[i] >= dates[i + 1],
            "dates should be descending: {dates:?}"
        );
    }
}
