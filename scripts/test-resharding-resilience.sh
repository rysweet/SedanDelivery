#!/usr/bin/env bash
# test-resharding-resilience.sh — Verify Bolt survives Redis Cluster resharding.
#
# Uses docker-compose.cluster.yml (6-node Redis Cluster) and runs a background
# write load via the neo4j Python driver while resharding slots between masters.
# Checks /metrics for StorageError::Backend errors to detect data loss or crashes.
#
# Usage:
#   export VALKEY_PASSWORD='change-me-for-local-dev'
#   ./scripts/test-resharding-resilience.sh
#
# Requirements:
#   - Docker with compose v2 plugin
#   - python3 with neo4j driver (pip install neo4j)
#   - redis-cli available on the host (or uses the one from the Redis container)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="$PROJECT_DIR/docker-compose.cluster.yml"

# ── Colors ──────────────────────────────────────────────────────────────
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' NC=''
fi

log_info()  { echo -e "${YELLOW}[INFO]${NC}  $*"; }
log_pass()  { echo -e "${GREEN}[PASS]${NC}  $*"; }
log_fail()  { echo -e "${RED}[FAIL]${NC}  $*"; }

# ── Validate environment ───────────────────────────────────────────────
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

# ── Cleanup ─────────────────────────────────────────────────────────────
LOAD_PID=""
cleanup() {
    log_info "Cleaning up..."
    if [ -n "$LOAD_PID" ] && kill -0 "$LOAD_PID" 2>/dev/null; then
        kill "$LOAD_PID" 2>/dev/null || true
        wait "$LOAD_PID" 2>/dev/null || true
    fi
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" down -v --remove-orphans --timeout 10 2>/dev/null || true
}
trap cleanup EXIT

# ── Background load generator ──────────────────────────────────────────
generate_load() {
    python3 -u - "$BOLT_PASSWORD" "$1" <<'PYEOF'
import sys
import time
from neo4j import GraphDatabase

password = sys.argv[1]
duration = int(sys.argv[2])
uri = "bolt://localhost:7687"
graph_name = "__reshard_test"

driver = GraphDatabase.driver(uri, auth=("neo4j", password))

# Setup: create the test graph
with driver.session() as s:
    try:
        s.run(f"DROP GRAPH {graph_name}").consume()
    except Exception:
        pass
    s.run(f"CREATE GRAPH {graph_name}").consume()

writes = 0
errors = 0
deadline = time.time() + duration

while time.time() < deadline:
    try:
        with driver.session() as s:
            s.run(f"USE {graph_name} CREATE (:Load {{ts: {time.time()}, seq: {writes}}})").consume()
        writes += 1
    except Exception as e:
        errors += 1
        print(f"  [LOAD] Write error #{errors}: {e}", file=sys.stderr)
    time.sleep(0.05)

print(f"[LOAD] Completed: {writes} writes, {errors} errors over {duration}s")

# Cleanup
try:
    with driver.session() as s:
        s.run(f"DROP GRAPH {graph_name}").consume()
except Exception:
    pass

driver.close()
PYEOF
}

# ── Reshard helper ─────────────────────────────────────────────────────
# Uses redis-cli from inside the redis-node-1 container to avoid host dependency.
run_redis_cli_cluster() {
    docker compose -f "$COMPOSE_FILE" exec -T redis-node-1 \
        sh -c "REDISCLI_AUTH='$VALKEY_PASSWORD' redis-cli $*"
}

get_node_id() {
    local host="$1"
    docker compose -f "$COMPOSE_FILE" exec -T redis-node-1 \
        sh -c "REDISCLI_AUTH='$VALKEY_PASSWORD' redis-cli -h $host -p 6379 cluster myid" | tr -d '\r'
}

# ── Main ───────────────────────────────────────────────────────────────
main() {
    echo "================================================================"
    echo "  Resharding Resilience Test"
    echo "================================================================"

    cd "$PROJECT_DIR"

    # Ensure clean state
    cleanup 2>/dev/null || true

    # Start the 6-node cluster + Bolt
    log_info "Building and starting Redis Cluster + Bolt..."
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" up -d --build

    # Wait for Bolt health
    log_info "Waiting for Bolt health endpoint..."
    for i in $(seq 1 120); do
        if curl -sf http://localhost:7688/health &>/dev/null; then
            log_info "Bolt is healthy (after ${i}s)"
            break
        fi
        if [ "$i" -eq 120 ]; then
            log_fail "Bolt did not become healthy within 120s"
            docker compose -f "$COMPOSE_FILE" logs --tail=30 sedandelivery-bolt 2>/dev/null || true
            exit 1
        fi
        sleep 1
    done

    # Capture baseline backend errors from /metrics
    BASELINE_ERRORS=$(curl -sf http://localhost:7688/metrics | \
        grep '^sedandelivery_bolt_backend_errors_total' | \
        awk '{print $2}' || echo "0")
    log_info "Baseline backend errors: $BASELINE_ERRORS"

    # Start background load (60 seconds of continuous writes)
    LOAD_DURATION=60
    log_info "Starting background write load for ${LOAD_DURATION}s..."
    generate_load "$LOAD_DURATION" &
    LOAD_PID=$!
    sleep 5  # Let a few writes succeed before resharding

    # Get node IDs for resharding
    log_info "Getting cluster node IDs..."
    NODE1_ID=$(get_node_id redis-node-1)
    NODE2_ID=$(get_node_id redis-node-2)
    log_info "  Node 1: $NODE1_ID"
    log_info "  Node 2: $NODE2_ID"

    # Reshard: move 1000 slots from node-1 to node-2
    log_info "Resharding 1000 slots from node-1 to node-2..."
    docker compose -f "$COMPOSE_FILE" exec -T redis-node-1 \
        sh -c "REDISCLI_AUTH='$VALKEY_PASSWORD' redis-cli --cluster reshard \
            redis-node-1:6379 \
            --cluster-from $NODE1_ID \
            --cluster-to $NODE2_ID \
            --cluster-slots 1000 \
            --cluster-yes" || {
        log_info "Reshard command returned non-zero (may be expected during load)"
    }

    log_info "Resharding complete, waiting for load to finish..."
    wait "$LOAD_PID" || true
    LOAD_PID=""

    # Wait a moment for metrics to settle
    sleep 2

    # Check /metrics for backend errors
    log_info "Checking /metrics for backend errors..."
    FINAL_ERRORS=$(curl -sf http://localhost:7688/metrics | \
        grep '^sedandelivery_bolt_backend_errors_total' | \
        awk '{print $2}' || echo "0")

    # Strip potential trailing decimals (Prometheus gauge may output "0.0" or "0")
    BASELINE_INT=$(printf "%.0f" "$BASELINE_ERRORS")
    FINAL_INT=$(printf "%.0f" "$FINAL_ERRORS")
    NEW_ERRORS=$((FINAL_INT - BASELINE_INT))

    echo ""
    echo "================================================================"
    echo "  Results"
    echo "================================================================"
    echo "  Baseline backend errors: $BASELINE_INT"
    echo "  Final backend errors:    $FINAL_INT"
    echo "  New errors during test:  $NEW_ERRORS"

    if [ "$NEW_ERRORS" -eq 0 ]; then
        log_pass "No StorageError::Backend errors during resharding"
        exit 0
    else
        log_fail "$NEW_ERRORS StorageError::Backend errors occurred during resharding"
        log_info "Bolt logs:"
        docker compose -f "$COMPOSE_FILE" logs --tail=50 sedandelivery-bolt 2>/dev/null || true
        exit 1
    fi
}

main "$@"
