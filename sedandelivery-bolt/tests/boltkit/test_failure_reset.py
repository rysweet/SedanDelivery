"""
Test f) -- FAILURE + RESET recovery at byte level.

Validates error handling and recovery:
  - Invalid query triggers FAILURE response
  - FAILURE response has 'code' and 'message' fields
  - After FAILURE, server enters Failed state (IGNORED for other messages)
  - RESET brings server back to Ready state
  - Connection is usable after RESET
"""

import pytest

from bolt_wire import (
    BoltConnection,
    Structure,
    TAG_SUCCESS,
    TAG_FAILURE,
    TAG_IGNORED,
    msg_run,
    msg_pull,
    msg_reset,
)


class TestFailureResponse:
    """Tests for FAILURE message format."""

    def test_invalid_query_returns_failure(self, bolt_server):
        """An invalid query causes the server to return FAILURE."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("THIS IS NOT VALID CYPHER !!!"))
            resp = conn.recv()
            assert resp.tag == TAG_FAILURE, \
                f"Expected FAILURE (0x7F), got 0x{resp.tag:02X}"

    def test_failure_has_code(self, bolt_server):
        """FAILURE metadata contains a 'code' field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("INVALID QUERY"))
            resp = conn.recv()
            assert resp.tag == TAG_FAILURE
            metadata = resp.fields[0]
            assert "code" in metadata, \
                f"FAILURE missing 'code' field: {metadata}"
            assert isinstance(metadata["code"], str)

    def test_failure_has_message(self, bolt_server):
        """FAILURE metadata contains a 'message' field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("INVALID QUERY"))
            resp = conn.recv()
            assert resp.tag == TAG_FAILURE
            metadata = resp.fields[0]
            assert "message" in metadata, \
                f"FAILURE missing 'message' field: {metadata}"
            assert isinstance(metadata["message"], str)

    def test_failure_code_format(self, bolt_server):
        """FAILURE code follows Neo4j error code format (e.g. 'Neo.ClientError.Statement.*')."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("INVALID QUERY"))
            resp = conn.recv()
            code = resp.fields[0]["code"]
            assert "." in code, \
                f"Error code should be dotted, got: {code}"

    def test_failure_structure_format(self, bolt_server):
        """FAILURE is a Structure with tag 0x7F and exactly 1 field (a map)."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("INVALID"))
            resp = conn.recv()
            assert resp.tag == 0x7F
            assert len(resp.fields) == 1
            assert isinstance(resp.fields[0], dict)


class TestFailedState:
    """Tests for the Failed state behavior."""

    def test_run_after_failure_returns_ignored(self, bolt_server):
        """RUN in Failed state returns IGNORED."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            # Cause failure
            conn.send(msg_run("INVALID QUERY"))
            resp = conn.recv()
            assert resp.tag == TAG_FAILURE

            # Try to RUN again - should be IGNORED
            conn.send(msg_run("RETURN 1 AS n"))
            resp = conn.recv()
            assert resp.tag == TAG_IGNORED, \
                f"Expected IGNORED (0x7E), got 0x{resp.tag:02X}"

    def test_pull_after_failure_returns_ignored(self, bolt_server):
        """PULL in Failed state returns IGNORED."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("INVALID QUERY"))
            resp = conn.recv()
            assert resp.tag == TAG_FAILURE

            conn.send(msg_pull())
            resp = conn.recv()
            assert resp.tag == TAG_IGNORED

    def test_ignored_structure_format(self, bolt_server):
        """IGNORED is a Structure with tag 0x7E and 0 fields."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("INVALID"))
            conn.recv()  # FAILURE

            conn.send(msg_run("RETURN 1"))
            resp = conn.recv()
            assert resp.tag == TAG_IGNORED
            assert len(resp.fields) == 0


class TestResetRecovery:
    """Tests for RESET message and recovery from Failed state."""

    def test_reset_returns_success(self, bolt_server):
        """RESET always returns SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_reset())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

    def test_reset_after_failure_returns_success(self, bolt_server):
        """RESET from Failed state returns SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            # Enter Failed state
            conn.send(msg_run("INVALID"))
            conn.recv()

            # RESET
            conn.send(msg_reset())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

    def test_query_works_after_reset(self, bolt_server):
        """Connection is usable for queries after RESET."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            # Enter Failed state
            conn.send(msg_run("INVALID"))
            conn.recv()

            # RESET
            conn.send(msg_reset())
            conn.recv()

            # Should be back in Ready state
            run_resp, records, summary = conn.run_pull("RETURN 42 AS n")
            assert run_resp.tag == TAG_SUCCESS
            assert records[0].fields[0] == [42]
            assert summary.tag == TAG_SUCCESS

    def test_reset_clears_pending_result(self, bolt_server):
        """RESET in Streaming state discards pending results."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            # RUN a query (now in Streaming state)
            conn.send(msg_run("RETURN 1 AS n"))
            conn.recv()

            # RESET instead of PULL
            conn.send(msg_reset())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

            # Should be back in Ready state, can run new query
            run_resp, records, summary = conn.run_pull("RETURN 2 AS m")
            assert run_resp.tag == TAG_SUCCESS

    def test_multiple_reset_idempotent(self, bolt_server):
        """Multiple RESETs in a row all return SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            for _ in range(3):
                conn.send(msg_reset())
                resp = conn.recv()
                assert resp.tag == TAG_SUCCESS

    def test_full_failure_recovery_cycle(self, bolt_server):
        """Full cycle: query -> failure -> ignored -> reset -> query works."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()

            # 1. Good query
            r, recs, s = conn.run_pull("RETURN 1 AS a")
            assert s.tag == TAG_SUCCESS

            # 2. Bad query -> FAILURE
            conn.send(msg_run("BAD QUERY"))
            fail = conn.recv()
            assert fail.tag == TAG_FAILURE

            # 3. Another query -> IGNORED
            conn.send(msg_run("RETURN 2 AS b"))
            ign = conn.recv()
            assert ign.tag == TAG_IGNORED

            # 4. RESET -> SUCCESS
            conn.send(msg_reset())
            rst = conn.recv()
            assert rst.tag == TAG_SUCCESS

            # 5. Good query again
            r, recs, s = conn.run_pull("RETURN 3 AS c")
            assert s.tag == TAG_SUCCESS
            assert recs[0].fields[0] == [3]
