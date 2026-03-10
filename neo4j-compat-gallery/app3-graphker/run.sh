#!/usr/bin/env bash
# run.sh - Setup and launch the CVE Knowledge Graph Explorer
#
# Usage:
#   ./run.sh              # Load sample data + start Flask app
#   ./run.sh --api        # Fetch from NVD API + start Flask app
#   ./run.sh --load-only  # Load data without starting the web server

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

BOLT_URI="${BOLT_URI:-bolt://localhost:7687}"
BOLT_USERNAME="${BOLT_USERNAME:-neo4j}"
BOLT_PASSWORD="${BOLT_PASSWORD:-}"
PORT="${PORT:-5001}"
LOAD_SOURCE="sample"
LOAD_ONLY=false

for arg in "$@"; do
    case "$arg" in
        --api)
            LOAD_SOURCE="api"
            ;;
        --load-only)
            LOAD_ONLY=true
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            echo "Usage: ./run.sh [--api] [--load-only]" >&2
            exit 1
            ;;
    esac
done

echo "============================================"
echo "  CVE Knowledge Graph Explorer"
echo "  Powered by SedanDelivery + Bolt v4.4"
echo "============================================"
echo ""
if [[ -n "$BOLT_PASSWORD" ]]; then
    echo "[*] Bolt auth enabled for user: $BOLT_USERNAME"
else
    echo "[*] Bolt auth disabled (BOLT_PASSWORD unset)"
fi
echo ""

# ── 1. Install requirements ─────────────────────────────────────────────────
echo "[1/3] Installing Python dependencies..."
pip install -q -r requirements.txt 2>/dev/null || pip3 install -q -r requirements.txt 2>/dev/null
echo "      Done."
echo ""

# ── 2. Load CVE data ────────────────────────────────────────────────────────
echo "[2/3] Loading CVE data into SedanDelivery..."

if [[ "$LOAD_SOURCE" == "api" ]]; then
    echo "      Mode: NVD API (live fetch)"
    python3 cve_loader.py --bolt-uri "$BOLT_URI"
else
    echo "      Mode: Sample data (offline)"
    python3 cve_loader.py --sample --bolt-uri "$BOLT_URI"
fi
echo ""

# ── 3. Start Flask app ──────────────────────────────────────────────────────
if [[ "$LOAD_ONLY" == true ]]; then
    echo "[3/3] Data loaded. Skipping web server (--load-only)."
    exit 0
fi

echo "[3/3] Starting Flask web server on port $PORT..."
echo "      Dashboard:  http://localhost:$PORT/"
echo "      Query page: http://localhost:$PORT/query"
echo ""
echo "      Press Ctrl+C to stop."
echo ""

export BOLT_URI
export BOLT_USERNAME
export BOLT_PASSWORD
export PORT
exec python3 app.py
