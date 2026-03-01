"""
Test e) -- RUN + PULL at byte level.

Validates the query execution flow:
  - RUN returns SUCCESS with 'fields' metadata
  - PULL returns RECORD messages followed by SUCCESS
  - SUCCESS after PULL contains bookmark, type, has_more
  - Multiple queries in sequence
  - Parameterized queries
  - DISCARD instead of PULL
"""

import pytest

from bolt_wire import (
    BoltConnection,
    Structure,
    TAG_SUCCESS,
    TAG_RECORD,
    TAG_FAILURE,
    msg_run,
    msg_pull,
    msg_discard,
)


class TestRunSuccess:
    """Tests for RUN message handling."""

    def test_run_returns_success(self, bolt_server):
        """RUN with a valid query returns SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("RETURN 1 AS n"))
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

    def test_run_success_has_fields(self, bolt_server):
        """RUN SUCCESS metadata contains 'fields' list."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("RETURN 1 AS n"))
            resp = conn.recv()
            metadata = resp.fields[0]
            assert "fields" in metadata
            assert isinstance(metadata["fields"], list)

    def test_run_fields_match_query(self, bolt_server):
        """RUN SUCCESS 'fields' matches the query's column names."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("RETURN 1 AS n"))
            resp = conn.recv()
            fields = resp.fields[0]["fields"]
            assert fields == ["n"], f"Expected ['n'], got {fields}"

    def test_run_success_has_t_first(self, bolt_server):
        """RUN SUCCESS metadata contains 't_first' timing field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("RETURN 1 AS n"))
            resp = conn.recv()
            metadata = resp.fields[0]
            assert "t_first" in metadata


class TestPullRecords:
    """Tests for PULL message handling and record retrieval."""

    def test_pull_returns_records_then_success(self, bolt_server):
        """PULL after RUN returns RECORD(s) followed by SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            run_resp, records, summary = conn.run_pull("RETURN 1 AS n")
            assert run_resp.tag == TAG_SUCCESS
            assert summary.tag == TAG_SUCCESS
            assert len(records) >= 1

    def test_record_structure_format(self, bolt_server):
        """RECORD is a Structure with tag 0x71 and 1 field (a list of values)."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            _, records, _ = conn.run_pull("RETURN 1 AS n")
            record = records[0]
            assert record.tag == TAG_RECORD
            assert len(record.fields) == 1
            assert isinstance(record.fields[0], list)

    def test_record_value_matches_query(self, bolt_server):
        """RECORD contains the value requested by the query."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            _, records, _ = conn.run_pull("RETURN 42 AS answer")
            assert records[0].fields[0] == [42]

    def test_pull_summary_has_bookmark(self, bolt_server):
        """PULL SUCCESS contains a 'bookmark' field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            _, _, summary = conn.run_pull("RETURN 1 AS n")
            metadata = summary.fields[0]
            assert "bookmark" in metadata
            assert isinstance(metadata["bookmark"], str)

    def test_pull_summary_has_type(self, bolt_server):
        """PULL SUCCESS contains a 'type' field (e.g. 'r' for read)."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            _, _, summary = conn.run_pull("RETURN 1 AS n")
            metadata = summary.fields[0]
            assert "type" in metadata
            assert isinstance(metadata["type"], str)

    def test_pull_summary_has_more_false(self, bolt_server):
        """PULL SUCCESS has 'has_more' = false when all records retrieved."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            _, _, summary = conn.run_pull("RETURN 1 AS n")
            metadata = summary.fields[0]
            assert "has_more" in metadata
            assert metadata["has_more"] is False

    def test_pull_summary_has_t_last(self, bolt_server):
        """PULL SUCCESS contains 't_last' timing field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            _, _, summary = conn.run_pull("RETURN 1 AS n")
            metadata = summary.fields[0]
            assert "t_last" in metadata

    def test_multiple_records(self, bolt_server):
        """Query returning multiple rows produces multiple RECORD messages."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            # UNWIND produces multiple rows
            _, records, summary = conn.run_pull("UNWIND [1, 2, 3] AS x RETURN x")
            assert summary.tag == TAG_SUCCESS
            assert len(records) == 3
            values = [r.fields[0][0] for r in records]
            assert values == [1, 2, 3]

    def test_no_records_for_empty_result(self, bolt_server):
        """Query with no results produces zero RECORD messages."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            # MATCH on empty graph returns no rows
            _, records, summary = conn.run_pull(
                "MATCH (n:NonexistentLabelXYZ123) RETURN n"
            )
            assert summary.tag == TAG_SUCCESS
            assert len(records) == 0


class TestPullWithN:
    """Tests for batched PULL with n parameter."""

    def test_pull_n_1(self, bolt_server):
        """PULL with n=1 returns exactly 1 record and has_more=true."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("UNWIND [1, 2, 3] AS x RETURN x"))
            run_resp = conn.recv()
            assert run_resp.tag == TAG_SUCCESS

            # Pull 1 record
            conn.send(msg_pull(1))
            records = []
            while True:
                msg = conn.recv()
                if msg.tag == TAG_SUCCESS:
                    metadata = msg.fields[0]
                    break
                records.append(msg)

            assert len(records) == 1
            assert metadata.get("has_more") is True

            # Pull remaining
            conn.send(msg_pull(-1))
            while True:
                msg = conn.recv()
                if msg.tag == TAG_SUCCESS:
                    final_meta = msg.fields[0]
                    break
                records.append(msg)

            assert len(records) == 3
            assert final_meta.get("has_more") is False


class TestDiscard:
    """Tests for DISCARD message."""

    def test_discard_after_run(self, bolt_server):
        """DISCARD after RUN returns SUCCESS and discards results."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("RETURN 1 AS n"))
            run_resp = conn.recv()
            assert run_resp.tag == TAG_SUCCESS

            conn.send(msg_discard())
            discard_resp = conn.recv()
            assert discard_resp.tag == TAG_SUCCESS

    def test_ready_after_discard(self, bolt_server):
        """Connection returns to Ready state after DISCARD."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("RETURN 1 AS n"))
            conn.recv()  # RUN SUCCESS

            conn.send(msg_discard())
            conn.recv()  # DISCARD SUCCESS

            # Should be able to run another query
            run_resp, records, summary = conn.run_pull("RETURN 2 AS m")
            assert run_resp.tag == TAG_SUCCESS
            assert records[0].fields[0] == [2]


class TestMultipleQueries:
    """Tests for running multiple queries in sequence."""

    def test_sequential_queries(self, bolt_server):
        """Multiple RUN+PULL sequences work on same connection."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            for i in range(5):
                _, records, summary = conn.run_pull(f"RETURN {i} AS n")
                assert summary.tag == TAG_SUCCESS
                assert records[0].fields[0] == [i]

    def test_bookmarks_increment(self, bolt_server):
        """Each query produces an incrementing bookmark."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            bookmarks = []
            for i in range(3):
                _, _, summary = conn.run_pull(f"RETURN {i} AS n")
                bookmarks.append(summary.fields[0]["bookmark"])
            # All bookmarks should be different
            assert len(set(bookmarks)) == 3
