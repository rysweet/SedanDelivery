#!/bin/bash
set -e
unset CLAUDECODE
cd /tmp/amplihack-workstreams/ws-r4-executor
claude -p "You are working on branch feat/tck-r4-executor in a Rust GQL/Cypher query engine (SedanDelivery). Current TCK: 2473/3806 (65.0%). Target: 100%.

YOUR TASK: Fix the 257 row count mismatch failures. These mean we return the wrong NUMBER of rows.

Key root causes:
1. Multi-label nodes: MATCH (n:A:B) should match nodes with BOTH labels A and B. Check if NodeScan and label filtering handle multiple labels.
2. OPTIONAL MATCH: Should produce NULL rows when no match found, not skip. Check LeftOuterApply operator.
3. UNWIND on NULL/empty: UNWIND null AS x should produce 0 rows. UNWIND [] AS x should produce 0 rows.
4. WITH clause: WITH a, b WHERE condition should filter AFTER projection. Check plan_with ordering.
5. Aggregation: COUNT(*) on empty input should return 0 (single row with 0), not empty result. Check Aggregate operator empty-input handling.
6. MERGE: Should CREATE if not exists, MATCH if exists. Check Merge operator.
7. Multiple MATCH patterns: MATCH (a), (b) is a cross product. MATCH (a)-[r]->(b), (b)-[s]->(c) shares variable b.
8. Pattern variable reuse: If same variable appears in multiple patterns, rows where the variable binds to different nodes should be filtered out.

Key files: gql-core/src/executor/engine.rs, gql-core/src/planner/planner.rs
Steps: Read code, implement fixes, cargo test -p gql-core --lib, cargo test --test tck 2>&1 | tail -20, commit, push, create PR.
" --max-turns 200 2>&1
