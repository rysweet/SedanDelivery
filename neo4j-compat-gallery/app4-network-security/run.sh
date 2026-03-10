#!/usr/bin/env bash
# Network Security Graph Analyzer - run script
# Connects to SedanDelivery Bolt server on localhost:7687

set -e

cd "$(dirname "$0")"

BOLT_URI="${BOLT_URI:-bolt://localhost:7687}"
BOLT_USERNAME="${BOLT_USERNAME:-neo4j}"
BOLT_PASSWORD="${BOLT_PASSWORD:-}"

echo "=========================================="
echo "  Network Security Graph Analyzer"
echo "  App 4 - Neo4j Compatibility Gallery"
echo "=========================================="
if [[ -n "$BOLT_PASSWORD" ]]; then
    echo "[*] Bolt auth enabled for user: $BOLT_USERNAME"
else
    echo "[*] Bolt auth disabled (BOLT_PASSWORD unset)"
fi

# Install dependencies
echo ""
echo "[*] Installing Python dependencies..."
pip install -q -r requirements.txt

# Verify Bolt server is reachable
echo "[*] Checking SedanDelivery Bolt server on localhost:7687..."
if BOLT_URI="$BOLT_URI" BOLT_USERNAME="$BOLT_USERNAME" BOLT_PASSWORD="$BOLT_PASSWORD" python3 - <<'PY' 2>/dev/null
import os

from neo4j import GraphDatabase

uri = os.environ.get("BOLT_URI", "bolt://localhost:7687")
password = os.environ.get("BOLT_PASSWORD", "")
auth = None if password == "" else (os.environ.get("BOLT_USERNAME", "neo4j"), password)
driver = GraphDatabase.driver(uri, auth=auth)
driver.verify_connectivity()
driver.close()
print("[+] Bolt server is reachable")
PY
then
    echo "[+] Connection verified"
else
    echo "[!] WARNING: Cannot reach Bolt server at $BOLT_URI"
    echo "    Make sure SedanDelivery is running:"
    echo "      cd /home/azureuser/src/SedanDelivery"
    echo "      cargo run --bin sedandelivery-bolt"
    echo ""
    echo "    Proceeding anyway (will retry on startup)..."
fi

# Load data and start Flask
echo ""
echo "[*] Starting Flask app on http://localhost:5002"
echo "[*] Network data will be loaded on startup"
echo "=========================================="
echo ""

export BOLT_URI
export BOLT_USERNAME
export BOLT_PASSWORD
python3 app.py
