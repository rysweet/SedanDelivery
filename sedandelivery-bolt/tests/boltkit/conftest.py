"""
Pytest fixtures for sedandelivery-bolt wire-level protocol tests.

Starts sedandelivery-bolt on a random port before the test session and tears it down
after all tests complete.  A health-check wrapper automatically restarts the
server if it becomes unresponsive.
"""

import os
import socket
import subprocess
import time

import pytest

SEDAN_BOLT_BIN = os.path.join(
    os.path.dirname(__file__), "..", "..", "..", "target", "debug", "sedandelivery-bolt"
)


def _find_free_port():
    """Return a free TCP port on localhost."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class BoltServerManager:
    """Manages the sedandelivery-bolt server process with automatic restart."""

    def __init__(self):
        self.proc = None
        self.host = "127.0.0.1"
        self.port = None

    def start(self):
        """Start a fresh server on a new port."""
        if self.proc is not None:
            self.stop()

        self.port = _find_free_port()
        env = os.environ.copy()
        env["BOLT_BIND_ADDR"] = f"{self.host}:{self.port}"
        env["RUST_LOG"] = "warn"
        env.pop("BOLT_PASSWORD", None)

        self.proc = subprocess.Popen(
            [os.path.abspath(SEDAN_BOLT_BIN)],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                with socket.create_connection(
                    (self.host, self.port), timeout=0.5
                ):
                    break
            except (ConnectionRefusedError, OSError):
                time.sleep(0.1)
                if self.proc.poll() is not None:
                    stdout = self.proc.stdout.read().decode()
                    stderr = self.proc.stderr.read().decode()
                    raise RuntimeError(
                        f"sedandelivery-bolt exited early (rc={self.proc.returncode})\n"
                        f"stdout: {stdout}\nstderr: {stderr}"
                    )
        else:
            self.stop()
            raise RuntimeError("sedandelivery-bolt did not start within 10 seconds")

    def stop(self):
        """Stop the server."""
        if self.proc is None:
            return
        self.proc.terminate()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        self.proc = None

    def is_healthy(self):
        """Check if the server responds to a TCP connection."""
        if self.proc is None or self.proc.poll() is not None:
            return False
        try:
            with socket.create_connection(
                (self.host, self.port), timeout=2.0
            ) as s:
                # Send a valid handshake and check for response
                handshake = (
                    b"\x60\x60\xb0\x17"
                    + bytes([0x00, 0x04, 0x04, 0x04])
                    + bytes(12)
                )
                s.sendall(handshake)
                s.settimeout(2.0)
                data = s.recv(4)
                return len(data) == 4
        except (ConnectionError, OSError, socket.timeout, TimeoutError):
            return False

    def ensure_healthy(self):
        """Restart the server if it's not responsive."""
        if not self.is_healthy():
            self.start()

    @property
    def address(self):
        return (self.host, self.port)


@pytest.fixture(scope="session")
def _server_manager():
    """Session-scoped server manager."""
    mgr = BoltServerManager()
    mgr.start()
    yield mgr
    mgr.stop()


@pytest.fixture(autouse=True)
def bolt_server(_server_manager):
    """Per-test fixture that ensures the server is healthy.

    If the server became unresponsive (e.g. from stress testing),
    it is automatically restarted.
    """
    _server_manager.ensure_healthy()
    yield _server_manager.address
