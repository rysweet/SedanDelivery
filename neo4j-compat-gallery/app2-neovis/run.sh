#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────
# run.sh — Start the neovis.js MITRE ATT&CK dashboard
#
# Prerequisites:
#   - SedanDelivery Bolt server running on localhost:7687
#   - MITRE ATT&CK dataset loaded
#   - Python 3.11+ with pip
# ─────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOLT_URI="${BOLT_URI:-bolt://localhost:7687}"
BOLT_PORT="${BOLT_URI##*:}"
BOLT_USERNAME="${BOLT_USERNAME:-neo4j}"
BOLT_PASSWORD="${BOLT_PASSWORD:-}"
HTTP_PORT="${PORT:-8080}"

# ── Colors ──────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${CYAN}╔══════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  neovis.js — MITRE ATT&CK Threat Dashboard      ║${NC}"
echo -e "${CYAN}║  SedanDelivery Neo4j Compatibility Gallery       ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════╝${NC}"
echo ""

# ── Check SedanDelivery Bolt server ─────────────────────────
echo -n "Checking SedanDelivery Bolt on port ${BOLT_PORT}... "
if nc -z localhost "${BOLT_PORT}" 2>/dev/null; then
    echo -e "${GREEN}RUNNING${NC}"
else
    echo -e "${RED}NOT FOUND${NC}"
    echo ""
    echo -e "${YELLOW}SedanDelivery Bolt server is not running on port ${BOLT_PORT}.${NC}"
    echo "Start it first with:"
    echo ""
    echo "  cd $(dirname "${SCRIPT_DIR}")/.."
    echo "  cargo run --release --bin sedandelivery-bolt"
    echo ""
    echo "Then load the MITRE ATT&CK dataset:"
    echo ""
    echo "  python3 neo4j-compat-gallery/data/load_data.py"
    echo ""
    exit 1
fi

# ── Check Python ─────────────────────────────────────────────
echo -n "Checking Python... "
if command -v python3 &>/dev/null; then
    echo -e "${GREEN}$(python3 --version)${NC}"
else
    echo -e "${RED}NOT FOUND${NC}"
    echo "Python 3 is required."
    exit 1
fi

# ── Check if port is already in use ─────────────────────────
if nc -z localhost "${HTTP_PORT}" 2>/dev/null; then
    echo ""
    echo -e "${YELLOW}Port ${HTTP_PORT} is already in use.${NC}"
    echo "Either stop the existing server or change HTTP_PORT in this script."
    echo ""
    echo -e "Dashboard may already be at: ${CYAN}http://localhost:${HTTP_PORT}${NC}"
    exit 1
fi

# ── Start HTTP server ──────────────────────────────────────
echo ""
if [[ -n "${BOLT_PASSWORD}" ]]; then
    echo -e "Bolt auth: ${GREEN}enabled${NC} for user ${CYAN}${BOLT_USERNAME}${NC}"
else
    echo -e "Bolt auth: ${YELLOW}disabled${NC} (BOLT_PASSWORD unset)"
fi
echo -e "Installing Python dependency: ${CYAN}neo4j${NC}"
python3 -m pip install -q neo4j
echo -e "Starting HTTP server on port ${CYAN}${HTTP_PORT}${NC}..."
echo -e "Dashboard URL: ${GREEN}http://localhost:${HTTP_PORT}${NC}"
echo ""
echo "Press Ctrl+C to stop."
echo ""

cd "${SCRIPT_DIR}"
export BOLT_URI
export BOLT_USERNAME
export BOLT_PASSWORD
export PORT="${HTTP_PORT}"
exec python3 server.py
