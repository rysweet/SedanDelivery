#!/usr/bin/env bash
# Neo4j Compatibility Gallery — Run all apps locally (no Docker required)
#
# Prerequisites:
#   - SedanDelivery Bolt server built: cargo build --release -p sedandelivery-bolt
#   - Python 3.11+ with neo4j driver: pip install neo4j flask requests
#   - Docker available if you want to launch Neodash from this script
#
# Usage: ./run-all.sh [--stop]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BOLT_BIN="$REPO_ROOT/target/release/sedandelivery-bolt"
PID_DIR="$SCRIPT_DIR/.pids"
BOLT_URI="${BOLT_URI:-bolt://localhost:7687}"
BOLT_USERNAME="${BOLT_USERNAME:-neo4j}"
BOLT_PASSWORD="${BOLT_PASSWORD:-}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Stop mode ────────────────────────────────────────────────────────────
if [[ "${1:-}" == "--stop" ]]; then
    echo -e "${YELLOW}Stopping all gallery services...${NC}"
    for pidfile in "$PID_DIR"/*.pid; do
        [ -f "$pidfile" ] || continue
        name=$(basename "$pidfile" .pid)
        pid=$(cat "$pidfile")
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            echo -e "  ${RED}Stopped${NC} $name (PID $pid)"
        fi
        rm -f "$pidfile"
    done
    echo -e "${GREEN}All services stopped.${NC}"
    exit 0
fi

mkdir -p "$PID_DIR"

if [[ -z "$BOLT_PASSWORD" ]]; then
    echo -e "${RED}BOLT_PASSWORD is required for ./neo4j-compat-gallery/run-all.sh${NC}"
    echo "Export BOLT_PASSWORD (and optionally BOLT_USERNAME) before starting the full gallery."
    echo "Use the per-app scripts directly only if you intentionally want a no-auth local demo."
    exit 1
fi

echo -e "${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║     Neo4j Compatibility Gallery — SedanDelivery         ║${NC}"
echo -e "${CYAN}║     Validating Bolt v4.4 protocol with 4 OSS apps       ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${GREEN}Bolt auth enabled${NC} for user ${BOLT_USERNAME}"
echo ""

export BOLT_URI
export BOLT_USERNAME
export BOLT_PASSWORD

# ── 1. Start SedanDelivery Bolt server ───────────────────────────────────
echo -e "${BLUE}[1/6]${NC} Starting SedanDelivery Bolt server..."
if nc -z localhost 7687 2>/dev/null; then
    echo -e "  ${GREEN}Already running on port 7687${NC}"
else
    if [ ! -f "$BOLT_BIN" ]; then
        echo -e "  ${YELLOW}Building Bolt server...${NC}"
        (cd "$REPO_ROOT" && cargo build --release -p sedandelivery-bolt 2>&1 | tail -1)
    fi
    RUST_LOG=info nohup "$BOLT_BIN" > /tmp/bolt-server.log 2>&1 &
    echo $! > "$PID_DIR/bolt-server.pid"
    sleep 2
    if nc -z localhost 7687 2>/dev/null; then
        echo -e "  ${GREEN}Bolt server started on port 7687${NC}"
    else
        echo -e "  ${RED}Failed to start Bolt server. Check /tmp/bolt-server.log${NC}"
        exit 1
    fi
fi

# ── 2. Load MITRE ATT&CK dataset ────────────────────────────────────────
echo -e "${BLUE}[2/6]${NC} Loading MITRE ATT&CK security dataset..."
python3 "$SCRIPT_DIR/data/load_data.py" --clear 2>&1 | grep -E "^  |loaded"

# ── 3. Start App 1: Neodash ─────────────────────────────────────────────
echo -e "${BLUE}[3/6]${NC} Starting Neodash dashboard (port 5005)..."
if command -v docker &>/dev/null; then
    docker rm -f gallery-neodash 2>/dev/null || true
    docker run -d --name gallery-neodash \
        -p 5005:5005 \
        -e ssoEnabled=false \
        -e standalone=true \
        -e standaloneProtocol=bolt \
        -e standaloneHost=172.17.0.1 \
        -e standalonePort=7687 \
        -e standaloneUsername="${BOLT_USERNAME}" \
        -e standalonePassword="${BOLT_PASSWORD}" \
        neo4jlabs/neodash:latest >/dev/null 2>&1 && \
    echo -e "  ${GREEN}Neodash running at http://localhost:5005${NC}" || \
    echo -e "  ${YELLOW}Neodash Docker failed — run manually: docker run -p 5005:5005 neo4jlabs/neodash:latest${NC}"
else
    echo -e "  ${YELLOW}Docker not available — Neodash requires Docker. See app1-neodash/README.md${NC}"
fi

# ── 4. Start App 2: neovis.js ───────────────────────────────────────────
echo -e "${BLUE}[4/6]${NC} Starting neovis.js visualization (port 8080)..."
if command -v python3 &>/dev/null; then
    (cd "$SCRIPT_DIR/app2-neovis" && \
        python3 -m pip install -q neo4j 2>/dev/null && \
        PORT=8080 nohup python3 server.py > /tmp/neovis-server.log 2>&1 &)
    echo $! > "$PID_DIR/neovis.pid"
    sleep 1
    echo -e "  ${GREEN}neovis.js running at http://localhost:8080${NC}"
else
    echo -e "  ${YELLOW}python3 not available — install Python 3. See app2-neovis/README.md${NC}"
fi

# ── 5. Start App 3: GraphKer CVE Explorer ────────────────────────────────
echo -e "${BLUE}[5/6]${NC} Starting GraphKer CVE Explorer (port 5001)..."
(cd "$SCRIPT_DIR/app3-graphker" && \
    python3 -m pip install -q neo4j flask requests 2>/dev/null && \
    python3 sample_cves.py 2>&1 | tail -1 && \
    FLASK_ENV=development PORT=5001 nohup python3 app.py > /tmp/graphker.log 2>&1 &)
echo $! > "$PID_DIR/graphker.pid"
sleep 2
echo -e "  ${GREEN}GraphKer running at http://localhost:5001${NC}"

# ── 6. Start App 4: Network Security Analyzer ───────────────────────────
echo -e "${BLUE}[6/6]${NC} Starting Network Security Analyzer (port 5002)..."
(cd "$SCRIPT_DIR/app4-network-security" && \
    python3 -m pip install -q neo4j flask 2>/dev/null && \
    python3 network_data.py 2>&1 | tail -1 && \
    FLASK_ENV=development PORT=5002 nohup python3 app.py > /tmp/network-security.log 2>&1 &)
echo $! > "$PID_DIR/network-security.pid"
sleep 2
echo -e "  ${GREEN}Network Security Analyzer running at http://localhost:5002${NC}"

# ── Summary ──────────────────────────────────────────────────────────────
echo ""
echo -e "${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║                   Gallery Running!                       ║${NC}"
echo -e "${CYAN}╠══════════════════════════════════════════════════════════╣${NC}"
echo -e "${CYAN}║${NC}  SedanDelivery Bolt:    ${GREEN}bolt://localhost:7687${NC}              ${CYAN}║${NC}"
echo -e "${CYAN}║${NC}  Health/Metrics:        ${GREEN}http://localhost:7688/health${NC}       ${CYAN}║${NC}"
echo -e "${CYAN}║${NC}                                                          ${CYAN}║${NC}"
echo -e "${CYAN}║${NC}  ${BLUE}App 1${NC} Neodash:         ${GREEN}http://localhost:5005${NC}            ${CYAN}║${NC}"
echo -e "${CYAN}║${NC}  ${BLUE}App 2${NC} neovis.js:       ${GREEN}http://localhost:8080${NC}            ${CYAN}║${NC}"
echo -e "${CYAN}║${NC}  ${BLUE}App 3${NC} GraphKer:        ${GREEN}http://localhost:5001${NC}            ${CYAN}║${NC}"
echo -e "${CYAN}║${NC}  ${BLUE}App 4${NC} Network Sec:     ${GREEN}http://localhost:5002${NC}            ${CYAN}║${NC}"
echo -e "${CYAN}╠══════════════════════════════════════════════════════════╣${NC}"
echo -e "${CYAN}║${NC}  Stop all: ${YELLOW}./run-all.sh --stop${NC}                           ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
