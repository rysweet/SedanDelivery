"""
Test d) -- HELLO/SUCCESS exchange at byte level.

Validates the HELLO message handling:
  - Server returns SUCCESS with 'server' and 'connection_id' metadata
  - Server identifier string format
  - Connection ID uniqueness across connections
"""

import time

import pytest

from bolt_wire import (
    BoltConnection,
    Structure,
    TAG_SUCCESS,
    TAG_FAILURE,
    msg_hello,
)


class TestHelloSuccess:
    """Tests for successful HELLO exchange."""

    def test_hello_returns_success(self, bolt_server):
        """HELLO with valid user_agent returns SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            assert resp.tag == TAG_SUCCESS, \
                f"Expected SUCCESS (0x70), got tag 0x{resp.tag:02X}"

    def test_success_has_server_field(self, bolt_server):
        """SUCCESS metadata contains 'server' field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            metadata = resp.fields[0]
            assert "server" in metadata, \
                f"Missing 'server' in metadata: {metadata}"

    def test_server_identifier_format(self, bolt_server):
        """Server identifier follows 'Name/Version' format."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            server = resp.fields[0]["server"]
            assert isinstance(server, str)
            assert "/" in server, \
                f"Server identifier should be 'Name/Version', got: {server}"
            assert "SedanDelivery" in server or "sedan" in server.lower(), \
                f"Server identifier should contain 'SedanDelivery', got: {server}"

    def test_success_has_connection_id(self, bolt_server):
        """SUCCESS metadata contains 'connection_id' field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            metadata = resp.fields[0]
            assert "connection_id" in metadata, \
                f"Missing 'connection_id' in metadata: {metadata}"

    def test_connection_id_is_string(self, bolt_server):
        """connection_id is a string value."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            conn_id = resp.fields[0]["connection_id"]
            assert isinstance(conn_id, str)

    def test_connection_ids_are_unique(self, bolt_server):
        """Each connection gets a unique connection_id."""
        host, port = bolt_server
        ids = []
        for _ in range(3):
            with BoltConnection(host, port, timeout=10.0) as conn:
                resp = conn.hello()
                ids.append(resp.fields[0]["connection_id"])
            time.sleep(0.1)  # allow server to clean up
        assert len(set(ids)) == 3, \
            f"Expected 3 unique connection IDs, got: {ids}"

    def test_hello_with_custom_user_agent(self, bolt_server):
        """HELLO accepts a custom user_agent string."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello(user_agent="my-custom-agent/2.0")
            assert resp.tag == TAG_SUCCESS

    def test_hello_metadata_is_map(self, bolt_server):
        """SUCCESS metadata field is a PackStream map."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            assert isinstance(resp.fields[0], dict), \
                f"Expected dict metadata, got {type(resp.fields[0])}"

    def test_hello_structure_format(self, bolt_server):
        """SUCCESS response is a Structure with tag 0x70 and exactly 1 field."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            assert resp.tag == 0x70
            assert len(resp.fields) == 1
