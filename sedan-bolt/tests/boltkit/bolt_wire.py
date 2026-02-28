"""
Low-level Bolt wire protocol helpers for testing.

This module provides raw TCP socket operations for the Bolt v4.x protocol,
including:
  - PackStream encoding/decoding (matching the Neo4j spec exactly)
  - Bolt chunked message framing
  - Handshake helpers
  - Message constructors for all Bolt 4.x client/server message types

These are intentionally independent of any driver library so that they can
validate the server's wire-level conformance.
"""

import struct
import socket as _socket


# ============================================================================
# Constants
# ============================================================================

BOLT_MAGIC = b"\x60\x60\xb0\x17"

# Bolt 4.x message tags
TAG_HELLO    = 0x01
TAG_GOODBYE  = 0x02
TAG_RESET    = 0x0F
TAG_RUN      = 0x10
TAG_BEGIN    = 0x11
TAG_COMMIT   = 0x12
TAG_ROLLBACK = 0x13
TAG_DISCARD  = 0x2F
TAG_PULL     = 0x3F
TAG_LOGON    = 0x6A

TAG_SUCCESS  = 0x70
TAG_RECORD   = 0x71
TAG_IGNORED  = 0x7E
TAG_FAILURE  = 0x7F

TAG_NAMES = {
    TAG_SUCCESS: "SUCCESS",
    TAG_RECORD: "RECORD",
    TAG_IGNORED: "IGNORED",
    TAG_FAILURE: "FAILURE",
    TAG_HELLO: "HELLO",
    TAG_GOODBYE: "GOODBYE",
    TAG_RESET: "RESET",
    TAG_RUN: "RUN",
    TAG_BEGIN: "BEGIN",
    TAG_COMMIT: "COMMIT",
    TAG_ROLLBACK: "ROLLBACK",
    TAG_DISCARD: "DISCARD",
    TAG_PULL: "PULL",
    TAG_LOGON: "LOGON",
}


# ============================================================================
# PackStream encoding
# ============================================================================

def packstream_encode(value):
    """Encode a Python value into PackStream bytes."""
    buf = bytearray()
    _encode_value(value, buf)
    return bytes(buf)


def _encode_value(value, buf):
    if value is None:
        buf.append(0xC0)
    elif value is True:
        buf.append(0xC3)
    elif value is False:
        buf.append(0xC2)
    elif isinstance(value, int) and not isinstance(value, bool):
        _encode_int(value, buf)
    elif isinstance(value, float):
        buf.append(0xC1)
        buf.extend(struct.pack(">d", value))
    elif isinstance(value, str):
        _encode_string(value, buf)
    elif isinstance(value, bytes):
        _encode_bytes(value, buf)
    elif isinstance(value, list):
        _encode_list(value, buf)
    elif isinstance(value, dict):
        _encode_map(value, buf)
    elif isinstance(value, Structure):
        _encode_structure(value, buf)
    else:
        raise ValueError(f"Cannot encode {type(value)}: {value!r}")


def _encode_int(n, buf):
    if -16 <= n <= 127:
        buf.extend(struct.pack(">b", n))
    elif -128 <= n <= 127:
        buf.append(0xC8)
        buf.extend(struct.pack(">b", n))
    elif -32768 <= n <= 32767:
        buf.append(0xC9)
        buf.extend(struct.pack(">h", n))
    elif -2147483648 <= n <= 2147483647:
        buf.append(0xCA)
        buf.extend(struct.pack(">i", n))
    else:
        buf.append(0xCB)
        buf.extend(struct.pack(">q", n))


def _encode_string(s, buf):
    encoded = s.encode("utf-8")
    length = len(encoded)
    if length <= 15:
        buf.append(0x80 | length)
    elif length <= 255:
        buf.append(0xD0)
        buf.append(length)
    elif length <= 65535:
        buf.append(0xD1)
        buf.extend(struct.pack(">H", length))
    else:
        buf.append(0xD2)
        buf.extend(struct.pack(">I", length))
    buf.extend(encoded)


def _encode_bytes(data, buf):
    length = len(data)
    if length <= 255:
        buf.append(0xCC)
        buf.append(length)
    elif length <= 65535:
        buf.append(0xCD)
        buf.extend(struct.pack(">H", length))
    else:
        buf.append(0xCE)
        buf.extend(struct.pack(">I", length))
    buf.extend(data)


def _encode_list(items, buf):
    length = len(items)
    if length <= 15:
        buf.append(0x90 | length)
    elif length <= 255:
        buf.append(0xD4)
        buf.append(length)
    elif length <= 65535:
        buf.append(0xD5)
        buf.extend(struct.pack(">H", length))
    else:
        buf.append(0xD6)
        buf.extend(struct.pack(">I", length))
    for item in items:
        _encode_value(item, buf)


def _encode_map(entries, buf):
    length = len(entries)
    if length <= 15:
        buf.append(0xA0 | length)
    elif length <= 255:
        buf.append(0xD8)
        buf.append(length)
    elif length <= 65535:
        buf.append(0xD9)
        buf.extend(struct.pack(">H", length))
    else:
        buf.append(0xDA)
        buf.extend(struct.pack(">I", length))
    for key, val in entries.items():
        _encode_string(key, buf)
        _encode_value(val, buf)


def _encode_structure(s, buf):
    nfields = len(s.fields)
    assert nfields <= 15, f"Structure has {nfields} fields (max 15 for tiny struct)"
    buf.append(0xB0 | nfields)
    buf.append(s.tag)
    for field in s.fields:
        _encode_value(field, buf)


# ============================================================================
# PackStream decoding
# ============================================================================

class Structure:
    """A PackStream structure with a tag byte and fields."""

    def __init__(self, tag, *fields):
        self.tag = tag
        self.fields = list(fields)

    def __repr__(self):
        name = TAG_NAMES.get(self.tag, f"0x{self.tag:02X}")
        return f"Structure({name}, {', '.join(repr(f) for f in self.fields)})"

    def __eq__(self, other):
        return (
            isinstance(other, Structure)
            and self.tag == other.tag
            and self.fields == other.fields
        )


class PackStreamDecoder:
    """Stateful decoder that reads PackStream values from a byte buffer."""

    def __init__(self, data):
        self.data = data
        self.offset = 0

    def _read(self, n):
        if self.offset + n > len(self.data):
            raise ValueError(
                f"Unexpected EOF at offset {self.offset}, need {n} bytes, "
                f"have {len(self.data) - self.offset}"
            )
        result = self.data[self.offset : self.offset + n]
        self.offset += n
        return result

    def _read_u8(self):
        return self._read(1)[0]

    def _read_i8(self):
        return struct.unpack(">b", self._read(1))[0]

    def _read_i16(self):
        return struct.unpack(">h", self._read(2))[0]

    def _read_i32(self):
        return struct.unpack(">i", self._read(4))[0]

    def _read_i64(self):
        return struct.unpack(">q", self._read(8))[0]

    def _read_u16(self):
        return struct.unpack(">H", self._read(2))[0]

    def _read_u32(self):
        return struct.unpack(">I", self._read(4))[0]

    def _read_f64(self):
        return struct.unpack(">d", self._read(8))[0]

    def decode(self):
        marker = self._read_u8()

        # Null
        if marker == 0xC0:
            return None
        # Bool
        if marker == 0xC2:
            return False
        if marker == 0xC3:
            return True
        # Float64
        if marker == 0xC1:
            return self._read_f64()
        # Int8
        if marker == 0xC8:
            return self._read_i8()
        # Int16
        if marker == 0xC9:
            return self._read_i16()
        # Int32
        if marker == 0xCA:
            return self._read_i32()
        # Int64
        if marker == 0xCB:
            return self._read_i64()
        # Bytes
        if marker == 0xCC:
            return self._read(self._read_u8())
        if marker == 0xCD:
            return self._read(self._read_u16())
        if marker == 0xCE:
            return self._read(self._read_u32())
        # String
        if 0x80 <= marker <= 0x8F:
            return self._read(marker & 0x0F).decode("utf-8")
        if marker == 0xD0:
            return self._read(self._read_u8()).decode("utf-8")
        if marker == 0xD1:
            return self._read(self._read_u16()).decode("utf-8")
        if marker == 0xD2:
            return self._read(self._read_u32()).decode("utf-8")
        # List
        if 0x90 <= marker <= 0x9F:
            return [self.decode() for _ in range(marker & 0x0F)]
        if marker == 0xD4:
            return [self.decode() for _ in range(self._read_u8())]
        if marker == 0xD5:
            return [self.decode() for _ in range(self._read_u16())]
        if marker == 0xD6:
            return [self.decode() for _ in range(self._read_u32())]
        # Map
        if 0xA0 <= marker <= 0xAF:
            return {self.decode(): self.decode() for _ in range(marker & 0x0F)}
        if marker == 0xD8:
            return {self.decode(): self.decode() for _ in range(self._read_u8())}
        if marker == 0xD9:
            return {self.decode(): self.decode() for _ in range(self._read_u16())}
        if marker == 0xDA:
            return {self.decode(): self.decode() for _ in range(self._read_u32())}
        # Structure
        if 0xB0 <= marker <= 0xBF:
            nfields = marker & 0x0F
            tag = self._read_u8()
            fields = [self.decode() for _ in range(nfields)]
            return Structure(tag, *fields)

        # TinyInt positive (0x00..0x7F)
        if marker <= 0x7F:
            return marker
        # TinyInt negative (0xF0..0xFF)
        if marker >= 0xF0:
            return marker - 0x100

        raise ValueError(f"Unknown marker byte 0x{marker:02X}")


def packstream_decode(data):
    """Decode one PackStream value from bytes. Returns the value."""
    decoder = PackStreamDecoder(data)
    return decoder.decode()


# ============================================================================
# Bolt chunked message framing
# ============================================================================

def chunk_encode(message_bytes):
    """Encode a message into Bolt chunked wire format.

    Returns the chunked bytes: one or more (len_header + data) pairs followed
    by a zero-length terminator (0x00 0x00).
    """
    MAX_CHUNK = 16384
    out = bytearray()
    if not message_bytes:
        out.extend(b"\x00\x00")
        return bytes(out)
    for i in range(0, len(message_bytes), MAX_CHUNK):
        chunk = message_bytes[i : i + MAX_CHUNK]
        out.extend(struct.pack(">H", len(chunk)))
        out.extend(chunk)
    out.extend(b"\x00\x00")
    return bytes(out)


def recv_exact(sock, n):
    """Receive exactly n bytes from sock, raising on premature close."""
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError(
                f"Connection closed after receiving {len(buf)} of {n} bytes"
            )
        buf.extend(chunk)
    return bytes(buf)


def recv_message(sock):
    """Read one chunked Bolt message from a socket.

    Returns the reassembled message bytes (without chunk framing).
    """
    message = bytearray()
    while True:
        header = recv_exact(sock, 2)
        chunk_len = struct.unpack(">H", header)[0]
        if chunk_len == 0:
            break
        data = recv_exact(sock, chunk_len)
        message.extend(data)
    return bytes(message)


def recv_structure(sock):
    """Read one chunked Bolt message and decode it as a PackStream Structure."""
    data = recv_message(sock)
    return packstream_decode(data)


# ============================================================================
# High-level message helpers
# ============================================================================

def encode_message(structure):
    """Encode a Structure into chunked wire bytes ready to send."""
    return chunk_encode(packstream_encode(structure))


def msg_hello(user_agent="boltkit-test/1.0", scheme="none", **extra):
    """Build a HELLO message."""
    fields = {"user_agent": user_agent, "scheme": scheme}
    fields.update(extra)
    return Structure(TAG_HELLO, fields)


def msg_goodbye():
    """Build a GOODBYE message."""
    return Structure(TAG_GOODBYE)


def msg_reset():
    """Build a RESET message."""
    return Structure(TAG_RESET)


def msg_run(query, params=None, extra=None):
    """Build a RUN message."""
    return Structure(TAG_RUN, query, params or {}, extra or {})


def msg_pull(n=-1):
    """Build a PULL message."""
    return Structure(TAG_PULL, {"n": n})


def msg_discard(n=-1):
    """Build a DISCARD message."""
    return Structure(TAG_DISCARD, {"n": n})


def msg_begin(extra=None):
    """Build a BEGIN message."""
    return Structure(TAG_BEGIN, extra or {})


def msg_commit():
    """Build a COMMIT message."""
    return Structure(TAG_COMMIT)


def msg_rollback():
    """Build a ROLLBACK message."""
    return Structure(TAG_ROLLBACK)


def msg_logon(scheme="basic", principal="neo4j", credentials=""):
    """Build a LOGON message."""
    return Structure(TAG_LOGON, {
        "scheme": scheme,
        "principal": principal,
        "credentials": credentials,
    })


# ============================================================================
# Connection helper
# ============================================================================

class BoltConnection:
    """A raw Bolt connection for testing.

    Performs the handshake on creation and provides send/recv methods that
    operate at the chunked message level.
    """

    def __init__(self, host, port, bolt_version=(4, 4), timeout=5.0):
        import time as _time
        last_err = None
        for attempt in range(5):
            try:
                self.sock = _socket.create_connection((host, port), timeout=timeout)
                self.sock.settimeout(timeout)
                self.negotiated_version = self._handshake(bolt_version)
                return
            except (OSError, TimeoutError) as e:
                last_err = e
                _time.sleep(0.2 * (attempt + 1))
        raise last_err

    def _handshake(self, version):
        """Perform Bolt handshake. version is (major, minor)."""
        major, minor = version
        # Build the 20-byte handshake: 4 magic + 4x4 version proposals
        # Version proposal format: [padding, range, minor, major]
        # We propose our version in slot 1, rest zeroed
        handshake = bytearray(BOLT_MAGIC)
        # Slot 0: our preferred version with range covering all 4.x
        handshake.extend(bytes([0x00, minor, minor, major]))
        # Slots 1-3: zeroed
        handshake.extend(bytes(12))
        self.sock.sendall(handshake)

        # Read 4-byte response: [0, 0, minor, major]
        response = recv_exact(self.sock, 4)
        resp_major = response[3]
        resp_minor = response[2]
        return (resp_major, resp_minor)

    def send(self, structure):
        """Send a Structure as a chunked Bolt message."""
        self.sock.sendall(encode_message(structure))

    def send_raw(self, data):
        """Send raw bytes (for testing malformed messages)."""
        self.sock.sendall(data)

    def recv(self):
        """Receive one Bolt message as a Structure."""
        return recv_structure(self.sock)

    def recv_raw_message(self):
        """Receive one chunked message as raw bytes (post-dechunk)."""
        return recv_message(self.sock)

    def close(self):
        """Close the underlying socket."""
        try:
            self.sock.shutdown(_socket.SHUT_RDWR)
        except OSError:
            pass
        self.sock.close()

    def hello(self, **kwargs):
        """Send HELLO message and return the server's response."""
        self.send(msg_hello(**kwargs))
        return self.recv()

    def run_pull(self, query, params=None, extra=None, pull_n=-1):
        """Send RUN + PULL and return (run_response, [records...], pull_summary)."""
        self.send(msg_run(query, params, extra))
        run_resp = self.recv()

        self.send(msg_pull(pull_n))
        messages = []
        while True:
            msg = self.recv()
            if msg.tag == TAG_RECORD:
                messages.append(msg)
            else:
                # SUCCESS or FAILURE after records
                return run_resp, messages, msg

    def __enter__(self):
        return self

    def __exit__(self, *args):
        try:
            self.send(msg_goodbye())
        except OSError:
            pass
        self.close()
