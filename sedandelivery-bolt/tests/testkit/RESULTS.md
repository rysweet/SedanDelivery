# TestKit Conformance Test Results

**Date:** 2026-02-28
**sedandelivery-bolt version:** 0.1.0 (Bolt v4.x)
**neo4j Python driver:** 6.0.3
**testkit:** latest (neo4j-drivers/testkit, main branch)

## Summary

| Suite | Passed | Failed | Error | Skipped | Total |
|-------|--------|--------|-------|---------|-------|
| test_session_run | 13 | 2 | 4 | 0 | 19 |
| test_tx_run | 15 | 4 | 2 | 0 | 21 |
| test_tx_func_run | 4 | 1 | 3 | 0 | 8 |
| test_authentication | 2 | 2 | 0 | 0 | 4 |
| test_bookmarks | 3 | 2 | 3 | 0 | 8 |
| test_summary | 10 | 1 | 3 | 1 | 15 |
| test_direct_driver | 2 | 1 | 0 | 3 | 6 |
| test_datatypes | 8 | 1 | 1 | 0 | 10* |
| test_spatial_types | 0 | 1 | 0 | 0 | 4* |
| test_temporal_types | 2 | 5 | 9 | 0 | 26* |
| **Total** | **~58** | **~20** | **~25** | **~4** | **~121** |

*Note: Subtest counts differ from unittest `Ran N` summary due to parameterized tests.

**Overall pass rate: ~56% (58/104 unique tests)**

## What Works

### Session Run (13/19)
- Simple queries (RETURN, UNWIND)
- Parameterized queries
- Iteration (small, large, nested, partial)
- Error recovery from invalid queries
- Regex in queries and parameters
- Session reuse
- Bookmark tracking after commits

### Transaction Run (15/21)
- Explicit transactions (begin, commit, rollback)
- Error handling within transactions
- Transaction state management (no re-run after commit/rollback)
- Bookmark tracking

### Transaction Functions (4/8)
- Simple read/write transaction functions
- Parameterized queries in tx functions
- Bookmark management

### Authentication (2/4)
- Basic auth (any credentials accepted)
- Auth with realm

### Bookmarks (3/8)
- Bookmark creation after commit
- Multiple bookmark handling
- No bookmark after rollback

### Summary (10/15)
- Server address in summary
- Time information
- Status codes (success, no_data, omitted)
- Protocol version
- Summary counters (CREATE, MATCH counts)
- Notification absence

### Datatypes (8/11)
- Maps, lists, nested structures
- Nodes and relationships
- Very long strings, lists, maps
- Date and Duration Cypher creation

### Direct Driver (2/6)
- Custom address resolution
- HTTP port detection

## What Fails and Why

### Category 1: Unimplemented Features (ERROR)
These tests timeout because sedandelivery-bolt doesn't support the feature:

- **tx_metadata**: `test_autocommit_transactions_should_support_metadata`, `test_tx_configuration`, `test_tx_func_configuration`, `test_meta_data` -- sedandelivery-bolt ignores tx metadata
- **EXPLAIN/PROFILE**: `test_can_obtain_plan_info`, `test_can_obtain_profile_info` -- not implemented
- **Notifications**: `test_can_obtain_notification_info` -- not implemented
- **Path echo**: `test_can_return_path`, `test_should_echo_path` -- path serialization issues
- **Bookmark validation**: `test_can_pass_bookmark_into_next_session`, `test_can_pass_read_bookmark_into_write_session`, `test_can_pass_write_bookmark_into_write_session` -- cross-session bookmark chaining
- **Long strings**: `test_long_string` -- chunking issues with very large payloads
- **Temporal components**: Various temporal component extraction tests -- sedandelivery-bolt doesn't decompose temporal values into components
- **Timezone IDs**: `test_should_echo_all_timezone_ids`, `test_date_time_cypher_created_tz_id` -- timezone ID support incomplete

### Category 2: Wrong Error Codes (FAIL)
sedandelivery-bolt returns different error codes than Neo4j:

- `test_fails_on_missing_parameter`: Returns `Neo.ClientError.Statement.TypeError` instead of `Neo.ClientError.Statement.ParameterMissing`
- `test_error_on_incorrect_credentials` / `test_error_on_incorrect_credentials_tx`: sedandelivery-bolt accepts any auth, doesn't reject bad credentials
- `test_fails_on_invalid_bookmark` / `test_fails_on_invalid_bookmark_using_tx_func`: Invalid bookmark handling differs
- `test_should_fail_to_run_query_for_invalid_bookmark`: Same bookmark validation issue

### Category 3: Missing Protocol Features (FAIL)
- `test_autocommit_transactions_should_support_timeout`: Query timeout not implemented
- `test_tx_timeout`: Transaction timeout not implemented
- `test_agent_string`: Server agent string differs from expected format
- `test_supports_multi_db`: Multi-database support detection
- `test_can_rollback_transaction`: Rollback behavior differs slightly
- `test_client_exception_rolls_back_change`: Transaction rollback on client exception

### Category 4: Temporal/Spatial Type Gaps (FAIL)
- Temporal type echo (Date, Time, DateTime, Duration) -- packstream encoding of temporal types
- Spatial point echo -- packstream encoding of spatial types
- Nested temporal/spatial values

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

## What Would Be Needed for Full TestKit

### Stub Tests
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

### Full Integration (100%)
To pass all integration tests, sedandelivery-bolt would need:

1. **Authentication rejection** -- Return `Neo.ClientError.Security.Unauthorized` for bad credentials
2. **Error code alignment** -- Map all error codes to Neo4j-compatible codes
3. **Query timeout** -- Implement `tx_timeout` metadata on BEGIN/RUN
4. **TX metadata** -- Store and echo back transaction metadata
5. **EXPLAIN/PROFILE** -- Return query plans
6. **Notifications** -- Return query notifications/warnings
7. **Multi-database support** -- Implement database selection in HELLO/LOGON
8. **Bookmark validation** -- Reject invalid bookmark formats
9. **Cross-session bookmarks** -- Honor bookmarks from previous sessions
10. **Temporal types** -- Full packstream encode/decode for Date, Time, DateTime, Duration
11. **Spatial types** -- Full packstream encode/decode for Point2D, Point3D
12. **Path serialization** -- Correct path structure in results
13. **Server agent string** -- Return `Neo4j/4.4.0` format in connection metadata

### Docker-Based Full TestKit
The full testkit (`main.py`) uses Docker to:
1. Build a driver backend image
2. Start a Neo4j server container
3. Run tests in an isolated network

To use this with sedandelivery-bolt, we would need:
- A Dockerfile for sedandelivery-bolt
- A custom `neo4j.py` adapter (or use `--external-integration`)
- Network configuration so testkit can reach sedandelivery-bolt

The `--external-integration` mode with manual server startup is the most practical path.
