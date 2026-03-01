# TestKit Conformance Test Results

**Date:** 2026-03-01
**sedandelivery-bolt version:** 0.1.0 (Bolt v4.x)
**neo4j Python driver:** 6.0.3
**testkit:** latest (neo4j-drivers/testkit, main branch)

## Summary

| Suite | Passed | Failed | Error | Skipped | Total |
|-------|--------|--------|-------|---------|-------|
| test_session_run | 19 | 0 | 0 | 0 | 19 |
| test_tx_run | 24 | 0 | 0 | 0 | 24 |
| test_tx_func_run | 9 | 0 | 0 | 0 | 9 |
| test_authentication | 4 | 0 | 0 | 0 | 4 |
| test_bookmarks | 8 | 0 | 0 | 0 | 8 |
| test_summary | 15 | 0 | 0 | 0 | 15 |
| test_direct_driver | 6 | 0 | 0 | 0 | 6 |
| test_datatypes | 11 | 0 | 0 | 0 | 11 |
| test_spatial_types | 4 | 0 | 0 | 0 | 4 |
| test_temporal_types | 17 | 0 | 0 | 1 | 18 |
| **Total** | **117** | **0** | **0** | **1** | **118** |

**Overall pass rate: 117/118 (99.2%), 0 failures, 1 skipped**

## Skipped Tests (1)

### Python Driver Limitation (1 skip) — Accepted as architectural limit

- `test_should_echo_temporal_type` (1 subtest) -- Python's pytz library cannot
  distinguish between `+00:00` (fixed offset) and `UTC` (named timezone). The
  driver skips this subtest with: "timezone library cannot tell the difference
  between named UTC and 0s offset timezone"

**Root cause analysis (Issue #334, Step 3):**

The server correctly distinguishes these two representations using different
Bolt protocol tags:

| Input                 | Bolt Tag | Field 3          | Notes                |
|-----------------------|----------|------------------|----------------------|
| `UTC` (named tz)     | `0x69`   | String("UTC")    | TAG_DATETIME_ZONE_ID_UTC |
| `+00:00` (fixed)     | `0x49`   | Int(0)           | TAG_DATETIME_ZONE_OFFSET_UTC |

The skip occurs because `pytz.timezone("UTC")` and `pytz.FixedOffset(0)` are
treated as interchangeable by the Python driver's test assertion. The Python
`zoneinfo` module (3.9+) does distinguish them, but the neo4j driver's testkit
uses `pytz` internally.

**Decision**: Accept 117/118 as the correct result (Option C). Changing the
server's encoding to work around pytz would violate the Bolt protocol spec.
The server sends correct, distinguishable structures — the limitation is in the
test harness, not in the protocol implementation.

## What Works

### Session Run (19/19)
- Simple queries (RETURN, UNWIND)
- Parameterized queries
- Iteration (small, large, nested, partial)
- Error recovery from invalid queries
- Regex in queries and parameters
- Session reuse
- Bookmark tracking after commits
- Autocommit metadata and timeout support
- Path echo support
- Long string support
- Missing parameter error detection

### Transaction Run (24/24)
- Explicit transactions (begin, commit, rollback)
- Error handling within transactions
- Transaction state management (no re-run after commit/rollback)
- Bookmark tracking and validation
- Transaction configuration and metadata
- Transaction timeout with lock contention
- Interwoven and parallel queries
- Invalid bookmark detection
- Unconsumed result handling

### Transaction Functions (9/9)
- Simple read/write transaction functions
- Parameterized queries in tx functions
- Bookmark management
- Client exception rolls back changes
- Nested iteration in tx functions
- Transaction metadata via tx.getMetaData procedure
- Transaction function configuration and timeout

### Authentication (4/4)
- Basic auth (correct credentials accepted)
- Auth with realm
- Incorrect credentials rejected with proper error code
- Incorrect credentials in transactions rejected

### Bookmarks (8/8)
- Bookmark creation after commit
- Multiple bookmark handling
- No bookmark after rollback
- Cross-session bookmark passing (read -> write, write -> write)
- Invalid bookmark rejection with proper error code
- Invalid bookmark rejection via tx functions

### Summary (15/15)
- Server address in summary
- Time information
- Status codes (success, no_data, omitted)
- Protocol version
- Summary counters (CREATE, MATCH counts)
- Agent string (Neo4j/4.4.0)
- EXPLAIN plan info
- PROFILE plan info
- Notification info (cartesian product warnings)
- Summary after consume
- Multi-database summary counters (CREATE/DROP DATABASE, CREATE/DROP INDEX,
  CREATE/DROP CONSTRAINT with system_updates, indexes_added/removed,
  constraints_added/removed, labels_removed)

### Direct Driver (6/6)
- Custom address resolution
- HTTP port detection
- Multi-db support detection (correctly reports supported)
- Multi-db with non-existing database (DatabaseNotFound error)
- Multi-db create/drop database and query isolation
- Multi-db various databases (isolation between testa, testb, default)

### Datatypes (11/11)
- Primitive echo (bool, null, int, float, string, bytes)
- Maps, lists, nested structures
- Nodes and relationships
- Path echo with index sequence
- Very long strings, lists, maps

### Spatial Types (4/4)
- Point echo for all 4 CRS types (WGS-84 2D/3D, Cartesian 2D/3D)
- Point components (x, y, z, longitude, latitude, height)
- Cypher-created points (point({x:3, y:4}), point({longitude:3, latitude:4}))
- Nested points in lists

### Temporal Types (17/18, 1 skipped)
- Date echo and Cypher creation
- Time echo (local, zoned with offset)
- DateTime echo (local, zoned with offset, zoned with timezone ID)
- Duration echo and Cypher creation
- All timezone ID echo (hundreds of IANA zones)
- Date/time/datetime components extraction
- Duration components extraction
- Nested temporal types in lists
- Cypher datetime creation with various formats

## Multi-Database Support

sedandelivery-bolt implements enterprise-level multi-database support:

- **CREATE DATABASE / DROP DATABASE**: Full lifecycle management with IF NOT
  EXISTS / IF EXISTS support
- **SHOW DATABASES**: Lists all databases including the virtual "system" database
- **Database isolation**: Data created in one database is invisible to queries
  targeting a different database
- **DatabaseNotFound errors**: Queries targeting non-existent databases return
  proper `Neo.ClientError.Database.DatabaseNotFound` errors
- **Default database**: The "neo4j" database is pre-created at server startup

### DDL Support

- **CREATE INDEX / DROP INDEX**: Index lifecycle management with proper
  `indexes_added` / `indexes_removed` counters
- **CREATE CONSTRAINT / DROP CONSTRAINT**: Constraint lifecycle (accepted and
  tracked for counters, not enforced)
- **labels_removed**: REMOVE label operations correctly tracked

## Running Tests

```bash
# Setup (one-time)
./sedandelivery-bolt/tests/testkit/setup.sh

# Run all integration tests
./sedandelivery-bolt/tests/testkit/run_testkit.sh

# Run only passing tests (for CI)
./sedandelivery-bolt/tests/testkit/run_testkit.sh --passing-only

# Run specific suite
./sedandelivery-bolt/tests/testkit/run_testkit.sh session_run
```

## Stub Tests

The stub tests use a "bolt stub server" that speaks scripted Bolt protocol.
These test the *driver's* protocol handling, not the server's.
They are useful for verifying that the Python driver correctly handles Bolt protocol
edge cases, but they do not test sedandelivery-bolt directly.

Stub tests that passed (276+ across 23 test modules):
- `tests.stub.authorization` -- 157/157 (auth token managers, bearer, basic, user switching)
- `tests.stub.basic_query` -- 6/6 (element IDs for nodes, rels, paths)
- `tests.stub.bookmarks` -- 21/21 (v3, v4, v5 bookmark handling)
- `tests.stub.configuration_hints` -- 14/14 (recv timeout)
- `tests.stub.connectivity_check` -- 36/36 (server info, verify connectivity)
- `tests.stub.datatypes` -- 23/23 (temporal, unsupported types, vectors)
- `tests.stub.disconnects` -- 11/12 (connection handling)
- `tests.stub.driver_execute_query` -- 15/15 (execute query API)
- `tests.stub.driver_parameters` -- 41/41 (bookmark managers, agent strings, timeouts)
- `tests.stub.errors` -- 8/8 (GQL errors, classifications, diagnostic records)
- `tests.stub.session_run` -- 5/5 (discard on close, error on run)
- `tests.stub.transport` -- 6/6 (handshakes, noops, version negotiation)

For testing sedandelivery-bolt as a *server*, the integration tests (`tests.neo4j.*`) are the
appropriate suite.
