#!/usr/bin/env bash
# Network Security Graph Analyzer - run script
# Connects to SedanDelivery Bolt server on localhost:7687

set -e

cd "$(dirname "$0")"

echo "=========================================="
echo "  Network Security Graph Analyzer"
echo "  App 4 - Neo4j Compatibility Gallery"
echo "=========================================="

# Install dependencies
echo ""
echo "[*] Installing Python dependencies..."
pip install -q -r requirements.txt

# Verify Bolt server is reachable
echo "[*] Checking SedanDelivery Bolt server on localhost:7687..."
if python3 -c "
from neo4j import GraphDatabase
d = GraphDatabase.driver('bolt://localhost:7687', auth=None)
d.verify_connectivity()
d.close()
print('[+] Bolt server is reachable')
" 2>/dev/null; then
    echo "[+] Connection verified"
else
    echo "[!] WARNING: Cannot reach Bolt server on localhost:7687"
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

python3 app.py
