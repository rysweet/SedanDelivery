"""
Test g) -- GOODBYE graceful close.

Validates that sedan-bolt handles GOODBYE correctly:
  - Server closes the connection after receiving GOODBYE
  - No response is expected for GOODBYE
  - Connection is cleanly terminated
"""

import socket
import time

import pytest

from bolt_wire import (
    BoltConnection,
    Structure,
    TAG_SUCCESS,
    chunk_encode,
    packstream_encode,
    recv_exact,
    msg_hello,
    msg_goodbye,
    msg_run,
    msg_pull,
)


class TestGoodbye:
    """Tests for GOODBYE message handling."""

    def test_goodbye_closes_connection(self, bolt_server):
        """Server closes the TCP connection after receiving GOODBYE."""
        host, port = bolt_server
        conn = BoltConnection(host, port)
        resp = conn.hello()
        assert resp.tag == TAG_SUCCESS

        # Send GOODBYE
        conn.send(msg_goodbye())

        # Give server a moment to process
        time.sleep(0.2)

        # The connection should be closed -- recv should return empty
        conn.sock.settimeout(3)
        try:
            data = conn.sock.recv(1024)
            # Empty data means connection closed
            assert data == b"", \
                f"Expected empty data (connection closed), got {len(data)} bytes"
        except (ConnectionError, ConnectionResetError, OSError):
            pass  # Also acceptable -- connection reset/closed
        finally:
            conn.close()

    def test_goodbye_after_query(self, bolt_server):
        """GOODBYE works correctly after running a query."""
        host, port = bolt_server
        conn = BoltConnection(host, port)
        resp = conn.hello()
        assert resp.tag == TAG_SUCCESS

        # Run a query first
        conn.send(msg_run("RETURN 1 AS n"))
        conn.recv()
        conn.send(msg_pull())
        while True:
            msg = conn.recv()
            if msg.tag == TAG_SUCCESS:
                break

        # Now send GOODBYE
        conn.send(msg_goodbye())
        time.sleep(0.2)

        conn.sock.settimeout(3)
        try:
            data = conn.sock.recv(1024)
            assert data == b""
        except (ConnectionError, ConnectionResetError, OSError):
            pass
        finally:
            conn.close()

    def test_goodbye_immediately_after_hello(self, bolt_server):
        """GOODBYE can be sent immediately after HELLO."""
        host, port = bolt_server
        conn = BoltConnection(host, port)
        resp = conn.hello()
        assert resp.tag == TAG_SUCCESS

        conn.send(msg_goodbye())
        time.sleep(0.2)

        conn.sock.settimeout(3)
        try:
            data = conn.sock.recv(1024)
            assert data == b""
        except (ConnectionError, ConnectionResetError, OSError):
            pass
        finally:
            conn.close()

    def test_new_connection_after_goodbye(self, bolt_server):
        """Server accepts new connections after a GOODBYE on another connection."""
        host, port = bolt_server

        # First connection
        conn1 = BoltConnection(host, port)
        resp1 = conn1.hello()
        assert resp1.tag == TAG_SUCCESS
        conn1.send(msg_goodbye())
        time.sleep(0.2)
        conn1.close()

        # Second connection should work fine
        with BoltConnection(host, port) as conn2:
            resp2 = conn2.hello()
            assert resp2.tag == TAG_SUCCESS
            _, records, summary = conn2.run_pull("RETURN 1 AS n")
            assert summary.tag == TAG_SUCCESS
