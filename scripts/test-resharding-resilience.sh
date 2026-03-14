#!/usr/bin/env bash
# test-resharding-resilience.sh — Verify Bolt survives Redis Cluster resharding.
#
# Uses docker-compose.cluster.yml (6-node Redis Cluster) and runs a background
# write load via the neo4j Python driver while resharding slots between masters.
# Checks /metrics for StorageError::Backend errors to detect data loss or crashes.
#
# Usage:
#   ./scripts/test-resharding-resilience.sh
#   VALKEY_PASSWORD='custom-pass' BOLT_PASSWORD='custom-pass' ./scripts/test-resharding-resilience.sh
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
export VALKEY_PASSWORD="${VALKEY_PASSWORD:-local-cluster-pass}"
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
LOAD_OUTPUT_FILE=""
cleanup() {
    log_info "Cleaning up..."
    if [ -n "$LOAD_PID" ] && kill -0 "$LOAD_PID" 2>/dev/null; then
        kill "$LOAD_PID" 2>/dev/null || true
        wait "$LOAD_PID" 2>/dev/null || true
    fi
    if [ -n "$LOAD_OUTPUT_FILE" ] && [ -f "$LOAD_OUTPUT_FILE" ]; then
        rm -f "$LOAD_OUTPUT_FILE"
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
write_errors = 0
read_checks = 0
read_errors = 0
label_mutations = 0
label_errors = 0
deadline = time.time() + duration

while time.time() < deadline:
    try:
        with driver.session(database=graph_name) as s:
            s.run(
                "CREATE (:LoadNode {ts: $ts, seq: $seq})",
                ts=time.time(),
                seq=writes,
            ).consume()
        writes += 1
    except Exception as e:
        write_errors += 1
        print(f"  [LOAD] Write error #{write_errors}: {e}", file=sys.stderr)

    if writes > 0 and writes % 25 == 0:
        try:
            with driver.session(database=graph_name) as s:
                s.run("MATCH (n:LoadNode) RETURN count(n) AS c").single()["c"]
            read_checks += 1
        except Exception as e:
            read_errors += 1
            print(f"  [READ] Read error #{read_errors}: {e}", file=sys.stderr)
        try:
            target_seq = writes - 1
            with driver.session(database=graph_name) as s:
                s.run(
                    "MATCH (n:LoadNode {seq: $seq}) SET n:HotNode",
                    seq=target_seq,
                ).consume()
                hot_count = s.run(
                    "MATCH (n:HotNode {seq: $seq}) RETURN count(n) AS c",
                    seq=target_seq,
                ).single()["c"]
                if hot_count != 1:
                    raise RuntimeError(
                        f"expected node seq={target_seq} to gain HotNode label, got {hot_count} matches"
                    )
                s.run(
                    "MATCH (n:HotNode {seq: $seq}) REMOVE n:HotNode",
                    seq=target_seq,
                ).consume()
                hot_count = s.run(
                    "MATCH (n:HotNode {seq: $seq}) RETURN count(n) AS c",
                    seq=target_seq,
                ).single()["c"]
                if hot_count != 0:
                    raise RuntimeError(
                        f"expected node seq={target_seq} to lose HotNode label, got {hot_count} matches"
                    )
            label_mutations += 1
        except Exception as e:
            label_errors += 1
            print(f"  [LABEL] Label mutation error #{label_errors}: {e}", file=sys.stderr)
    time.sleep(0.05)

print(f"[LOAD] Completed: {writes} writes, {write_errors} errors over {duration}s")
print(f"[READ] Completed: {read_checks} checks, {read_errors} errors during load")
print(f"[LABEL] Completed: {label_mutations} mutation cycles, {label_errors} errors during load")

with driver.session(database=graph_name) as s:
    visible = s.run("MATCH (n:LoadNode) RETURN count(n) AS c").single()["c"]
print(f"[READBACK] Visible after resharding: {visible} nodes")
if visible != writes:
    print(
        f"[READBACK] Expected {writes} nodes after resharding, found {visible}",
        file=sys.stderr,
    )
    sys.exit(1)

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
    LOAD_OUTPUT_FILE="$(mktemp)"
    generate_load "$LOAD_DURATION" > >(tee "$LOAD_OUTPUT_FILE") 2>&1 &
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
    set +e
    wait "$LOAD_PID"
    LOAD_EXIT=$?
    set -e
    LOAD_PID=""

    if [ "$LOAD_EXIT" -ne 0 ]; then
        log_fail "Load generator exited with status $LOAD_EXIT"
        exit 1
    fi

    LOAD_SUMMARY=$(grep '\[LOAD\] Completed:' "$LOAD_OUTPUT_FILE" | tail -n 1 || true)
    if [ -z "$LOAD_SUMMARY" ]; then
        log_fail "Load generator did not emit a completion summary"
        exit 1
    fi
    WRITE_COUNT=$(printf '%s\n' "$LOAD_SUMMARY" | grep -oP '\[LOAD\] Completed: \K\d+(?= writes,)' || true)
    LOAD_ERRORS=$(printf '%s\n' "$LOAD_SUMMARY" | grep -oP 'writes, \K\d+(?= errors)' || true)
    WRITE_COUNT="${WRITE_COUNT:-0}"
    LOAD_ERRORS="${LOAD_ERRORS:-0}"
    READ_SUMMARY=$(grep '\[READ\] Completed:' "$LOAD_OUTPUT_FILE" | tail -n 1 || true)
    if [ -z "$READ_SUMMARY" ]; then
        log_fail "Load generator did not emit a read verification summary"
        exit 1
    fi
    READ_CHECKS=$(printf '%s\n' "$READ_SUMMARY" | grep -oP '\[READ\] Completed: \K\d+(?= checks,)' || true)
    READ_ERRORS=$(printf '%s\n' "$READ_SUMMARY" | grep -oP 'checks, \K\d+(?= errors during load)' || true)
    READ_CHECKS="${READ_CHECKS:-0}"
    READ_ERRORS="${READ_ERRORS:-0}"
    LABEL_SUMMARY=$(grep '\[LABEL\] Completed:' "$LOAD_OUTPUT_FILE" | tail -n 1 || true)
    if [ -z "$LABEL_SUMMARY" ]; then
        log_fail "Load generator did not emit a label mutation summary"
        exit 1
    fi
    LABEL_MUTATIONS=$(printf '%s\n' "$LABEL_SUMMARY" | grep -oP '\[LABEL\] Completed: \K\d+(?= mutation cycles,)' || true)
    LABEL_ERRORS=$(printf '%s\n' "$LABEL_SUMMARY" | grep -oP 'mutation cycles, \K\d+(?= errors during load)' || true)
    LABEL_MUTATIONS="${LABEL_MUTATIONS:-0}"
    LABEL_ERRORS="${LABEL_ERRORS:-0}"
    READBACK_SUMMARY=$(grep '\[READBACK\] Visible after resharding:' "$LOAD_OUTPUT_FILE" | tail -n 1 || true)
    if [ -z "$READBACK_SUMMARY" ]; then
        log_fail "Load generator did not emit a post-resharding read-back summary"
        exit 1
    fi
    READBACK_COUNT=$(printf '%s\n' "$READBACK_SUMMARY" | grep -oP '\[READBACK\] Visible after resharding: \K\d+(?= nodes)' || true)
    READBACK_COUNT="${READBACK_COUNT:-0}"

    if [ "$WRITE_COUNT" -lt 10 ]; then
        log_fail "Load generator produced only $WRITE_COUNT writes; expected at least 10"
        exit 1
    fi
    if [ "$LOAD_ERRORS" -ne 0 ]; then
        log_fail "Load generator saw $LOAD_ERRORS application-level write errors during resharding"
        exit 1
    fi
    if [ "$READ_CHECKS" -lt 1 ]; then
        log_fail "Load generator did not complete any read checks during resharding"
        exit 1
    fi
    if [ "$READ_ERRORS" -ne 0 ]; then
        log_fail "Load generator saw $READ_ERRORS read errors during resharding"
        exit 1
    fi
    if [ "$LABEL_MUTATIONS" -lt 1 ]; then
        log_fail "Load generator did not complete any label mutation cycles during resharding"
        exit 1
    fi
    if [ "$LABEL_ERRORS" -ne 0 ]; then
        log_fail "Load generator saw $LABEL_ERRORS label mutation errors during resharding"
        exit 1
    fi
    if [ "$READBACK_COUNT" -ne "$WRITE_COUNT" ]; then
        log_fail "Read-back after resharding found $READBACK_COUNT nodes; expected $WRITE_COUNT"
        exit 1
    fi
    log_pass "Read-back after resharding matched load count ($READBACK_COUNT nodes)"

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
