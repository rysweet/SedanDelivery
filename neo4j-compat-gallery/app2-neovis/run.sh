#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────
# run.sh — Start the neovis.js MITRE ATT&CK dashboard
#
# Prerequisites:
#   - SedanDelivery Bolt server running on localhost:7687
#   - MITRE ATT&CK dataset loaded
#   - Node.js / npx available
# ─────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOLT_PORT=7687
HTTP_PORT=8080

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

# ── Check Node.js ───────────────────────────────────────────
echo -n "Checking Node.js... "
if command -v node &>/dev/null; then
    echo -e "${GREEN}$(node --version)${NC}"
else
    echo -e "${RED}NOT FOUND${NC}"
    echo "Node.js is required. Install it from https://nodejs.org/"
    exit 1
fi

# ── Check npx ───────────────────────────────────────────────
echo -n "Checking npx... "
if command -v npx &>/dev/null; then
    echo -e "${GREEN}available${NC}"
else
    echo -e "${RED}NOT FOUND${NC}"
    echo "npx is required (comes with npm 5.2+)."
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
echo -e "Starting HTTP server on port ${CYAN}${HTTP_PORT}${NC}..."
echo -e "Dashboard URL: ${GREEN}http://localhost:${HTTP_PORT}${NC}"
echo ""
echo "Press Ctrl+C to stop."
echo ""

cd "${SCRIPT_DIR}"
npx serve -l "${HTTP_PORT}" -s . --cors
