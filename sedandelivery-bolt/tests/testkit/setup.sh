#!/usr/bin/env bash
# setup.sh - One-time setup for running testkit conformance tests
#
# This script clones the required repositories and installs Python dependencies.

set -euo pipefail

echo "=== TestKit Setup for sedandelivery-bolt ==="
echo ""

# 1. Clone testkit
if [[ -d /tmp/testkit ]]; then
    echo "[OK] testkit already cloned at /tmp/testkit"
else
    echo "[..] Cloning neo4j-drivers/testkit..."
    git clone https://github.com/neo4j-drivers/testkit /tmp/testkit
    echo "[OK] testkit cloned"
fi

# 2. Clone neo4j-python-driver (for testkitbackend)
if [[ -d /tmp/neo4j-python-driver ]]; then
    echo "[OK] neo4j-python-driver already cloned at /tmp/neo4j-python-driver"
else
    echo "[..] Cloning neo4j/neo4j-python-driver..."
    git clone --depth=1 https://github.com/neo4j/neo4j-python-driver /tmp/neo4j-python-driver
    echo "[OK] neo4j-python-driver cloned"
fi

# 3. Install Python dependencies
echo "[..] Installing Python dependencies..."

# Detect pip tool
if command -v uv &>/dev/null; then
    PIP="uv pip install"
elif command -v pip3 &>/dev/null; then
    PIP="pip3 install"
elif python3 -m pip --version &>/dev/null; then
    PIP="python3 -m pip install"
else
    echo "[!!] No pip found. Please install uv, pip3, or python3-pip."
    exit 1
fi

$PIP neo4j ifaddr freezegun pytz lark 2>&1 | tail -3
echo "[OK] Python packages installed"

# 4. Install testkit requirements
$PIP -r /tmp/testkit/requirements.txt 2>&1 | tail -3
echo "[OK] testkit requirements installed"

# 5. Build sedandelivery-bolt
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SD2_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

echo "[..] Building sedandelivery-bolt..."
(cd "$SD2_ROOT" && cargo build -p sedandelivery-bolt 2>&1 | tail -3)
echo "[OK] sedandelivery-bolt built"

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Run tests with:"
echo "  $SCRIPT_DIR/run_testkit.sh                  # All integration tests"
echo "  $SCRIPT_DIR/run_testkit.sh --passing-only   # Only known-passing tests"
echo "  $SCRIPT_DIR/run_testkit.sh session_run      # Specific test suite"
