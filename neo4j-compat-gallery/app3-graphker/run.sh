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
PORT="${PORT:-5001}"

echo "============================================"
echo "  CVE Knowledge Graph Explorer"
echo "  Powered by SedanDelivery + Bolt v4.4"
echo "============================================"
echo ""

# ── 1. Install requirements ─────────────────────────────────────────────────
echo "[1/3] Installing Python dependencies..."
pip install -q -r requirements.txt 2>/dev/null || pip3 install -q -r requirements.txt 2>/dev/null
echo "      Done."
echo ""

# ── 2. Load CVE data ────────────────────────────────────────────────────────
echo "[2/3] Loading CVE data into SedanDelivery..."

if [[ "${1:-}" == "--api" ]]; then
    echo "      Mode: NVD API (live fetch)"
    python3 cve_loader.py --bolt-uri "$BOLT_URI"
else
    echo "      Mode: Sample data (offline)"
    python3 cve_loader.py --sample --bolt-uri "$BOLT_URI"
fi
echo ""

# ── 3. Start Flask app ──────────────────────────────────────────────────────
if [[ "${1:-}" == "--load-only" ]]; then
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
export PORT
exec python3 app.py
