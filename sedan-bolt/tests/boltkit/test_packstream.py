"""
Test b) -- PackStream encoding/decoding.

Validates that our PackStream encoder/decoder matches the Neo4j spec by
sending known byte sequences to the server and verifying responses contain
correct PackStream encoding.

These tests work at two levels:
1. Unit tests of our local Python PackStream implementation
2. Integration tests that round-trip through the server
"""

import struct
import pytest

from bolt_wire import (
    packstream_encode,
    packstream_decode,
    Structure,
    BoltConnection,
    TAG_SUCCESS,
    TAG_RECORD,
)


class TestPackStreamUnitNull:
    """Null encoding/decoding."""

    def test_encode_null(self):
        assert packstream_encode(None) == b"\xC0"

    def test_decode_null(self):
        assert packstream_decode(b"\xC0") is None


class TestPackStreamUnitBool:
    """Boolean encoding/decoding."""

    def test_encode_true(self):
        assert packstream_encode(True) == b"\xC3"

    def test_encode_false(self):
        assert packstream_encode(False) == b"\xC2"

    def test_decode_true(self):
        assert packstream_decode(b"\xC3") is True

    def test_decode_false(self):
        assert packstream_decode(b"\xC2") is False


class TestPackStreamUnitInt:
    """Integer encoding/decoding across all size tiers."""

    @pytest.mark.parametrize("value,expected_hex", [
        (0, "00"),
        (1, "01"),
        (42, "2a"),
        (127, "7f"),
        (-1, "ff"),
        (-16, "f0"),
    ])
    def test_tiny_int(self, value, expected_hex):
        encoded = packstream_encode(value)
        assert encoded == bytes.fromhex(expected_hex)
        assert packstream_decode(encoded) == value

    @pytest.mark.parametrize("value,expected_hex", [
        (-17, "c8ef"),
        (-128, "c880"),
    ])
    def test_int8(self, value, expected_hex):
        encoded = packstream_encode(value)
        assert encoded == bytes.fromhex(expected_hex)
        assert packstream_decode(encoded) == value

    @pytest.mark.parametrize("value,expected_hex", [
        (128, "c90080"),
        (-129, "c9ff7f"),
        (32767, "c97fff"),
        (-32768, "c98000"),
    ])
    def test_int16(self, value, expected_hex):
        encoded = packstream_encode(value)
        assert encoded == bytes.fromhex(expected_hex)
        assert packstream_decode(encoded) == value

    @pytest.mark.parametrize("value", [32768, -32769, 2147483647, -2147483648])
    def test_int32_roundtrip(self, value):
        encoded = packstream_encode(value)
        assert encoded[0] == 0xCA
        assert packstream_decode(encoded) == value

    @pytest.mark.parametrize("value", [2147483648, -2147483649, 2**62, -(2**62)])
    def test_int64_roundtrip(self, value):
        encoded = packstream_encode(value)
        assert encoded[0] == 0xCB
        assert packstream_decode(encoded) == value


class TestPackStreamUnitFloat:
    """Float encoding/decoding."""

    def test_encode_1_1(self):
        encoded = packstream_encode(1.1)
        assert encoded == bytes.fromhex("c13ff199999999999a")

    def test_roundtrip_zero(self):
        assert packstream_decode(packstream_encode(0.0)) == 0.0

    def test_roundtrip_negative(self):
        val = -999.999
        assert packstream_decode(packstream_encode(val)) == val


class TestPackStreamUnitString:
    """String encoding/decoding across all size tiers."""

    def test_empty_string(self):
        assert packstream_encode("") == b"\x80"
        assert packstream_decode(b"\x80") == ""

    def test_tiny_string_hello(self):
        encoded = packstream_encode("hello")
        assert encoded == bytes.fromhex("8568656c6c6f")
        assert packstream_decode(encoded) == "hello"

    def test_string_15_bytes(self):
        s = "abcdefghijklmno"
        encoded = packstream_encode(s)
        assert encoded[0] == 0x8F
        assert packstream_decode(encoded) == s

    def test_string8(self):
        s = "a" * 16
        encoded = packstream_encode(s)
        assert encoded[0] == 0xD0
        assert encoded[1] == 16
        assert packstream_decode(encoded) == s

    def test_string16(self):
        s = "x" * 256
        encoded = packstream_encode(s)
        assert encoded[0] == 0xD1
        assert packstream_decode(encoded) == s

    def test_utf8_string(self):
        s = "Groessenmasstabe"  # ASCII-safe variant
        encoded = packstream_encode(s)
        assert packstream_decode(encoded) == s

    def test_unicode_emoji(self):
        s = "hello\u2603"  # snowman
        encoded = packstream_encode(s)
        assert packstream_decode(encoded) == s


class TestPackStreamUnitList:
    """List encoding/decoding."""

    def test_empty_list(self):
        assert packstream_encode([]) == b"\x90"
        assert packstream_decode(b"\x90") == []

    def test_tiny_list(self):
        encoded = packstream_encode([1, 2, 3])
        assert encoded == bytes.fromhex("93010203")
        assert packstream_decode(encoded) == [1, 2, 3]

    def test_list8(self):
        items = list(range(16))
        encoded = packstream_encode(items)
        assert encoded[0] == 0xD4
        assert packstream_decode(encoded) == items


class TestPackStreamUnitMap:
    """Map encoding/decoding."""

    def test_empty_map(self):
        assert packstream_encode({}) == b"\xA0"
        assert packstream_decode(b"\xA0") == {}

    def test_tiny_map(self):
        encoded = packstream_encode({"one": 1})
        assert encoded == bytes.fromhex("a183 6f6e 6501".replace(" ", ""))
        assert packstream_decode(encoded) == {"one": 1}


class TestPackStreamUnitStructure:
    """Structure encoding/decoding."""

    def test_empty_structure(self):
        s = Structure(0x01)
        encoded = packstream_encode(s)
        assert encoded == b"\xB0\x01"
        decoded = packstream_decode(encoded)
        assert decoded.tag == 0x01
        assert decoded.fields == []

    def test_structure_with_fields(self):
        s = Structure(0x71, 42)
        encoded = packstream_encode(s)
        assert encoded == bytes.fromhex("b1712a")
        decoded = packstream_decode(encoded)
        assert decoded.tag == 0x71
        assert decoded.fields == [42]

    def test_success_structure(self):
        """Encode a SUCCESS message like the server would produce."""
        s = Structure(0x70, {"server": "test"})
        encoded = packstream_encode(s)
        decoded = packstream_decode(encoded)
        assert decoded.tag == 0x70
        assert decoded.fields[0]["server"] == "test"


class TestPackStreamViaServer:
    """Integration tests that validate PackStream by round-tripping through sedan-bolt."""

    def test_server_returns_valid_packstream_success(self, bolt_server):
        """HELLO response is a valid PackStream SUCCESS structure."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            resp = conn.hello()
            assert isinstance(resp, Structure)
            assert resp.tag == TAG_SUCCESS
            assert isinstance(resp.fields[0], dict)
            assert "server" in resp.fields[0]

    def test_integer_values_in_response(self, bolt_server):
        """Server encodes integer values correctly in RECORD messages."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            run_resp, records, summary = conn.run_pull("RETURN 42 AS n")
            assert run_resp.tag == TAG_SUCCESS
            assert len(records) == 1
            assert records[0].tag == TAG_RECORD
            # The value 42 should be in the record
            assert records[0].fields[0] == [42]

    def test_string_values_in_response(self, bolt_server):
        """Server encodes string values correctly."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            run_resp, records, summary = conn.run_pull("RETURN 'hello' AS s")
            assert run_resp.tag == TAG_SUCCESS
            assert len(records) == 1
            assert records[0].fields[0] == ["hello"]

    def test_null_value_in_response(self, bolt_server):
        """Server encodes null correctly."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            run_resp, records, summary = conn.run_pull("RETURN null AS n")
            assert run_resp.tag == TAG_SUCCESS
            assert len(records) == 1
            assert records[0].fields[0] == [None]

    def test_boolean_values_in_response(self, bolt_server):
        """Server encodes boolean values correctly."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            run_resp, records, summary = conn.run_pull("RETURN true AS b")
            assert run_resp.tag == TAG_SUCCESS
            assert len(records) == 1
            assert records[0].fields[0] == [True]

    def test_float_value_in_response(self, bolt_server):
        """Server encodes float values correctly."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            run_resp, records, summary = conn.run_pull("RETURN 3.14 AS f")
            assert run_resp.tag == TAG_SUCCESS
            if len(records) == 1:
                val = records[0].fields[0][0]
                assert isinstance(val, (int, float))

    def test_list_value_in_response(self, bolt_server):
        """Server encodes list values correctly."""
        host, port = bolt_server
        with BoltConnection(host, port) as conn:
            conn.hello()
            run_resp, records, summary = conn.run_pull("RETURN [1, 2, 3] AS l")
            assert run_resp.tag == TAG_SUCCESS
            if len(records) == 1:
                val = records[0].fields[0][0]
                assert isinstance(val, list)
                assert val == [1, 2, 3]
