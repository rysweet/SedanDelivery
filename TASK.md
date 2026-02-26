# Workstream 2: Planner/Executor Correctness for TCK Compliance

## GitHub Issue: #15
## Branch: feat/tck-planner-executor
## Goal: 100% openCypher TCK compliance (planner/executor portion)

## Context
This is a Rust GQL/Cypher query engine. The planner converts AST to operator trees, and the executor runs them. Currently 362/3874 TCK scenarios pass (9.3%). ~960 failures are row count mismatches from planner/executor bugs.

## Tasks (ordered by impact)

### Task 1: Relationship variables in scope (~part of 960 row-count failures)
**Error**: Relationship variables not available in query results
**Root cause**: When an Expand operator has `rel_alias: Some(alias)`, the executor doesn't add `alias → Value::Edge(edge_id)` to the row. Currently only `dst_alias` is added; `rel_alias` is ignored.
**Fix**: In engine.rs where Expand is executed, after matching an edge, insert `rel_alias → Value::Edge(edge_id)` into the row alongside the destination node.

### Task 2: Named path variables (~26 failures)
**Error**: `expected variable 'p' in scope, got undefined`
**Root cause**: `MATCH p=(a)-[:KNOWS]->(b) RETURN p` — the named path variable `p` is parsed as `PathPattern.variable` but the planner doesn't create a row entry for it.
**Fix**: In `plan_path_pattern()` in planner.rs, when the pattern has a variable name, track all nodes and edges during traversal. After the full pattern is matched, add `name → Value::Path{nodes, edges}` to the row.

### Task 3: CREATE without variables (~40 failures)
**Error**: `execution error: expected variable '__src__' in scope, got undefined`
**Root cause**: `CREATE ()-[:T]->()` uses anonymous nodes. The planner creates internal aliases like `__src__` and `__dst__` but the executor can't find them.
**Fix**: In `plan_create()`, when a node pattern has no variable, generate a unique internal alias (use `__anon_N__` naming) and ensure it's populated in the row by a preceding Create operation.

### Task 4: Multi-pattern CREATE
**Root cause**: `CREATE (a:A)-[:REL1]->(b:B), (b)-[:REL2]->(a)` — second pattern references variables from first. Must resolve variables across pattern boundaries within the same CREATE clause.
**Fix**: Track all created variables across patterns in the same CREATE clause so later patterns can reference earlier ones.

### Task 5: RETURN * expansion (~31 failures)
**Error**: `execution error: expected variable '*' in scope, got undefined`
**Root cause**: `RETURN *` is parsed but the planner emits a Project with literal `*` expression instead of expanding to all in-scope variables.
**Fix**: In `plan_return()`, detect `ReturnItem` with `Expr::Variable(Identifier("*".into()))` and expand it to all variables currently in scope from the preceding MATCH/WITH clauses.

## Key Files
- `gql-core/src/planner/planner.rs` (2115 lines) — main fix target
- `gql-core/src/executor/engine.rs` (2806 lines) — executor fixes
- `gql-core/src/ast/mod.rs` (~700 lines) — AST types

## Test Commands
```bash
cargo test -p gql-core --lib
cargo test -p gql-core --test end_to_end
cargo test --test tck 2>&1 | tail -20
cargo clippy -p gql-core -- -D warnings
```

## Architecture Rules (do not change)
- Parser: Recursive descent (not LALR)
- IDs: u64
- License: MIT OR Apache-2.0
- ExecutionBudget: Required for every query

## Definition of Done
- Relationship variables appear in query results
- Named path variables (p=...) work in MATCH/RETURN
- CREATE with anonymous nodes works
- Multi-pattern CREATE resolves cross-references
- RETURN * expands to all in-scope variables
- All existing tests still pass
- cargo clippy -D warnings clean
- cargo fmt clean
- Commit and create PR against main
