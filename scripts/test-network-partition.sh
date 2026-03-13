#!/usr/bin/env bash
# test-network-partition.sh — Verify Bolt handles Redis network partitions gracefully.
#
# Uses docker-compose.chaos-test.yml (Bolt -> toxiproxy -> Valkey).
# Injects latency, then a full disconnect via the toxiproxy admin API,
# verifies Bolt returns proper errors (not hangs or panics), then heals
# the partition and confirms recovery.
#
# Usage:
#   export VALKEY_PASSWORD='change-me-for-local-dev'
#   ./scripts/test-network-partition.sh
#
# Requirements:
#   - Docker with compose v2 plugin
#   - python3 with neo4j driver (pip install neo4j)
#   - curl

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="$PROJECT_DIR/docker-compose.chaos-test.yml"

TOXIPROXY_API="http://localhost:8474"
BOLT1_URI="bolt://localhost:7687"
BOLT1_METRICS="http://localhost:7688/metrics"
BOLT1_HEALTH="http://localhost:7688/health"
BOLT2_URI="bolt://localhost:7689"
BOLT2_HEALTH="http://localhost:7690/health"

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
LOAD_OUTPUT_FILE=""
cleanup() {
    log_info "Cleaning up..."
    if [ -n "$LOAD_PID" ] && kill -0 "$LOAD_PID" 2>/dev/null; then
        kill "$LOAD_PID" 2>/dev/null || true
        wait "$LOAD_PID" 2>/dev/null || true
    fi
    [ -n "$LOAD_OUTPUT_FILE" ] && rm -f "$LOAD_OUTPUT_FILE" 2>/dev/null || true
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" down -v --remove-orphans --timeout 10 2>/dev/null || true
}
trap cleanup EXIT

# ── Background load generator ──────────────────────────────────────────
# Writes continuously and tracks successes/errors. Does NOT exit on error
# because we expect errors during the partition.
generate_load() {
    python3 -u - "$BOLT_PASSWORD" "$1" "$2" <<'PYEOF'
import sys
import time
import json
from neo4j import GraphDatabase
from neo4j.exceptions import ServiceUnavailable, SessionExpired, Neo4jError

password = sys.argv[1]
uri = sys.argv[2]
duration = int(sys.argv[3])
graph_name = "__partition_test"

driver = GraphDatabase.driver(uri, auth=("neo4j", password),
                              connection_timeout=5, max_transaction_retry_time=5)

# Setup: create the test graph (tolerate failure during partition)
for attempt in range(10):
    try:
        with driver.session() as s:
            try:
                s.run(f"DROP GRAPH {graph_name}").consume()
            except Exception:
                pass
            s.run(f"CREATE GRAPH {graph_name}").consume()
        break
    except Exception as e:
        if attempt == 9:
            print(f"[LOAD] Failed to create graph after 10 attempts: {e}", file=sys.stderr)
            sys.exit(1)
        time.sleep(1)

writes = 0
errors = 0
error_types = {}
deadline = time.time() + duration

while time.time() < deadline:
    try:
        with driver.session() as s:
            s.run(f"USE {graph_name} CREATE (:Load {{ts: {time.time()}, seq: {writes}}})").consume()
        writes += 1
    except (ServiceUnavailable, SessionExpired, OSError) as e:
        errors += 1
        etype = type(e).__name__
        error_types[etype] = error_types.get(etype, 0) + 1
    except Neo4jError as e:
        errors += 1
        etype = type(e).__name__
        error_types[etype] = error_types.get(etype, 0) + 1
    except Exception as e:
        errors += 1
        etype = type(e).__name__
        error_types[etype] = error_types.get(etype, 0) + 1
        print(f"  [LOAD] Unexpected error: {etype}: {e}", file=sys.stderr)
    time.sleep(0.1)

# Output results as JSON for the parent script to parse
result = {"writes": writes, "errors": errors, "error_types": error_types}
print(f"LOAD_RESULT:{json.dumps(result)}")

# Cleanup
try:
    with driver.session() as s:
        s.run(f"DROP GRAPH {graph_name}").consume()
except Exception:
    pass

driver.close()
PYEOF
}

# ── Toxiproxy helpers ──────────────────────────────────────────────────
toxi_add_toxic() {
    local toxic_name="$1"
    local toxic_type="$2"
    local toxic_attrs="$3"
    log_info "Adding toxic: $toxic_name ($toxic_type)"
    curl -sf -X POST "$TOXIPROXY_API/proxies/redis/toxics" \
        -H 'Content-Type: application/json' \
        -d "{
            \"name\": \"$toxic_name\",
            \"type\": \"$toxic_type\",
            \"attributes\": $toxic_attrs
        }" || {
        log_fail "Failed to add toxic: $toxic_name"
        return 1
    }
    echo ""
}

toxi_remove_toxic() {
    local toxic_name="$1"
    log_info "Removing toxic: $toxic_name"
    curl -sf -X DELETE "$TOXIPROXY_API/proxies/redis/toxics/$toxic_name" || {
        log_info "Toxic $toxic_name may already be removed"
    }
}

toxi_disable_proxy() {
    log_info "Disabling redis proxy (full disconnect)"
    curl -sf -X POST "$TOXIPROXY_API/proxies/redis" \
        -H 'Content-Type: application/json' \
        -d '{"enabled": false}' || {
        log_fail "Failed to disable proxy"
        return 1
    }
}

toxi_enable_proxy() {
    log_info "Re-enabling redis proxy (healing partition)"
    curl -sf -X POST "$TOXIPROXY_API/proxies/redis" \
        -H 'Content-Type: application/json' \
        -d '{"enabled": true}' || {
        log_fail "Failed to enable proxy"
        return 1
    }
}

# ── Bolt query helper ─────────────────────────────────────────────────
# Returns 0 if the query succeeds, 1 if it fails with an error, 2 if it hangs.
bolt_query_with_timeout() {
    local uri="$1"
    local query="$2"
    local timeout_secs="${3:-10}"

    python3 -u - "$BOLT_PASSWORD" "$uri" "$query" "$timeout_secs" <<'PYEOF'
import sys
import signal
import time
from neo4j import GraphDatabase

password = sys.argv[1]
uri = sys.argv[2]
query = sys.argv[3]
timeout = int(sys.argv[4])

def alarm_handler(signum, frame):
    print("QUERY_RESULT:HANG", flush=True)
    sys.exit(2)

signal.signal(signal.SIGALRM, alarm_handler)
signal.alarm(timeout)

try:
    driver = GraphDatabase.driver(uri, auth=("neo4j", password),
                                  connection_timeout=5, max_transaction_retry_time=3)
    with driver.session() as s:
        result = s.run(query)
        records = [dict(r) for r in result]
    driver.close()
    print(f"QUERY_RESULT:OK:{records}", flush=True)
    sys.exit(0)
except Exception as e:
    print(f"QUERY_RESULT:ERROR:{type(e).__name__}:{e}", flush=True)
    sys.exit(1)
PYEOF
}

# ── Wait for service health ────────────────────────────────────────────
wait_for_health() {
    local url="$1"
    local name="$2"
    local max_wait="${3:-90}"
    for i in $(seq 1 "$max_wait"); do
        if curl -sf "$url" &>/dev/null; then
            log_info "$name is healthy (after ${i}s)"
            return 0
        fi
        if [ "$i" -eq "$max_wait" ]; then
            log_fail "$name did not become healthy within ${max_wait}s"
            return 1
        fi
        sleep 1
    done
}

# ── Main ───────────────────────────────────────────────────────────────
main() {
    local passed=0
    local failed=0

    echo "================================================================"
    echo "  Network Partition Test"
    echo "================================================================"

    cd "$PROJECT_DIR"

    # Ensure clean state
    cleanup 2>/dev/null || true

    # Start chaos test stack
    log_info "Building and starting chaos test stack..."
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" up -d --build

    # Wait for both Bolt instances
    wait_for_health "$BOLT1_HEALTH" "bolt-1" 120 || exit 1
    wait_for_health "$BOLT2_HEALTH" "bolt-2" 120 || exit 1

    # Verify toxiproxy is ready
    log_info "Verifying toxiproxy redis proxy..."
    curl -sf "$TOXIPROXY_API/proxies/redis" | python3 -c "
import sys, json
proxy = json.load(sys.stdin)
print(f'  Proxy: {proxy[\"name\"]} ({proxy[\"listen\"]} -> {proxy[\"upstream\"]})')
print(f'  Enabled: {proxy[\"enabled\"]}')
" || {
        log_fail "Toxiproxy redis proxy is not configured"
        exit 1
    }

    # ── Phase 1: Verify baseline works ──────────────────────────────────
    echo ""
    log_info "=== Phase 1: Baseline connectivity ==="
    bolt_query_with_timeout "$BOLT1_URI" "RETURN 1 AS n" 10
    PHASE1_EXIT=$?
    if [ $PHASE1_EXIT -eq 0 ]; then
        log_pass "Baseline query succeeds"
        passed=$((passed + 1))
    else
        log_fail "Baseline query failed (exit=$PHASE1_EXIT)"
        failed=$((failed + 1))
    fi

    # ── Phase 2: Add latency, verify queries still work ─────────────────
    echo ""
    log_info "=== Phase 2: High latency (500ms) ==="
    toxi_add_toxic "high_latency" "latency" '{"latency": 500, "jitter": 100}'
    sleep 2

    bolt_query_with_timeout "$BOLT1_URI" "RETURN 1 AS n" 15
    PHASE2_EXIT=$?
    if [ $PHASE2_EXIT -eq 0 ]; then
        log_pass "Query succeeds under 500ms latency"
        passed=$((passed + 1))
    elif [ $PHASE2_EXIT -eq 1 ]; then
        log_pass "Query returned error under latency (acceptable, not a hang)"
        passed=$((passed + 1))
    else
        log_fail "Query HUNG under latency (exit=$PHASE2_EXIT)"
        failed=$((failed + 1))
    fi

    toxi_remove_toxic "high_latency"
    sleep 1

    # ── Phase 3: Full disconnect via proxy disable ──────────────────────
    echo ""
    log_info "=== Phase 3: Full network partition ==="

    # Start background load before the partition
    LOAD_DURATION=30
    log_info "Starting background load for ${LOAD_DURATION}s..."
    LOAD_OUTPUT_FILE=$(mktemp)
    generate_load "$BOLT1_URI" "$LOAD_DURATION" > "$LOAD_OUTPUT_FILE" 2>&1 &
    LOAD_PID=$!
    sleep 3  # Let some writes succeed

    # Disable the proxy (full partition)
    toxi_disable_proxy
    log_info "Partition active, waiting 10s..."
    sleep 10

    # Verify queries fail with errors (not hangs)
    bolt_query_with_timeout "$BOLT1_URI" "RETURN 1 AS n" 10
    PHASE3_EXIT=$?
    if [ $PHASE3_EXIT -eq 1 ]; then
        log_pass "Query returned error during partition (correct behavior)"
        passed=$((passed + 1))
    elif [ $PHASE3_EXIT -eq 0 ]; then
        log_info "Query somehow succeeded during partition (cached connection?)"
        passed=$((passed + 1))
    else
        log_fail "Query HUNG during partition"
        failed=$((failed + 1))
    fi

    # ── Phase 4: Heal partition, verify recovery ────────────────────────
    echo ""
    log_info "=== Phase 4: Recovery after partition heals ==="
    toxi_enable_proxy
    log_info "Partition healed, waiting for recovery..."

    # Wait for load generator to finish
    wait "$LOAD_PID" 2>/dev/null || true
    LOAD_PID=""

    # Parse load results
    if [ -f "$LOAD_OUTPUT_FILE" ]; then
        LOAD_RESULT=$(grep "^LOAD_RESULT:" "$LOAD_OUTPUT_FILE" | head -1 | sed 's/^LOAD_RESULT://')
        if [ -n "$LOAD_RESULT" ]; then
            log_info "Load generator results: $LOAD_RESULT"
            LOAD_WRITES=$(echo "$LOAD_RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin)['writes'])")
            LOAD_ERRORS=$(echo "$LOAD_RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin)['errors'])")
            log_info "  Successful writes: $LOAD_WRITES"
            log_info "  Errors during partition: $LOAD_ERRORS"
        fi
        rm -f "$LOAD_OUTPUT_FILE"
    fi

    # Wait for Bolt to recover and verify a query works
    sleep 5
    RECOVERY_OK=false
    for i in $(seq 1 30); do
        bolt_query_with_timeout "$BOLT1_URI" "RETURN 1 AS n" 10
        if [ $? -eq 0 ]; then
            log_pass "Recovery: query succeeded after partition healed (after ${i}s)"
            RECOVERY_OK=true
            passed=$((passed + 1))
            break
        fi
        sleep 1
    done
    if [ "$RECOVERY_OK" = false ]; then
        log_fail "Bolt did not recover within 30s after partition healed"
        failed=$((failed + 1))
    fi

    # ── Phase 5: Verify bolt-2 also recovers ───────────────────────────
    echo ""
    log_info "=== Phase 5: Second instance recovery ==="
    BOLT2_OK=false
    for i in $(seq 1 15); do
        bolt_query_with_timeout "$BOLT2_URI" "RETURN 1 AS n" 10
        if [ $? -eq 0 ]; then
            log_pass "bolt-2 recovered after partition healed (after ${i}s)"
            BOLT2_OK=true
            passed=$((passed + 1))
            break
        fi
        sleep 1
    done
    if [ "$BOLT2_OK" = false ]; then
        log_fail "bolt-2 did not recover within 15s"
        failed=$((failed + 1))
    fi

    # ── Results ─────────────────────────────────────────────────────────
    echo ""
    echo "================================================================"
    echo "  Results"
    echo "================================================================"
    echo "  Passed: $passed"
    echo "  Failed: $failed"
    echo "================================================================"

    if [ "$failed" -eq 0 ]; then
        log_pass "All network partition tests passed"
        exit 0
    else
        log_fail "$failed test(s) failed"
        log_info "bolt-1 logs:"
        docker compose -f "$COMPOSE_FILE" logs --tail=30 bolt-1 2>/dev/null || true
        log_info "bolt-2 logs:"
        docker compose -f "$COMPOSE_FILE" logs --tail=30 bolt-2 2>/dev/null || true
        exit 1
    fi
}

main "$@"
