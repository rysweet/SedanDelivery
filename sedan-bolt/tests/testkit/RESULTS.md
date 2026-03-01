# TestKit Conformance Test Results

**Date:** 2026-02-28
**sedan-bolt version:** 0.1.0 (Bolt v4.x)
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
| test_summary | 14 | 0 | 0 | 1 | 15 |
| test_direct_driver | 3 | 0 | 0 | 3 | 6 |
| test_datatypes | 11 | 0 | 0 | 0 | 11 |
| test_spatial_types | 4 | 0 | 0 | 0 | 4 |
| test_temporal_types | 17 | 0 | 0 | 1 | 18 |
| **Total** | **113** | **0** | **0** | **5** | **118** |

**Overall pass rate: 113/118 (95.8%), 0 failures, 5 skipped**

## Skipped Tests (5)

All 5 skips are expected and cannot be resolved:

### Multi-Database (4 skips)
sedan-bolt is a single-database engine. The Python driver correctly detects this
via `supports_multi_db()` and skips multi-database tests:

- `test_summary_counters_case_2` -- "Server does not support multiple databases."
- `test_multi_db` -- "Server does not support multiple databases."
- `test_multi_db_non_existing` -- "Needs multi DB support"
- `test_multi_db_various_databases` -- "Server does not support multiple databases."

### Python Driver Limitation (1 skip)
- `test_should_echo_temporal_type` (1 subtest) -- Python's pytz library cannot
  distinguish between `+00:00` (fixed offset) and `UTC` (named timezone). The
  driver skips this subtest with: "timezone library cannot tell the difference
  between named UTC and 0s offset timezone"

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

### Summary (14/15, 1 skipped)
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

### Direct Driver (3/6, 3 skipped)
- Custom address resolution
- HTTP port detection
- Multi-db support detection (correctly reports unsupported)

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

## Running Tests

```bash
# Setup (one-time)
./sedan-bolt/tests/testkit/setup.sh

# Run all integration tests
./sedan-bolt/tests/testkit/run_testkit.sh

# Run only passing tests (for CI)
./sedan-bolt/tests/testkit/run_testkit.sh --passing-only

# Run specific suite
./sedan-bolt/tests/testkit/run_testkit.sh session_run
```

## Stub Tests

The stub tests use a "bolt stub server" that speaks scripted Bolt protocol.
These test the *driver's* protocol handling, not the server's.
They are useful for verifying that the Python driver correctly handles Bolt protocol
edge cases, but they do not test sedan-bolt directly.

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

For testing sedan-bolt as a *server*, the integration tests (`tests.neo4j.*`) are the
appropriate suite.
