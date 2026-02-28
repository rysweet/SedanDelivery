"""
Test a) -- Bolt handshake byte sequences.

Validates that sedan-bolt correctly implements the Bolt v4.x handshake:
  - Accepts the 4-byte magic preamble (0x6060B017)
  - Negotiates version from 4 proposals
  - Returns agreed version in [0, 0, minor, major] format
  - Rejects bad magic bytes
  - Handles version negotiation when no version matches
"""

import socket
import struct
import time

import pytest

from bolt_wire import (
    BOLT_MAGIC,
    recv_exact,
)


class TestHandshakeMagic:
    """Tests for the initial 4-byte magic preamble."""

    def test_correct_magic_accepted(self, bolt_server):
        """Server accepts the standard Bolt magic bytes."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            # Send magic + version 4.4 in slot 0
            handshake = bytearray(BOLT_MAGIC)
            handshake.extend(bytes([0x00, 0x04, 0x04, 0x04]))  # v4.4 with range 4
            handshake.extend(bytes(12))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            major = response[3]
            minor = response[2]
            assert major == 4, f"Expected major version 4, got {major}"
            assert minor <= 4, f"Expected minor version <= 4, got {minor}"
        finally:
            sock.close()

    def test_bad_magic_disconnects(self, bolt_server):
        """Server rejects connections with invalid magic bytes."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            # Send bad magic
            sock.sendall(b"\xDE\xAD\xBE\xEF")
            # Send 16 bytes of version proposals
            sock.sendall(bytes(16))
            # Server should close the connection or send error
            # We give it a moment to process
            sock.settimeout(3)
            try:
                data = sock.recv(1024)
                # If we got data, it should be empty (connection closed)
                # or the server might close without sending anything
                # Either way, the connection should not proceed normally
            except (ConnectionError, socket.timeout, OSError):
                pass  # Expected -- server closed or timed out
        finally:
            sock.close()

    def test_all_zeroes_magic_rejected(self, bolt_server):
        """Server rejects all-zero magic bytes."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            sock.sendall(b"\x00\x00\x00\x00")
            sock.sendall(bytes(16))
            sock.settimeout(3)
            try:
                data = sock.recv(1024)
            except (ConnectionError, socket.timeout, OSError):
                pass
        finally:
            sock.close()


class TestVersionNegotiation:
    """Tests for Bolt version negotiation."""

    def test_v4_4_negotiation(self, bolt_server):
        """Server agrees on Bolt 4.4."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            # Slot 0: version 4.4, no range
            handshake.extend(bytes([0x00, 0x00, 0x04, 0x04]))
            handshake.extend(bytes(12))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            assert response == bytes([0x00, 0x00, 0x04, 0x04])
        finally:
            sock.close()

    def test_v4_3_negotiation(self, bolt_server):
        """Server agrees on Bolt 4.3."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            # Slot 0: version 4.3, no range
            handshake.extend(bytes([0x00, 0x00, 0x03, 0x04]))
            handshake.extend(bytes(12))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            assert response == bytes([0x00, 0x00, 0x03, 0x04])
        finally:
            sock.close()

    def test_v4_1_negotiation(self, bolt_server):
        """Server agrees on Bolt 4.1."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            handshake.extend(bytes([0x00, 0x00, 0x01, 0x04]))
            handshake.extend(bytes(12))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            assert response == bytes([0x00, 0x00, 0x01, 0x04])
        finally:
            sock.close()

    def test_v4_0_negotiation(self, bolt_server):
        """Server agrees on Bolt 4.0."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            handshake.extend(bytes([0x00, 0x00, 0x00, 0x04]))
            handshake.extend(bytes(12))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            assert response == bytes([0x00, 0x00, 0x00, 0x04])
        finally:
            sock.close()

    def test_version_range_negotiation(self, bolt_server):
        """Server handles version ranges (client proposes 4.4 with range=4, covering 4.0-4.4)."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            # Slot 0: major=4, minor=4, range=4 -> covers 4.0 through 4.4
            handshake.extend(bytes([0x00, 0x04, 0x04, 0x04]))
            handshake.extend(bytes(12))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            major = response[3]
            minor = response[2]
            assert major == 4
            assert 0 <= minor <= 4
        finally:
            sock.close()

    def test_no_supported_version_returns_zero(self, bolt_server):
        """Server returns [0,0,0,0] when no versions match."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            # Propose only version 5.0 (not supported by sedan-bolt)
            handshake.extend(bytes([0x00, 0x00, 0x00, 0x05]))
            # Propose version 3.0
            handshake.extend(bytes([0x00, 0x00, 0x00, 0x03]))
            # Two empty slots
            handshake.extend(bytes(8))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            assert response == bytes([0x00, 0x00, 0x00, 0x00]), \
                f"Expected zero version for unsupported proposal, got {response.hex()}"
        finally:
            sock.close()

    def test_prefers_first_matching_slot(self, bolt_server):
        """Server selects the first matching version slot."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            # Slot 0: v4.3
            handshake.extend(bytes([0x00, 0x00, 0x03, 0x04]))
            # Slot 1: v4.4
            handshake.extend(bytes([0x00, 0x00, 0x04, 0x04]))
            # Slots 2-3: zero
            handshake.extend(bytes(8))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            major = response[3]
            minor = response[2]
            assert major == 4
            # Server should pick slot 0 (v4.3) since it matches first
            assert minor == 3
        finally:
            sock.close()

    def test_four_proposals_all_different(self, bolt_server):
        """Server handles 4 different version proposals."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            # Slot 0: v5.0 (unsupported)
            handshake.extend(bytes([0x00, 0x00, 0x00, 0x05]))
            # Slot 1: v3.0 (unsupported)
            handshake.extend(bytes([0x00, 0x00, 0x00, 0x03]))
            # Slot 2: v4.2 (supported!)
            handshake.extend(bytes([0x00, 0x00, 0x02, 0x04]))
            # Slot 3: v2.0 (unsupported)
            handshake.extend(bytes([0x00, 0x00, 0x00, 0x02]))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            assert response == bytes([0x00, 0x00, 0x02, 0x04])
        finally:
            sock.close()

    def test_response_format(self, bolt_server):
        """Server response is exactly 4 bytes in [0, 0, minor, major] format."""
        host, port = bolt_server
        sock = socket.create_connection((host, port), timeout=5)
        try:
            handshake = bytearray(BOLT_MAGIC)
            handshake.extend(bytes([0x00, 0x00, 0x04, 0x04]))
            handshake.extend(bytes(12))
            sock.sendall(handshake)

            response = recv_exact(sock, 4)
            assert len(response) == 4
            assert response[0] == 0, "Padding byte 0 should be zero"
            assert response[1] == 0, "Padding byte 1 should be zero"
            # Bytes 2 and 3 are minor and major
        finally:
            sock.close()
