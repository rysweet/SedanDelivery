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
#   - redis-cli is provided by the Redis container

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
export COMPOSE_FILE="${COMPOSE_FILE:-$PROJECT_DIR/docker-compose.cluster.yml}"

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
export CI_PREBUILD_BOLT="${CI_PREBUILD_BOLT:-0}"
export RESHARD_LOAD_DURATION="${RESHARD_LOAD_DURATION:-60}"
export RESHARD_WARMUP_SECS="${RESHARD_WARMUP_SECS:-7}"
export RESHARD_SLOT_COUNT="${RESHARD_SLOT_COUNT:-1000}"

if ! docker compose version &>/dev/null; then
    echo "ERROR: docker compose v2 is required but not found."
    exit 1
fi

if ! python3 -c "import neo4j" 2>/dev/null; then
    echo "ERROR: Python neo4j driver is required. Install with: pip install neo4j"
    exit 1
fi

docker_compose() {
    docker compose "$@"
}

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
        docker_compose down -v --remove-orphans --timeout 10 2>/dev/null || true
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
    docker_compose exec -T redis-node-1 \
        sh -c "REDISCLI_AUTH='$VALKEY_PASSWORD' redis-cli $*"
}

get_node_id() {
    local host="$1"
    docker_compose exec -T redis-node-1 \
        sh -c "REDISCLI_AUTH='$VALKEY_PASSWORD' redis-cli -h $host -p 6379 cluster myid" | tr -d '\r'
}

get_slot_count() {
    local node_id="$1"
    local cluster_nodes
    cluster_nodes="$(run_redis_cli_cluster cluster nodes)"
    CLUSTER_NODES_TEXT="$cluster_nodes" python3 - "$node_id" <<'PYEOF'
import os
import sys

node_id = sys.argv[1]

for raw_line in os.environ["CLUSTER_NODES_TEXT"].splitlines():
    parts = raw_line.strip().split()
    if not parts or parts[0] != node_id:
        continue

    total = 0
    for token in parts[8:]:
        if token.startswith('['):
            continue
        if '-' in token:
            start, end = token.split('-', 1)
            if start.isdigit() and end.isdigit():
                total += int(end) - int(start) + 1
        elif token.isdigit():
            total += 1

    print(total)
    break
else:
    raise SystemExit(f"node {node_id} not found in CLUSTER NODES output")
PYEOF
}

parse_summary_fields() {
    local kind="$1"
    local line="$2"
    python3 - "$kind" "$line" <<'PYEOF'
import re
import sys

kind = sys.argv[1]
line = sys.argv[2]
patterns = {
    "load": r"\[LOAD\] Completed: (\d+) writes, (\d+) errors over (\d+)s",
    "read": r"\[READ\] Completed: (\d+) checks, (\d+) errors during load",
    "label": r"\[LABEL\] Completed: (\d+) mutation cycles, (\d+) errors during load",
    "readback": r"\[READBACK\] Visible after resharding: (\d+) nodes",
}

match = re.search(patterns[kind], line)
if not match:
    raise SystemExit(f"could not parse {kind} summary: {line}")

print(" ".join(match.groups()))
PYEOF
}

# ── Main ───────────────────────────────────────────────────────────────
main() {
    echo "================================================================"
    echo "  Resharding Resilience Test"
    echo "================================================================"

    cd "$PROJECT_DIR"
    log_info "Using compose definition(s): $COMPOSE_FILE"

    # Ensure clean state
    cleanup 2>/dev/null || true

    # Start the 6-node cluster + Bolt
    if [ "$CI_PREBUILD_BOLT" = "1" ]; then
        log_info "Starting Redis Cluster + Bolt from prebuilt image..."
        VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
            docker_compose up -d
    else
        log_info "Building and starting Redis Cluster + Bolt..."
        VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
            docker_compose up -d --build
    fi

    # Wait for Bolt health
    log_info "Waiting for Bolt health endpoint..."
    for i in $(seq 1 120); do
        if curl -sf http://localhost:7688/health &>/dev/null; then
            log_info "Bolt is healthy (after ${i}s)"
            break
        fi
        if [ "$i" -eq 120 ]; then
            log_fail "Bolt did not become healthy within 120s"
            docker_compose logs --tail=30 sedandelivery-bolt 2>/dev/null || true
            exit 1
        fi
        sleep 1
    done

    # Start background load and give the setup phase time to settle before
    # taking the backend-error baseline. Graph creation can legitimately touch
    # the backend once during startup, but the resharding window must stay clean.
    log_info "Starting background write load for ${RESHARD_LOAD_DURATION}s..."
    LOAD_OUTPUT_FILE="$(mktemp)"
    generate_load "$RESHARD_LOAD_DURATION" > >(tee "$LOAD_OUTPUT_FILE") 2>&1 &
    LOAD_PID=$!
    sleep "$RESHARD_WARMUP_SECS"

    BASELINE_ERRORS=$(curl -sf http://localhost:7688/metrics | \
        grep '^sedandelivery_bolt_backend_errors_total' | \
        awk '{print $2}' || echo "0")
    log_info "Baseline backend errors after warmup: $BASELINE_ERRORS"

    # Get node IDs for resharding
    log_info "Getting cluster node IDs..."
    NODE1_ID=$(get_node_id redis-node-1)
    NODE2_ID=$(get_node_id redis-node-2)
    log_info "  Node 1: $NODE1_ID"
    log_info "  Node 2: $NODE2_ID"
    NODE1_SLOTS_BEFORE=$(get_slot_count "$NODE1_ID")
    NODE2_SLOTS_BEFORE=$(get_slot_count "$NODE2_ID")
    log_info "  Node 1 slots before: $NODE1_SLOTS_BEFORE"
    log_info "  Node 2 slots before: $NODE2_SLOTS_BEFORE"

    # Reshard a chunk of slots from node-1 to node-2 while the workload runs.
    log_info "Resharding ${RESHARD_SLOT_COUNT} slots from node-1 to node-2..."
    if ! docker_compose exec -T redis-node-1 \
        sh -c "REDISCLI_AUTH='$VALKEY_PASSWORD' redis-cli --cluster reshard \
            redis-node-1:6379 \
            --cluster-from $NODE1_ID \
            --cluster-to $NODE2_ID \
            --cluster-slots $RESHARD_SLOT_COUNT \
            --cluster-yes"; then
        log_fail "Reshard command failed before slot movement could be verified"
        exit 1
    fi
    NODE1_SLOTS_AFTER=$(get_slot_count "$NODE1_ID")
    NODE2_SLOTS_AFTER=$(get_slot_count "$NODE2_ID")
    MOVED_FROM_NODE1=$((NODE1_SLOTS_BEFORE - NODE1_SLOTS_AFTER))
    MOVED_TO_NODE2=$((NODE2_SLOTS_AFTER - NODE2_SLOTS_BEFORE))
    if [ "$MOVED_FROM_NODE1" -ne "$RESHARD_SLOT_COUNT" ] || \
       [ "$MOVED_TO_NODE2" -ne "$RESHARD_SLOT_COUNT" ]; then
        log_fail "Slot ownership did not change as requested (node-1: $NODE1_SLOTS_BEFORE->$NODE1_SLOTS_AFTER, node-2: $NODE2_SLOTS_BEFORE->$NODE2_SLOTS_AFTER, expected move: $RESHARD_SLOT_COUNT)"
        exit 1
    fi
    log_pass "Slot ownership changed during resharding (node-1: $NODE1_SLOTS_BEFORE->$NODE1_SLOTS_AFTER, node-2: $NODE2_SLOTS_BEFORE->$NODE2_SLOTS_AFTER, moved $RESHARD_SLOT_COUNT slots)"

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
    LOAD_FIELDS=$(parse_summary_fields load "$LOAD_SUMMARY") || {
        log_fail "Could not parse load summary: $LOAD_SUMMARY"
        exit 1
    }
    read -r WRITE_COUNT LOAD_ERRORS _LOAD_DURATION_REPORTED <<< "$LOAD_FIELDS"
    READ_SUMMARY=$(grep '\[READ\] Completed:' "$LOAD_OUTPUT_FILE" | tail -n 1 || true)
    if [ -z "$READ_SUMMARY" ]; then
        log_fail "Load generator did not emit a read verification summary"
        exit 1
    fi
    READ_FIELDS=$(parse_summary_fields read "$READ_SUMMARY") || {
        log_fail "Could not parse read summary: $READ_SUMMARY"
        exit 1
    }
    read -r READ_CHECKS READ_ERRORS <<< "$READ_FIELDS"
    LABEL_SUMMARY=$(grep '\[LABEL\] Completed:' "$LOAD_OUTPUT_FILE" | tail -n 1 || true)
    if [ -z "$LABEL_SUMMARY" ]; then
        log_fail "Load generator did not emit a label mutation summary"
        exit 1
    fi
    LABEL_FIELDS=$(parse_summary_fields label "$LABEL_SUMMARY") || {
        log_fail "Could not parse label summary: $LABEL_SUMMARY"
        exit 1
    }
    read -r LABEL_MUTATIONS LABEL_ERRORS <<< "$LABEL_FIELDS"
    READBACK_SUMMARY=$(grep '\[READBACK\] Visible after resharding:' "$LOAD_OUTPUT_FILE" | tail -n 1 || true)
    if [ -z "$READBACK_SUMMARY" ]; then
        log_fail "Load generator did not emit a post-resharding read-back summary"
        exit 1
    fi
    READBACK_FIELDS=$(parse_summary_fields readback "$READBACK_SUMMARY") || {
        log_fail "Could not parse read-back summary: $READBACK_SUMMARY"
        exit 1
    }
    read -r READBACK_COUNT <<< "$READBACK_FIELDS"

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
        docker_compose logs --tail=50 sedandelivery-bolt 2>/dev/null || true
        exit 1
    fi
}

main "$@"
