#!/usr/bin/env bash
# run_live_redis_tests.sh — Boot a Valkey/Redis cluster (3 masters) in Docker
# and run every `#[ignore]`-gated integration test under sd2-redis-store
# against it. Mirrors what `.github/workflows/live-redis.yml` does in CI so
# contributors can reproduce the run locally.
#
# Usage:
#   bash scripts/run_live_redis_tests.sh
#
# Env overrides:
#   VALKEY_IMAGE           — container image (default valkey/valkey:8)
#   CLUSTER_PORT_BASE      — first cluster port (default 7000); ports
#                            BASE..BASE+2 must be free
#   STANDALONE_PORT        — single-node port (default 6399)
#   KEEP_CONTAINERS        — set to 1 to skip cleanup (debugging)
#   SKIP_CLUSTER           — set to 1 to skip cluster boot/tests
#   SKIP_STANDALONE        — set to 1 to skip standalone boot/tests
#   CARGO_TEST_FLAGS       — extra flags appended to `cargo test`
#
# This script is intentionally modeled after scripts/run_bolt_conformance.sh:
# colored logging, robust trap-based cleanup, and a nonzero exit on the first
# failing test.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

if [ -t 1 ]; then
    GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
else
    GREEN=''; YELLOW=''; RED=''; NC=''
fi
log()  { echo -e "${YELLOW}[live-redis]${NC} $*"; }
ok()   { echo -e "${GREEN}[live-redis]${NC} $*"; }
fail() { echo -e "${RED}[live-redis]${NC} $*"; }

VALKEY_IMAGE="${VALKEY_IMAGE:-valkey/valkey:8}"
CLUSTER_PORT_BASE="${CLUSTER_PORT_BASE:-7000}"
STANDALONE_PORT="${STANDALONE_PORT:-6399}"
KEEP_CONTAINERS="${KEEP_CONTAINERS:-0}"
SKIP_CLUSTER="${SKIP_CLUSTER:-0}"
SKIP_STANDALONE="${SKIP_STANDALONE:-0}"
CARGO_TEST_FLAGS="${CARGO_TEST_FLAGS:-}"

RUN_TAG="sd2-live-$$-$(date +%s)"
STANDALONE_NAME="${RUN_TAG}-standalone"
CLUSTER_NAMES=()
for i in 0 1 2; do
    CLUSTER_NAMES+=("${RUN_TAG}-cluster-${i}")
done

if ! command -v docker >/dev/null 2>&1; then
    fail "docker is not installed; this script requires Docker."
    exit 2
fi

cleanup() {
    if [ "$KEEP_CONTAINERS" = "1" ]; then
        log "KEEP_CONTAINERS=1 — leaving containers running."
        log "Standalone : ${STANDALONE_NAME}"
        log "Cluster    : ${CLUSTER_NAMES[*]}"
        return
    fi
    log "Cleaning up containers..."
    docker rm -f "$STANDALONE_NAME" >/dev/null 2>&1 || true
    for name in "${CLUSTER_NAMES[@]}"; do
        docker rm -f "$name" >/dev/null 2>&1 || true
    done
}
trap cleanup EXIT

# Run redis-cli inside a throwaway container that uses host networking, so we
# don't need redis-cli installed on the host.
rcli() {
    docker run --rm --network host "$VALKEY_IMAGE" redis-cli "$@"
}

wait_for_ping() {
    local port="$1"
    local attempts="${2:-60}"
    for _ in $(seq 1 "$attempts"); do
        if rcli -p "$port" ping 2>/dev/null | grep -q PONG; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

OVERALL_RC=0

# ── Standalone Valkey ─────────────────────────────────────────────────────
if [ "$SKIP_STANDALONE" != "1" ]; then
    log "Starting standalone ${VALKEY_IMAGE} on port ${STANDALONE_PORT} (container ${STANDALONE_NAME})..."
    docker run -d --rm --name "$STANDALONE_NAME" \
        -p "${STANDALONE_PORT}:6379" \
        "$VALKEY_IMAGE" >/dev/null

    log "Waiting for standalone PING..."
    if ! wait_for_ping "$STANDALONE_PORT"; then
        fail "Standalone Valkey never responded to PING on port ${STANDALONE_PORT}"
        docker logs "$STANDALONE_NAME" 2>&1 | tail -40 || true
        exit 1
    fi
    ok "Standalone Valkey ready."

    log "Running --ignored tests against standalone (cluster_write_path, metadata_audit)..."
    set +e
    GQL_REDIS_URL="redis://127.0.0.1:${STANDALONE_PORT}" \
        cargo test -p sd2-redis-store --test cluster_write_path --test metadata_audit \
        -- --ignored --nocapture ${CARGO_TEST_FLAGS}
    RC=$?
    set -e
    if [ "$RC" -ne 0 ]; then
        fail "Standalone --ignored tests failed (rc=${RC})"
        OVERALL_RC=$RC
    else
        ok "Standalone --ignored tests passed."
    fi
fi

# ── Cluster (3 masters, no replicas) ──────────────────────────────────────
if [ "$SKIP_CLUSTER" != "1" ]; then
    log "Starting 3-node Valkey cluster (ports ${CLUSTER_PORT_BASE}..$((CLUSTER_PORT_BASE+2)))..."
    for i in 0 1 2; do
        port=$((CLUSTER_PORT_BASE + i))
        bus_port=$((port + 10000))
        name="${CLUSTER_NAMES[$i]}"
        docker run -d --rm --name "$name" --network host \
            "$VALKEY_IMAGE" \
            redis-server \
            --port "$port" \
            --cluster-enabled yes \
            --cluster-config-file "nodes-${port}.conf" \
            --cluster-node-timeout 5000 \
            --cluster-announce-ip 127.0.0.1 \
            --cluster-announce-port "$port" \
            --cluster-announce-bus-port "$bus_port" \
            --appendonly no \
            --bind 0.0.0.0 \
            --protected-mode no >/dev/null
    done

    log "Waiting for cluster nodes to respond to PING..."
    for i in 0 1 2; do
        port=$((CLUSTER_PORT_BASE + i))
        if ! wait_for_ping "$port"; then
            fail "Cluster node on port ${port} never responded to PING"
            for name in "${CLUSTER_NAMES[@]}"; do
                echo "----- logs for ${name} -----"
                docker logs "$name" 2>&1 | tail -30 || true
            done
            exit 1
        fi
    done

    log "Forming cluster via redis-cli --cluster create..."
    rcli --cluster create \
        "127.0.0.1:${CLUSTER_PORT_BASE}" \
        "127.0.0.1:$((CLUSTER_PORT_BASE+1))" \
        "127.0.0.1:$((CLUSTER_PORT_BASE+2))" \
        --cluster-replicas 0 --cluster-yes >/dev/null

    log "Waiting for cluster_state:ok..."
    READY=0
    for _ in $(seq 1 60); do
        if rcli -p "$CLUSTER_PORT_BASE" cluster info 2>/dev/null | grep -q "cluster_state:ok"; then
            READY=1
            break
        fi
        sleep 0.5
    done
    if [ "$READY" -ne 1 ]; then
        fail "Cluster never reached cluster_state:ok"
        rcli -p "$CLUSTER_PORT_BASE" cluster info || true
        exit 1
    fi
    ok "Cluster ready."

    NODES_CSV="redis://127.0.0.1:${CLUSTER_PORT_BASE},redis://127.0.0.1:$((CLUSTER_PORT_BASE+1)),redis://127.0.0.1:$((CLUSTER_PORT_BASE+2))"
    # cluster_pipeline.rs is the only test file that *requires* cluster mode
    # (it asserts GQL_REDIS_CLUSTER_NODES is set). The other --ignored test
    # files (cluster_write_path, metadata_audit) read GQL_REDIS_URL and run
    # against the standalone phase above.
    log "Running cluster-mode --ignored tests (GQL_REDIS_CLUSTER_NODES=${NODES_CSV})..."
    set +e
    GQL_REDIS_CLUSTER_NODES="$NODES_CSV" \
        cargo test -p sd2-redis-store --test cluster_pipeline \
        -- --ignored --nocapture ${CARGO_TEST_FLAGS}
    RC=$?
    set -e
    if [ "$RC" -ne 0 ]; then
        fail "Cluster --ignored tests failed (rc=${RC})"
        OVERALL_RC=$RC
    else
        ok "Cluster --ignored tests passed."
    fi
fi

if [ "$OVERALL_RC" -eq 0 ]; then
    ok "All live-Redis --ignored tests passed."
else
    fail "One or more live-Redis test groups failed (rc=${OVERALL_RC})."
fi
exit "$OVERALL_RC"
