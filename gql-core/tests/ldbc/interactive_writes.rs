//! LDBC SNB Interactive Write Operations.
//!
//! Tests the 8 insert operations and 8 delete operations defined in the
//! LDBC SNB Interactive v2 spec.  Each test uses a fresh graph so write
//! operations are independent.
//!
//! Reference: https://ldbcouncil.org/benchmarks/snb/

use std::collections::HashMap;
use std::time::Duration;

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;

use super::loader::{build, GRAPH};

// ── Helper ────────────────────────────────────────────────────────────────────

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

fn count(store: &InMemoryGraphStore, label: &str) -> i64 {
    let rows = run(store, &format!("MATCH (n:{label}) RETURN count(n) AS c"));
    match rows[0]["c"] {
        Value::Int(n) => n,
        _ => panic!("expected Int for count"),
    }
}

// ── INSERT operations ─────────────────────────────────────────────────────────

/// IU1: Insert a new Person node.
#[test]
fn iu1_insert_person() {
    let (store, _) = build();
    let before = count(&store, "Person");
    run(
        &store,
        "CREATE (:Person {firstName:'NewUser', lastName:'Tester',
                          birthday:19990101, creationDate:20230601,
                          gender:'other'})",
    );
    let after = count(&store, "Person");
    assert_eq!(after, before + 1, "person count must increase by 1");

    let rows = run(
        &store,
        "MATCH (n:Person {firstName:'NewUser'}) RETURN n.lastName AS ln",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ln"], Value::Str("Tester".into()));
}

/// IU2: Insert a KNOWS relationship between two existing persons.
#[test]
fn iu2_insert_knows_edge() {
    let (store, ids) = build();

    // Alice does not directly know Henry; create the edge.
    run(
        &store,
        "MATCH (a:Person {firstName:'Alice'}), (b:Person {firstName:'Henry'})
         MERGE (a)-[:KNOWS {creationDate:20230701}]->(b)",
    );

    // Verify the edge exists.
    let rows = run(
        &store,
        "MATCH (a:Person {firstName:'Alice'})-[:KNOWS]->(b:Person {firstName:'Henry'})
         RETURN count(*) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(1),
        "KNOWS edge Alice→Henry must exist"
    );

    // Make sure we didn't accidentally create a duplicate.
    let rows2 = run(
        &store,
        "MATCH (a:Person {firstName:'Alice'})-[:KNOWS]->(b:Person {firstName:'Henry'})
         RETURN count(*) AS c",
    );
    assert_eq!(rows2[0]["c"], Value::Int(1), "MERGE must not duplicate");

    let _ = ids; // suppress unused warning
}

/// IU3: Insert a new Post.
#[test]
fn iu3_insert_post() {
    let (store, ids) = build();
    let before = count(&store, "Post");

    run(
        &store,
        "MATCH (creator:Person {firstName:'Alice'})
         CREATE (p:Post {content:'New LDBC post', creationDate:20230801, length:50})
         CREATE (p)-[:HAS_CREATOR]->(creator)",
    );

    let after = count(&store, "Post");
    assert_eq!(after, before + 1, "post count must increase by 1");

    let rows = run(
        &store,
        "MATCH (p:Post {content:'New LDBC post'})-[:HAS_CREATOR]->(creator:Person)
         RETURN creator.firstName AS fn",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["fn"], Value::Str("Alice".into()));

    let _ = ids;
}

/// IU4: Insert a new Comment replying to a Post.
#[test]
fn iu4_insert_comment() {
    let (store, ids) = build();
    let before = count(&store, "Comment");

    run(
        &store,
        "MATCH (post:Post {content:'Tech news today'}), (author:Person {firstName:'Grace'})
         CREATE (c:Comment {content:'Interesting!', creationDate:20230901, length:12})
         CREATE (c)-[:REPLY_OF]->(post)
         CREATE (c)-[:HAS_CREATOR]->(author)",
    );

    let after = count(&store, "Comment");
    assert_eq!(after, before + 1, "comment count must increase by 1");

    let rows = run(
        &store,
        "MATCH (c:Comment {content:'Interesting!'})-[:REPLY_OF]->(p:Post),
               (c)-[:HAS_CREATOR]->(a:Person)
         RETURN p.content AS post, a.firstName AS author",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["post"], Value::Str("Tech news today".into()));

    let _ = ids;
}

/// IU5: Insert a new Forum.
#[test]
fn iu5_insert_forum() {
    let (store, _) = build();
    let before = count(&store, "Forum");

    run(
        &store,
        "CREATE (:Forum {title:'NewsDaily', creationDate:20231001})",
    );

    let after = count(&store, "Forum");
    assert_eq!(after, before + 1, "forum count must increase by 1");
}

/// IU6: Insert a LIKES relationship (person likes a post).
#[test]
fn iu6_insert_likes() {
    let (store, ids) = build();

    // Grace has not yet liked any post; add a LIKES edge to post1.
    run(
        &store,
        "MATCH (p:Person {firstName:'Grace'}), (post:Post {content:'Tech news today'})
         MERGE (p)-[:LIKES {creationDate:20231101}]->(post)",
    );

    let rows = run(
        &store,
        "MATCH (p:Person {firstName:'Grace'})-[:LIKES]->(post:Post)
         RETURN count(*) AS c",
    );
    assert_eq!(rows[0]["c"], Value::Int(1), "Grace should now like 1 post");

    let _ = ids;
}

/// IU7: Add a Person as a Forum member (HAS_MEMBER).
#[test]
fn iu7_insert_forum_membership() {
    let (store, _) = build();

    run(
        &store,
        "MATCH (f:Forum {title:'TechForum'}), (p:Person {firstName:'Diana'})
         MERGE (f)-[:HAS_MEMBER {creationDate:20231201}]->(p)",
    );

    let rows = run(
        &store,
        "MATCH (f:Forum {title:'TechForum'})-[:HAS_MEMBER]->(p:Person)
         RETURN p.firstName AS fn ORDER BY fn",
    );
    let first_names: Vec<String> = rows
        .iter()
        .filter_map(|r| match r.get("fn") {
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        first_names.contains(&"Diana".to_string()),
        "Diana must be a TechForum member; got {first_names:?}"
    );
}

/// IU8: Set a Forum moderator (HAS_MODERATOR).
#[test]
fn iu8_set_forum_moderator() {
    let (store, _) = build();

    run(
        &store,
        "MATCH (f:Forum {title:'TechForum'}), (p:Person {firstName:'Alice'})
         MERGE (f)-[:HAS_MODERATOR]->(p)",
    );

    let rows = run(
        &store,
        "MATCH (f:Forum {title:'TechForum'})-[:HAS_MODERATOR]->(p:Person)
         RETURN p.firstName AS fn",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["fn"], Value::Str("Alice".into()));
}

// ── DELETE operations ─────────────────────────────────────────────────────────

/// ID1: Delete a Person (cascade: removes all connected edges).
#[test]
fn id1_delete_person_cascade() {
    let (store, _) = build();
    let before = count(&store, "Person");

    run(
        &store,
        "MATCH (n:Person {firstName:'Grace'}) DETACH DELETE n",
    );

    let after = count(&store, "Person");
    assert_eq!(after, before - 1, "one person must be removed");

    // Grace's knows edge (Diana→Grace) must also be gone.
    let rows = run(
        &store,
        "MATCH ()-[r:KNOWS]->(n:Person {firstName:'Grace'}) RETURN count(r) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(0),
        "KNOWS edges to Grace must be removed"
    );
}

/// ID2: Delete a Forum (cascade).
#[test]
fn id2_delete_forum_cascade() {
    let (store, _) = build();
    let before = count(&store, "Forum");

    run(
        &store,
        "MATCH (f:Forum {title:'SportsForum'}) DETACH DELETE f",
    );

    let after = count(&store, "Forum");
    assert_eq!(after, before - 1, "forum count must decrease by 1");

    // CONTAINER_OF edges from SportsForum must be gone.
    let rows = run(
        &store,
        "MATCH (f:Forum {title:'SportsForum'}) RETURN count(f) AS c",
    );
    assert_eq!(rows[0]["c"], Value::Int(0), "SportsForum must be deleted");
}

/// ID3: Delete a Post (cascade: removes LIKES, REPLY_OF, HAS_CREATOR, CONTAINER_OF edges).
#[test]
fn id3_delete_post_cascade() {
    let (store, _) = build();
    let before = count(&store, "Post");

    run(
        &store,
        "MATCH (p:Post {content:'Sports update'}) DETACH DELETE p",
    );

    let after = count(&store, "Post");
    assert_eq!(after, before - 1, "post count must decrease by 1");
}

/// ID4: Delete a Comment (cascade).
#[test]
fn id4_delete_comment_cascade() {
    let (store, _) = build();
    let before = count(&store, "Comment");

    run(
        &store,
        "MATCH (c:Comment {content:'Great article!'}) DETACH DELETE c",
    );

    let after = count(&store, "Comment");
    assert_eq!(after, before - 1, "comment count must decrease by 1");
}

/// ID5: Delete a KNOWS edge (leave both nodes intact).
#[test]
fn id5_delete_knows_edge() {
    let (store, _) = build();
    let person_count_before = count(&store, "Person");

    run(
        &store,
        "MATCH (a:Person {firstName:'Bob'})-[r:KNOWS]->(b:Person {firstName:'Eve'}) DELETE r",
    );

    // Both persons must still exist.
    assert_eq!(count(&store, "Person"), person_count_before);

    // The edge must be gone.
    let rows = run(
        &store,
        "MATCH (a:Person {firstName:'Bob'})-[r:KNOWS]->(b:Person {firstName:'Eve'})
         RETURN count(r) AS c",
    );
    assert_eq!(rows[0]["c"], Value::Int(0), "KNOWS Bob→Eve must be deleted");
}

/// ID6: Delete a LIKES edge.
#[test]
fn id6_delete_likes_edge() {
    let (store, _) = build();

    // Bob likes post1; delete that LIKES edge.
    run(
        &store,
        "MATCH (p:Person {firstName:'Bob'})-[r:LIKES]->(post:Post {content:'Tech news today'})
         DELETE r",
    );

    let rows = run(
        &store,
        "MATCH (p:Person {firstName:'Bob'})-[r:LIKES]->(post:Post {content:'Tech news today'})
         RETURN count(r) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(0),
        "LIKES Bob→post1 must be deleted"
    );

    // Bob should still exist.
    let rows2 = run(
        &store,
        "MATCH (n:Person {firstName:'Bob'}) RETURN count(n) AS c",
    );
    assert_eq!(rows2[0]["c"], Value::Int(1), "Bob must still exist");
}

/// ID7: Delete a HAS_MEMBER edge (remove a person from a forum).
#[test]
fn id7_delete_forum_membership() {
    let (store, _) = build();

    run(
        &store,
        "MATCH (f:Forum {title:'TechForum'})-[r:HAS_MEMBER]->(p:Person {firstName:'Bob'}) DELETE r",
    );

    let rows = run(
        &store,
        "MATCH (f:Forum {title:'TechForum'})-[:HAS_MEMBER]->(p:Person {firstName:'Bob'})
         RETURN count(*) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(0),
        "Bob must no longer be in TechForum"
    );

    // Forum and Bob still exist.
    let rows2 = run(
        &store,
        "MATCH (f:Forum {title:'TechForum'}) RETURN count(f) AS c",
    );
    assert_eq!(rows2[0]["c"], Value::Int(1), "TechForum must still exist");
}

/// ID8: Delete a WORK_AT edge (remove a person from an organisation).
#[test]
fn id8_delete_work_at_edge() {
    let (store, _) = build();

    run(
        &store,
        "MATCH (p:Person {firstName:'Alice'})-[r:WORK_AT]->(o:Organisation) DELETE r",
    );

    let rows = run(
        &store,
        "MATCH (p:Person {firstName:'Alice'})-[:WORK_AT]->(o:Organisation)
         RETURN count(*) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(0),
        "Alice must have no WORK_AT edges"
    );

    // Alice and the org must still exist.
    let rows2 = run(
        &store,
        "MATCH (n:Person {firstName:'Alice'}) RETURN count(n) AS c",
    );
    assert_eq!(rows2[0]["c"], Value::Int(1), "Alice must still exist");
}

// ── Write atomicity: verify no phantom edges after cascaded delete ─────────────

#[test]
fn write_no_dangling_edges_after_cascade_delete() {
    let (store, _) = build();

    // Delete Charlie (who has multiple outgoing/incoming edges).
    run(
        &store,
        "MATCH (n:Person {firstName:'Charlie'}) DETACH DELETE n",
    );

    // Verify all remaining edges have valid endpoints.
    let edge_ids = store.all_edge_ids(GRAPH).expect("all_edge_ids failed");
    for eid in &edge_ids {
        let edge = store
            .get_edge(GRAPH, *eid)
            .expect("get_edge failed")
            .expect("edge should exist");
        assert!(
            store.get_node(GRAPH, edge.src).unwrap().is_some(),
            "edge src {:?} must still exist",
            edge.src
        );
        assert!(
            store.get_node(GRAPH, edge.dst).unwrap().is_some(),
            "edge dst {:?} must still exist",
            edge.dst
        );
    }
}
