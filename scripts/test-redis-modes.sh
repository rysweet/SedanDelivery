#!/usr/bin/env bash
# test-redis-modes.sh — Smoke test all Redis/Valkey docker-compose configurations.
#
# Usage:
#   ./scripts/test-redis-modes.sh
#
# Starts each docker-compose configuration, runs a basic smoke test via the
# Bolt protocol (create graph, create node, query, delete), reports pass/fail,
# and cleans up containers.
#
# Requirements:
#   - Docker with compose v2 plugin
#   - cypher-shell OR python3 with neo4j driver OR curl (uses whichever is available)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors for output (disabled if not a terminal)
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

PASSED=0
FAILED=0
SKIPPED=0

if [ -z "${VALKEY_PASSWORD:-}" ]; then
    echo "ERROR: VALKEY_PASSWORD must be set before running compose-backed Redis/Valkey smoke tests."
    echo "Example: export VALKEY_PASSWORD='change-me-for-local-dev'"
    exit 1
fi
export VALKEY_PASSWORD

log_info()  { echo -e "${YELLOW}[INFO]${NC}  $*"; }
log_pass()  { echo -e "${GREEN}[PASS]${NC}  $*"; }
log_fail()  { echo -e "${RED}[FAIL]${NC}  $*"; }

# ── Bolt query helper ────────────────────────────────────────────────────────
# Tries multiple methods to send a Cypher query via Bolt protocol.
# Falls back to the HTTP health endpoint for basic connectivity if no Bolt
# client is available.

bolt_query() {
    local query="$1"
    local bolt_url="${2:-bolt://localhost:7687}"

    if command -v cypher-shell &>/dev/null; then
        echo "$query" | cypher-shell -a "$bolt_url" -u neo4j -p password --format plain 2>&1
        return $?
    fi

    if command -v python3 &>/dev/null && python3 -c "import neo4j" 2>/dev/null; then
        python3 -c "
from neo4j import GraphDatabase
driver = GraphDatabase.driver('$bolt_url', auth=('neo4j', 'password'))
with driver.session() as s:
    result = s.run('''$query''')
    for record in result:
        print(dict(record))
driver.close()
" 2>&1
        return $?
    fi

    # Fallback: use the REST health endpoint to confirm the server is up,
    # and report that Bolt client tools are not available.
    echo "WARNING: No Bolt client available (cypher-shell or python3+neo4j). Using HTTP health check only."
    curl -sf http://localhost:7688/health 2>&1
    return $?
}

# ── Wait for Bolt server ─────────────────────────────────────────────────────
wait_for_bolt() {
    local max_attempts="${1:-60}"
    local attempt=0
    log_info "Waiting for Bolt server health endpoint (max ${max_attempts}s)..."
    while [ $attempt -lt "$max_attempts" ]; do
        if curl -sf http://localhost:7688/health &>/dev/null; then
            log_info "Bolt server is healthy (after ${attempt}s)"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    log_fail "Bolt server did not become healthy within ${max_attempts}s"
    return 1
}

# ── Smoke test ────────────────────────────────────────────────────────────────
# Runs create graph -> create node -> query -> delete graph via Bolt.
run_smoke_test() {
    local mode_name="$1"
    log_info "Running smoke test for: $mode_name"

    # If no Bolt client is available, just verify HTTP health
    if ! command -v cypher-shell &>/dev/null && \
       ! (command -v python3 &>/dev/null && python3 -c "import neo4j" 2>/dev/null); then
        log_info "No Bolt client available; verifying HTTP health endpoint only"
        if curl -sf http://localhost:7688/health &>/dev/null; then
            log_pass "$mode_name — health check passed (no Bolt client for full test)"
            PASSED=$((PASSED + 1))
            return 0
        else
            log_fail "$mode_name — health check failed"
            FAILED=$((FAILED + 1))
            return 1
        fi
    fi

    local test_graph="__smoke_test_$$"
    local errors=0

    # Create graph
    if ! bolt_query "CREATE GRAPH $test_graph" &>/dev/null; then
        log_fail "$mode_name — failed to create graph"
        errors=$((errors + 1))
    fi

    # Create a node
    if ! bolt_query "USE $test_graph CREATE (:Person {name: 'Alice', age: 30})" &>/dev/null; then
        log_fail "$mode_name — failed to create node"
        errors=$((errors + 1))
    fi

    # Query the node
    local result
    result=$(bolt_query "USE $test_graph MATCH (p:Person) RETURN p.name, p.age" 2>&1) || true
    if echo "$result" | grep -q "Alice"; then
        log_info "Query returned expected data"
    else
        log_fail "$mode_name — query did not return expected data: $result"
        errors=$((errors + 1))
    fi

    # Delete graph (cleanup)
    bolt_query "DROP GRAPH $test_graph" &>/dev/null || true

    if [ $errors -eq 0 ]; then
        log_pass "$mode_name"
        PASSED=$((PASSED + 1))
    else
        log_fail "$mode_name — $errors error(s)"
        FAILED=$((FAILED + 1))
    fi
}

# ── Cleanup helper ────────────────────────────────────────────────────────────
cleanup() {
    local compose_file="$1"
    log_info "Cleaning up: $compose_file"
    VALKEY_PASSWORD="$VALKEY_PASSWORD" docker compose -f "$compose_file" down -v --remove-orphans --timeout 10 2>/dev/null || true
}

# ── Test a single mode ────────────────────────────────────────────────────────
test_mode() {
    local mode_name="$1"
    local compose_file="$2"
    local wait_time="${3:-90}"

    echo ""
    echo "================================================================"
    log_info "Testing mode: $mode_name"
    log_info "Compose file: $compose_file"
    log_info "Using caller-provided VALKEY_PASSWORD for compose-backed Redis/Valkey auth"
    echo "================================================================"

    if [ ! -f "$compose_file" ]; then
        log_fail "$mode_name — compose file not found: $compose_file"
        FAILED=$((FAILED + 1))
        return 1
    fi

    # Ensure clean state
    cleanup "$compose_file"

    # Start services
    log_info "Starting services..."
    if ! VALKEY_PASSWORD="$VALKEY_PASSWORD" docker compose -f "$compose_file" up -d --build 2>&1; then
        log_fail "$mode_name — docker compose up failed"
        FAILED=$((FAILED + 1))
        cleanup "$compose_file"
        return 1
    fi

    # Wait for bolt to be healthy
    if ! wait_for_bolt "$wait_time"; then
        log_fail "$mode_name — timeout waiting for Bolt server"
        log_info "Container logs:"
        VALKEY_PASSWORD="$VALKEY_PASSWORD" docker compose -f "$compose_file" logs --tail=30 sedandelivery-bolt 2>/dev/null || true
        FAILED=$((FAILED + 1))
        cleanup "$compose_file"
        return 1
    fi

    # Run smoke test
    run_smoke_test "$mode_name"

    # Cleanup
    cleanup "$compose_file"
}

# ── Main ──────────────────────────────────────────────────────────────────────
main() {
    echo "================================================================"
    echo "  SedanDelivery Redis Mode Smoke Tests"
    echo "================================================================"

    # Verify docker compose is available
    if ! docker compose version &>/dev/null; then
        echo "ERROR: docker compose v2 is required but not found."
        echo "Install: https://docs.docker.com/compose/install/"
        exit 1
    fi

    cd "$PROJECT_DIR"

    # Test 1: Standalone Valkey
    test_mode "Standalone Valkey" \
        "$PROJECT_DIR/docker-compose.yml" \
        90

    # Test 2: Redis Cluster (3 masters + 3 replicas)
    test_mode "Redis Cluster (6 nodes)" \
        "$PROJECT_DIR/docker-compose.cluster.yml" \
        120

    # Test 3: Master + Read Replicas
    test_mode "Master + 2 Read Replicas" \
        "$PROJECT_DIR/docker-compose.replica.yml" \
        90

    # ── Summary ──────────────────────────────────────────────────────────────
    echo ""
    echo "================================================================"
    echo "  Results"
    echo "================================================================"
    echo -e "  ${GREEN}Passed:${NC}  $PASSED"
    echo -e "  ${RED}Failed:${NC}  $FAILED"
    echo -e "  ${YELLOW}Skipped:${NC} $SKIPPED"
    echo "================================================================"

    if [ $FAILED -gt 0 ]; then
        exit 1
    fi
    exit 0
}

main "$@"
