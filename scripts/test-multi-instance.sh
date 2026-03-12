#!/usr/bin/env bash
# test-multi-instance.sh — Orchestrate multi-instance Bolt correctness test.
#
# Starts docker-compose.scale-test.yml (1 Valkey + 2 Bolt instances),
# runs the concurrent correctness test, and tears down.
#
# Usage:
#   export VALKEY_PASSWORD='change-me-for-local-dev'
#   ./scripts/test-multi-instance.sh
#
# Requirements:
#   - Docker with compose v2 plugin
#   - python3 with neo4j driver (pip install neo4j)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="$PROJECT_DIR/docker-compose.scale-test.yml"

# Colors (disabled if not a terminal)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    NC='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    NC=''
fi

log_info()  { echo -e "${YELLOW}[INFO]${NC}  $*"; }
log_pass()  { echo -e "${GREEN}[PASS]${NC}  $*"; }
log_fail()  { echo -e "${RED}[FAIL]${NC}  $*"; }

# ── Validate environment ──────────────────────────────────────────────────
if [ -z "${VALKEY_PASSWORD:-}" ]; then
    echo "ERROR: VALKEY_PASSWORD must be set."
    echo "Example: export VALKEY_PASSWORD='change-me-for-local-dev'"
    exit 1
fi
export VALKEY_PASSWORD
export BOLT_PASSWORD="${BOLT_PASSWORD:-$VALKEY_PASSWORD}"

if ! docker compose version &>/dev/null; then
    echo "ERROR: docker compose v2 is required but not found."
    exit 1
fi

if ! python3 -c "import neo4j" 2>/dev/null; then
    echo "ERROR: Python neo4j driver is required. Install with: pip install neo4j"
    exit 1
fi

# ── Cleanup function ─────────────────────────────────────────────────────
cleanup() {
    log_info "Cleaning up containers..."
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" down -v --remove-orphans --timeout 10 2>/dev/null || true
}
trap cleanup EXIT

# ── Main ─────────────────────────────────────────────────────────────────
main() {
    echo "================================================================"
    echo "  Multi-Instance Bolt Correctness Test"
    echo "================================================================"

    cd "$PROJECT_DIR"

    # Ensure clean state
    cleanup

    # Start services
    log_info "Building and starting services..."
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" up -d --build

    # Wait for both Bolt health endpoints
    log_info "Waiting for bolt-1 health endpoint..."
    for i in $(seq 1 90); do
        if curl -sf http://localhost:7688/health &>/dev/null; then
            log_info "bolt-1 is healthy (after ${i}s)"
            break
        fi
        if [ "$i" -eq 90 ]; then
            log_fail "bolt-1 did not become healthy within 90s"
            log_info "bolt-1 logs:"
            docker compose -f "$COMPOSE_FILE" logs --tail=30 bolt-1 2>/dev/null || true
            exit 1
        fi
        sleep 1
    done

    log_info "Waiting for bolt-2 health endpoint..."
    for i in $(seq 1 90); do
        if curl -sf http://localhost:7690/health &>/dev/null; then
            log_info "bolt-2 is healthy (after ${i}s)"
            break
        fi
        if [ "$i" -eq 90 ]; then
            log_fail "bolt-2 did not become healthy within 90s"
            log_info "bolt-2 logs:"
            docker compose -f "$COMPOSE_FILE" logs --tail=30 bolt-2 2>/dev/null || true
            exit 1
        fi
        sleep 1
    done

    # Run the test
    log_info "Running multi-instance correctness test..."
    python3 "$PROJECT_DIR/tests/test_multi_instance.py" \
        --bolt1 bolt://localhost:7687 \
        --bolt2 bolt://localhost:7689 \
        --password "$BOLT_PASSWORD" \
        --nodes-per-instance "${NODES_PER_INSTANCE:-100}"
    EXIT_CODE=$?

    if [ $EXIT_CODE -eq 0 ]; then
        log_pass "Multi-instance correctness test passed"
    else
        log_fail "Multi-instance correctness test failed"
        log_info "bolt-1 logs:"
        docker compose -f "$COMPOSE_FILE" logs --tail=50 bolt-1 2>/dev/null || true
        log_info "bolt-2 logs:"
        docker compose -f "$COMPOSE_FILE" logs --tail=50 bolt-2 2>/dev/null || true
    fi

    exit $EXIT_CODE
}

main "$@"
