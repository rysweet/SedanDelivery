#!/usr/bin/env bash
# run_bolt_conformance.sh — Boot sedandelivery-bolt on a free port with the
# in-memory store and run the neo4rs-driver-based conformance test suite
# (`tests/bolt-driver-conformance`) against it.
#
# Usage: bash scripts/run_bolt_conformance.sh
#
# Env overrides:
#   CARGO_PROFILE  — "release" (default) or "debug"
#   BOLT_BIND_HOST — bind host (default 127.0.0.1)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

if [ -t 1 ]; then
    GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
else
    GREEN=''; YELLOW=''; RED=''; NC=''
fi
log()  { echo -e "${YELLOW}[bolt-conformance]${NC} $*"; }
ok()   { echo -e "${GREEN}[bolt-conformance]${NC} $*"; }
fail() { echo -e "${RED}[bolt-conformance]${NC} $*"; }

CARGO_PROFILE="${CARGO_PROFILE:-release}"
BOLT_BIND_HOST="${BOLT_BIND_HOST:-127.0.0.1}"

# Pick a free TCP port without racing: bind a transient listener, read its port,
# then drop it. Python is universally available on CI runners.
pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

PORT="$(pick_free_port)"
log "Selected free port: ${PORT}"

log "Building sedandelivery-bolt (${CARGO_PROFILE})..."
if [ "$CARGO_PROFILE" = "release" ]; then
    cargo build --release -p sedandelivery-bolt
    BIN="$PROJECT_DIR/target/release/sedandelivery-bolt"
else
    cargo build -p sedandelivery-bolt
    BIN="$PROJECT_DIR/target/debug/sedandelivery-bolt"
fi

LOG_FILE="$PROJECT_DIR/target/bolt-conformance-server.log"
mkdir -p "$(dirname "$LOG_FILE")"
: > "$LOG_FILE"

# Critical: do NOT export BOLT_PASSWORD so the server runs in no-auth mode and
# accepts the default neo4j/neo4j credentials neo4rs sends. Do NOT set
# GQL_REDIS_URL so the in-memory store is used (volatile, isolated per run).
SERVER_PID=
cleanup() {
    if [ -n "${SERVER_PID:-}" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        log "Stopping Bolt server (pid=${SERVER_PID})..."
        kill "$SERVER_PID" 2>/dev/null || true
        # give it a moment to drain
        for _ in 1 2 3 4 5; do
            kill -0 "$SERVER_PID" 2>/dev/null || break
            sleep 0.5
        done
        kill -9 "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    if [ "${PRINT_SERVER_LOG_ON_EXIT:-0}" = "1" ] && [ -f "$LOG_FILE" ]; then
        echo "----- server log (tail) -----"
        tail -n 100 "$LOG_FILE" || true
    fi
}
trap cleanup EXIT

log "Starting Bolt server on ${BOLT_BIND_HOST}:${PORT} (in-memory store, no auth)..."
env -u BOLT_PASSWORD -u GQL_REDIS_URL -u GQL_REDIS_CLUSTER_NODES \
    BOLT_BIND_ADDR="${BOLT_BIND_HOST}:${PORT}" \
    BOLT_HEALTH_BIND_ADDR="${BOLT_BIND_HOST}" \
    BOLT_HEALTH_PORT=0 \
    RUST_LOG="${RUST_LOG:-warn,sedandelivery_bolt=info}" \
    "$BIN" >>"$LOG_FILE" 2>&1 &
SERVER_PID=$!
log "Server pid: ${SERVER_PID}"

# Wait for the Bolt port to accept connections (max 30s).
log "Waiting for Bolt port to accept connections..."
READY=0
for i in $(seq 1 60); do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        fail "Server exited before becoming ready (after ${i} attempts)"
        PRINT_SERVER_LOG_ON_EXIT=1
        exit 1
    fi
    if (echo > "/dev/tcp/${BOLT_BIND_HOST}/${PORT}") >/dev/null 2>&1; then
        ok "Bolt port is accepting connections (after ${i} attempts)"
        READY=1
        break
    fi
    sleep 0.5
done
if [ "$READY" -ne 1 ]; then
    fail "Bolt port never became ready"
    PRINT_SERVER_LOG_ON_EXIT=1
    exit 1
fi

export BOLT_URI="bolt://${BOLT_BIND_HOST}:${PORT}"
log "Running conformance tests against ${BOLT_URI}..."
set +e
cargo test -p bolt-driver-conformance --tests -- --ignored --test-threads=1 --nocapture
RC=$?
set -e

if [ $RC -ne 0 ]; then
    fail "Conformance tests failed (rc=${RC})"
    PRINT_SERVER_LOG_ON_EXIT=1
    exit "$RC"
fi

ok "Bolt conformance tests passed"
