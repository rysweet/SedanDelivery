"""
Test h) -- Edge cases.

Validates handling of unusual/malformed inputs:
  - Bad magic bytes (various patterns)
  - Connection drops at various points
  - Multiple connections simultaneously
  - Transaction messages (BEGIN/COMMIT/ROLLBACK)
"""

import socket
import struct
import time
import threading

import pytest

from bolt_wire import (
    BOLT_MAGIC,
    BoltConnection,
    Structure,
    TAG_SUCCESS,
    TAG_FAILURE,
    TAG_RECORD,
    TAG_IGNORED,
    chunk_encode,
    packstream_encode,
    recv_exact,
    recv_structure,
    msg_hello,
    msg_goodbye,
    msg_run,
    msg_pull,
    msg_reset,
    msg_begin,
    msg_commit,
    msg_rollback,
)


class TestBadMagicVariants:
    """Tests for various invalid magic byte patterns."""

    @pytest.mark.parametrize("bad_magic", [
        b"\x00\x00\x00\x00",
        b"\xFF\xFF\xFF\xFF",
        b"\x60\x60\xB0\x18",  # off by one
        b"\x60\x60\xB0\x16",  # off by one the other way
        b"\x17\xB0\x60\x60",  # reversed
        b"\xDE\xAD\xBE\xEF",
    ])
    def test_bad_magic_bytes(self, bolt_server, bad_magic):
        """Server rejects invalid magic bytes."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            sock.sendall(bad_magic + bytes(16))
            sock.settimeout(3)
            try:
                data = sock.recv(1024)
                # Server should close or return nothing useful
            except (ConnectionError, socket.timeout, OSError):
                pass
        finally:
            sock.close()


class TestConcurrentConnections:
    """Tests for multiple simultaneous connections."""

    def test_two_concurrent_connections(self, bolt_server):
        """Two connections can operate independently."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn1, \
             BoltConnection(host, port) as conn2:
            resp1 = conn1.hello()
            resp2 = conn2.hello()
            assert resp1.tag == TAG_SUCCESS
            assert resp2.tag == TAG_SUCCESS

            # Both can run queries
            _, recs1, s1 = conn1.run_pull("RETURN 1 AS n")
            _, recs2, s2 = conn2.run_pull("RETURN 2 AS n")
            assert recs1[0].fields[0] == [1]
            assert recs2[0].fields[0] == [2]

    def test_five_concurrent_connections(self, bolt_server):
        """Five connections can operate simultaneously."""
        host, port = bolt_server
        conns = [BoltConnection(host, port) for _ in range(5)]
        try:
            for i, conn in enumerate(conns):
                resp = conn.hello()
                assert resp.tag == TAG_SUCCESS

            for i, conn in enumerate(conns):
                _, records, summary = conn.run_pull(f"RETURN {i} AS n")
                assert summary.tag == TAG_SUCCESS
                assert records[0].fields[0] == [i]
        finally:
            for conn in conns:
                try:
                    conn.send(msg_goodbye())
                except OSError:
                    pass
                conn.close()

    def test_threaded_concurrent_queries(self, bolt_server):
        """Multiple threads can query concurrently."""
        host, port = bolt_server
        results = {}
        errors = []

        def worker(idx):
            try:
                with BoltConnection(host, port) as conn:
                    conn.hello()
                    _, records, _ = conn.run_pull(f"RETURN {idx} AS n")
                    results[idx] = records[0].fields[0][0]
            except Exception as e:
                errors.append((idx, e))

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(5)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=10)

        assert not errors, f"Thread errors: {errors}"
        for i in range(5):
            assert results[i] == i


class TestTransactions:
    """Tests for BEGIN/COMMIT/ROLLBACK transaction flow."""

    def test_begin_commit(self, bolt_server):
        """BEGIN + COMMIT transaction flow."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()

            # BEGIN
            conn.send(msg_begin())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

            # RUN inside transaction
            conn.send(msg_run("RETURN 1 AS n"))
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

            # PULL
            conn.send(msg_pull())
            while True:
                msg = conn.recv()
                if msg.tag == TAG_SUCCESS:
                    break

            # COMMIT
            conn.send(msg_commit())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS
            assert "bookmark" in resp.fields[0]

    def test_begin_rollback(self, bolt_server):
        """BEGIN + ROLLBACK transaction flow."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()

            conn.send(msg_begin())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

            conn.send(msg_run("RETURN 1 AS n"))
            conn.recv()
            conn.send(msg_pull())
            while True:
                msg = conn.recv()
                if msg.tag == TAG_SUCCESS:
                    break

            # ROLLBACK
            conn.send(msg_rollback())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

    def test_query_after_commit(self, bolt_server):
        """Connection returns to Ready state after COMMIT."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()

            conn.send(msg_begin())
            conn.recv()
            conn.send(msg_run("RETURN 1 AS n"))
            conn.recv()
            conn.send(msg_pull())
            while True:
                msg = conn.recv()
                if msg.tag == TAG_SUCCESS:
                    break
            conn.send(msg_commit())
            conn.recv()

            # Should be back in Ready state
            _, records, summary = conn.run_pull("RETURN 2 AS m")
            assert summary.tag == TAG_SUCCESS
            assert records[0].fields[0] == [2]

    def test_query_after_rollback(self, bolt_server):
        """Connection returns to Ready state after ROLLBACK."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()

            conn.send(msg_begin())
            conn.recv()
            conn.send(msg_run("RETURN 1 AS n"))
            conn.recv()
            conn.send(msg_pull())
            while True:
                msg = conn.recv()
                if msg.tag == TAG_SUCCESS:
                    break
            conn.send(msg_rollback())
            conn.recv()

            _, records, summary = conn.run_pull("RETURN 3 AS m")
            assert summary.tag == TAG_SUCCESS
            assert records[0].fields[0] == [3]

    def test_multiple_queries_in_transaction(self, bolt_server):
        """Multiple RUN+PULL sequences within a single transaction."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()

            conn.send(msg_begin())
            conn.recv()

            for i in range(3):
                _, records, summary = conn.run_pull(f"RETURN {i} AS n")
                assert summary.tag == TAG_SUCCESS
                assert records[0].fields[0] == [i]

            conn.send(msg_commit())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS


class TestConnectionDrop:
    """Tests for handling abrupt connection drops."""

    def test_close_before_handshake_complete(self, bolt_server):
        """Server handles client closing before completing handshake."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        # Send partial handshake (just magic, no versions)
        sock.sendall(BOLT_MAGIC)
        sock.close()
        # Server should not crash - verify by making another connection
        time.sleep(0.3)
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            assert resp.tag == TAG_SUCCESS

    def test_close_immediately(self, bolt_server):
        """Server handles immediate connection close."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        sock.close()
        time.sleep(0.3)
        # Server should still work
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            assert resp.tag == TAG_SUCCESS

    def test_close_after_hello(self, bolt_server):
        """Server handles disconnect after HELLO without GOODBYE."""
        host, port = bolt_server
        conn = BoltConnection(host, port)
        conn.hello()
        conn.sock.close()  # Close without GOODBYE
        time.sleep(0.3)
        # Server should still accept new connections
        with BoltConnection(host, port) as conn2:
            resp = conn2.hello()
            assert resp.tag == TAG_SUCCESS


class TestResetInVariousStates:
    """Tests for RESET behavior in different protocol states."""

    def test_reset_in_ready_state(self, bolt_server):
        """RESET in Ready state returns SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_reset())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

    def test_reset_in_streaming_state(self, bolt_server):
        """RESET in Streaming state returns SUCCESS and discards result."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_run("RETURN 1 AS n"))
            conn.recv()  # Streaming state now

            conn.send(msg_reset())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

            # Back in Ready
            _, recs, s = conn.run_pull("RETURN 2 AS m")
            assert s.tag == TAG_SUCCESS

    def test_reset_in_tx_ready_state(self, bolt_server):
        """RESET in TxReady state returns SUCCESS."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            conn.send(msg_begin())
            conn.recv()  # TxReady

            conn.send(msg_reset())
            resp = conn.recv()
            assert resp.tag == TAG_SUCCESS

            # Back in Ready (not TxReady)
            _, recs, s = conn.run_pull("RETURN 1 AS n")
            assert s.tag == TAG_SUCCESS
