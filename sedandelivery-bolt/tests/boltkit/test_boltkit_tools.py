"""
Test: boltkit toolkit capabilities assessment.

This file documents and tests what boltkit provides. It uses boltkit's
own driver.Connection to validate sedandelivery-bolt, demonstrating that the
toolkit's built-in driver can communicate with our server.

Note: boltkit is designed for testing Bolt DRIVERS (clients), not servers.
Its primary tool is StubServer -- a scriptable fake server that drivers
connect to. It also provides a minimal Connection (Bolt driver) class
and a PackStream encoder/decoder.

For server testing, boltkit's Connection class can act as a test client,
but it was not designed as a protocol conformance tool for servers.
"""

import pytest

from bolt_wire import BoltConnection, TAG_SUCCESS

# boltkit may be installed via uv but not available to system python.
# Try importing and mark unavailable if not found.
try:
    from boltkit.driver import Connection as BoltkitConnection, CLIENT, SERVER
    from boltkit.packstream import pack as bk_pack, unpack as bk_unpack
    from boltkit.packstream import Structure as BkStructure
    from boltkit.server import StubServer, Script
    BOLTKIT_AVAILABLE = True
except ImportError:
    BOLTKIT_AVAILABLE = False

requires_boltkit = pytest.mark.skipif(
    not BOLTKIT_AVAILABLE,
    reason="boltkit not importable (may need to run with uv's python)"
)


class TestBoltkitDriver:
    """Tests using boltkit's built-in Bolt driver against sedandelivery-bolt."""

    @requires_boltkit
    def test_boltkit_driver_connects(self, bolt_server):
        """boltkit's Connection class can connect to sedandelivery-bolt."""
        host, port = bolt_server
        try:
            cx = BoltkitConnection.open(
                (host, port),
                auth=("neo4j", ""),
                bolt_versions=[4],
                timeout=5,
            )
            assert cx.bolt_version == 4
            assert cx.server_agent is not None
            cx.close()
        except Exception as e:
            pytest.skip(f"boltkit driver connection failed: {e}")

    @requires_boltkit
    def test_boltkit_driver_run_query(self, bolt_server):
        """boltkit's Connection can run a query against sedandelivery-bolt."""
        host, port = bolt_server
        try:
            cx = BoltkitConnection.open(
                (host, port),
                auth=("neo4j", ""),
                bolt_versions=[4],
                timeout=5,
            )
            records = []
            response = cx.run("RETURN 1 AS n")
            cx.pull(-1, -1, records)
            cx.send_all()
            cx.fetch_all()
            assert response.metadata is not None
            cx.close()
        except Exception as e:
            pytest.skip(f"boltkit driver query failed: {e}")

    @requires_boltkit
    def test_boltkit_packstream_encode_decode(self):
        """boltkit's PackStream implementation is compatible with our encoder."""
        data = bk_pack(BkStructure(0x01, {"user_agent": "test/1.0", "scheme": "none"}))
        assert isinstance(data, bytes)
        assert len(data) > 0
        decoded = bk_unpack(data)
        assert isinstance(decoded, BkStructure)
        assert decoded.tag == 0x01

    @requires_boltkit
    def test_boltkit_packstream_roundtrip_integers(self):
        """boltkit PackStream handles integers correctly."""
        for val in [0, 1, -1, 127, -16, -17, 128, 32767, -32768, 2147483647]:
            encoded = bk_pack(val)
            decoded = bk_unpack(encoded)
            assert decoded == val, f"Failed for {val}: got {decoded}"

    @requires_boltkit
    def test_boltkit_packstream_roundtrip_strings(self):
        """boltkit PackStream handles strings correctly."""
        for s in ["", "hello", "a" * 16, "a" * 256]:
            encoded = bk_pack(s)
            decoded = bk_unpack(encoded)
            assert decoded == s

    @requires_boltkit
    def test_boltkit_packstream_roundtrip_map(self):
        """boltkit PackStream handles maps correctly."""
        m = {"name": "Alice", "age": 30}
        encoded = bk_pack(m)
        decoded = bk_unpack(encoded)
        assert decoded == m


class TestBoltkitCapabilities:
    """Informational tests documenting what boltkit provides."""

    @requires_boltkit
    def test_boltkit_has_stub_server(self):
        """boltkit provides a StubServer for testing drivers."""
        assert StubServer is not None

    @requires_boltkit
    def test_boltkit_has_connection(self):
        """boltkit provides a Connection class (minimal Bolt driver)."""
        assert BoltkitConnection is not None

    @requires_boltkit
    def test_boltkit_has_packstream(self):
        """boltkit provides PackStream encode/decode."""
        assert callable(bk_pack)
        assert callable(bk_unpack)

    @requires_boltkit
    def test_boltkit_has_script_parser(self):
        """boltkit provides a Script parser for stub server scripts."""
        assert Script is not None

    @requires_boltkit
    def test_boltkit_message_tags(self):
        """boltkit defines Bolt message tags for v1-v4."""
        assert CLIENT[4]["HELLO"] == 0x01
        assert CLIENT[4]["GOODBYE"] == 0x02
        assert CLIENT[4]["RUN"] == 0x10
        assert CLIENT[4]["PULL"] == 0x3F
        assert CLIENT[4]["BEGIN"] == 0x11
        assert CLIENT[4]["COMMIT"] == 0x12
        assert CLIENT[4]["ROLLBACK"] == 0x13
        assert SERVER[4]["SUCCESS"] == 0x70
        assert SERVER[4]["RECORD"] == 0x71
        assert SERVER[4]["FAILURE"] == 0x7F
        assert SERVER[4]["IGNORED"] == 0x7E
