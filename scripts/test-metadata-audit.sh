#!/usr/bin/env bash
# test-metadata-audit.sh — Outside-in validator for Issue #756.
#
# Verifies from an operator's perspective that:
# - the admin audit route reports stale `_labels`, `_reltypes`, and `_propidx`
#   metadata on a Redis-backed API instance
# - `repair=true` prunes only the stale metadata and leaves a clean follow-up run
# - orphan-like `_deleting` state is reported but not auto-cleared
# - non-Redis API instances reject the route with HTTP 501

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
API_BIN="${API_BIN:-$PROJECT_DIR/target/debug/gql-api}"

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*" >&2; }

pick_free_port() {
    python3 - "$1" <<'PY'
import socket
import sys

preferred = int(sys.argv[1])


def bind_port(port: int) -> int | None:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind(("127.0.0.1", port))
    except OSError:
        sock.close()
        return None
    selected = sock.getsockname()[1]
    sock.close()
    return selected


selected = bind_port(preferred)
if selected is None:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind(("127.0.0.1", 0))
    selected = sock.getsockname()[1]
    sock.close()

print(selected)
PY
}

wait_for_http() {
    local url="$1"
    local max_attempts="${2:-60}"
    local attempt=0
    while [ "$attempt" -lt "$max_attempts" ]; do
        if curl -sf "$url" >/dev/null 2>&1; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    return 1
}

wait_for_redis() {
    local port="$1"
    local max_attempts="${2:-30}"
    local attempt=0
    while [ "$attempt" -lt "$max_attempts" ]; do
        if redis-cli -p "$port" ping >/dev/null 2>&1; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    return 1
}

post_json() {
    local url="$1"
    local body_file="$2"
    local payload="$3"
    curl -sS -o "$body_file" -w "%{http_code}" \
        -H "content-type: application/json" \
        -X POST "$url" \
        --data "$payload"
}

post_empty() {
    local url="$1"
    local body_file="$2"
    curl -sS -o "$body_file" -w "%{http_code}" -X POST "$url"
}

assert_status() {
    local actual="$1"
    local expected="$2"
    local context="$3"
    local body_file="${4:-}"
    if [ "$actual" != "$expected" ]; then
        log_fail "$context returned HTTP $actual (expected $expected)"
        if [ -n "$body_file" ] && [ -f "$body_file" ]; then
            cat "$body_file" >&2
        fi
        exit 1
    fi
}

WORK_DIR="$(mktemp -d "$PROJECT_DIR/.issue756-audit.XXXXXX")"
REDIS_PORT="${REDIS_PORT:-$(pick_free_port 16382)}"
REDIS_API_PORT="${REDIS_API_PORT:-$(pick_free_port 13001)}"
MEMORY_API_PORT="${MEMORY_API_PORT:-$(pick_free_port 13002)}"

REDIS_PID_FILE="$WORK_DIR/redis.pid"
REDIS_API_LOG="$WORK_DIR/gql-api-redis.log"
MEMORY_API_LOG="$WORK_DIR/gql-api-memory.log"
REDIS_URL="redis://127.0.0.1:${REDIS_PORT}/"
REDIS_API_BASE="http://127.0.0.1:${REDIS_API_PORT}"
MEMORY_API_BASE="http://127.0.0.1:${MEMORY_API_PORT}"

REDIS_API_PID=""
MEMORY_API_PID=""

cleanup() {
    if [ -n "$REDIS_API_PID" ] && kill -0 "$REDIS_API_PID" 2>/dev/null; then
        kill "$REDIS_API_PID" 2>/dev/null || true
        wait "$REDIS_API_PID" 2>/dev/null || true
    fi
    if [ -n "$MEMORY_API_PID" ] && kill -0 "$MEMORY_API_PID" 2>/dev/null; then
        kill "$MEMORY_API_PID" 2>/dev/null || true
        wait "$MEMORY_API_PID" 2>/dev/null || true
    fi
    redis-cli -p "$REDIS_PORT" shutdown nosave >/dev/null 2>&1 || true
    rm -rf "$WORK_DIR"
}

show_logs_on_error() {
    local line="$1"
    log_fail "Validator failed at line $line"
    if [ -f "$REDIS_API_LOG" ]; then
        log_info "Redis-backed gql-api log tail:"
        tail -50 "$REDIS_API_LOG" >&2 || true
    fi
    if [ -f "$MEMORY_API_LOG" ]; then
        log_info "In-memory gql-api log tail:"
        tail -50 "$MEMORY_API_LOG" >&2 || true
    fi
}

trap cleanup EXIT
trap 'show_logs_on_error $LINENO' ERR

if [ ! -x "$API_BIN" ]; then
    echo "ERROR: gql-api binary not found at $API_BIN" >&2
    echo "Build it first with: cargo build -p gql-api" >&2
    exit 1
fi

if ! command -v redis-server >/dev/null 2>&1; then
    echo "ERROR: redis-server is required" >&2
    exit 1
fi

if ! command -v redis-cli >/dev/null 2>&1; then
    echo "ERROR: redis-cli is required" >&2
    exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
    echo "ERROR: curl is required" >&2
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required" >&2
    exit 1
fi

log_info "Starting temporary Redis on port $REDIS_PORT"
redis-server \
    --save '' \
    --appendonly no \
    --port "$REDIS_PORT" \
    --daemonize yes \
    --pidfile "$REDIS_PID_FILE" \
    --dir "$WORK_DIR" >/dev/null

if ! wait_for_redis "$REDIS_PORT" 30; then
    log_fail "Redis did not become ready on port $REDIS_PORT"
    exit 1
fi

log_info "Starting Redis-backed gql-api on port $REDIS_API_PORT"
GQL_API_BIND_ADDR="127.0.0.1:${REDIS_API_PORT}" \
GQL_REDIS_URL="$REDIS_URL" \
RUST_LOG="${RUST_LOG:-warn}" \
"$API_BIN" >"$REDIS_API_LOG" 2>&1 &
REDIS_API_PID=$!

if ! wait_for_http "$REDIS_API_BASE/health" 60; then
    log_fail "Redis-backed gql-api did not become healthy"
    exit 1
fi

log_info "Starting in-memory gql-api on port $MEMORY_API_PORT"
GQL_API_BIND_ADDR="127.0.0.1:${MEMORY_API_PORT}" \
RUST_LOG="${RUST_LOG:-warn}" \
"$API_BIN" >"$MEMORY_API_LOG" 2>&1 &
MEMORY_API_PID=$!

if ! wait_for_http "$MEMORY_API_BASE/health" 60; then
    log_fail "In-memory gql-api did not become healthy"
    exit 1
fi

GRAPH="issue_756_metadata_audit"
ORPHAN_GRAPH="issue_756_orphaned_delete"
STALE_LABEL="GhostLabel"
STALE_RELTYPE="STALE_REL"
STALE_PROPIDX="outside-audit-namespace:${GRAPH}"

log_info "Creating Redis-backed fixture graphs"
status="$(post_json "$REDIS_API_BASE/graphs" "$WORK_DIR/create-graph.json" "{\"name\":\"$GRAPH\"}")"
assert_status "$status" "201" "create graph $GRAPH" "$WORK_DIR/create-graph.json"

status="$(post_json "$REDIS_API_BASE/graphs" "$WORK_DIR/create-orphan-graph.json" "{\"name\":\"$ORPHAN_GRAPH\"}")"
assert_status "$status" "201" "create graph $ORPHAN_GRAPH" "$WORK_DIR/create-orphan-graph.json"

redis-cli -p "$REDIS_PORT" SADD "G:{${GRAPH}}:_labels" "$STALE_LABEL" >/dev/null
redis-cli -p "$REDIS_PORT" SADD "G:{${GRAPH}}:_reltypes" "$STALE_RELTYPE" >/dev/null
redis-cli -p "$REDIS_PORT" SADD "G:{${GRAPH}}:_propidx" "$STALE_PROPIDX" >/dev/null
redis-cli -p "$REDIS_PORT" SET "G:{${ORPHAN_GRAPH}}:_deleting" "1" >/dev/null

status="$(post_empty "$REDIS_API_BASE/admin/graphs/${GRAPH}/audit" "$WORK_DIR/dry-run.json")"
assert_status "$status" "200" "dry-run metadata audit" "$WORK_DIR/dry-run.json"
python3 - "$WORK_DIR/dry-run.json" "$GRAPH" "$STALE_LABEL" "$STALE_RELTYPE" "$STALE_PROPIDX" <<'PY'
import json
import sys

path, graph, stale_label, stale_reltype, stale_propidx = sys.argv[1:]
with open(path, "r", encoding="utf-8") as fh:
    data = json.load(fh)

assert data["graph"] == graph, data
assert data["repair"] is False, data
assert data["staleLabels"] == [stale_label], data
assert data["staleReltypes"] == [stale_reltype], data
assert data["stalePropidx"] == [stale_propidx], data
assert data["deletingOrphaned"] is False, data
PY
log_pass "Dry run reported stale label, reltype, and propidx entries"

status="$(post_empty "$REDIS_API_BASE/admin/graphs/${GRAPH}/audit?repair=true" "$WORK_DIR/repair-run.json")"
assert_status "$status" "200" "repair metadata audit" "$WORK_DIR/repair-run.json"
python3 - "$WORK_DIR/repair-run.json" "$GRAPH" "$STALE_LABEL" "$STALE_RELTYPE" "$STALE_PROPIDX" <<'PY'
import json
import sys

path, graph, stale_label, stale_reltype, stale_propidx = sys.argv[1:]
with open(path, "r", encoding="utf-8") as fh:
    data = json.load(fh)

assert data["graph"] == graph, data
assert data["repair"] is True, data
assert data["staleLabels"] == [stale_label], data
assert data["staleReltypes"] == [stale_reltype], data
assert data["stalePropidx"] == [stale_propidx], data
assert data["deletingOrphaned"] is False, data
PY
log_pass "Repair run revalidated and pruned stale metadata"

status="$(post_empty "$REDIS_API_BASE/admin/graphs/${GRAPH}/audit" "$WORK_DIR/clean-run.json")"
assert_status "$status" "200" "follow-up clean metadata audit" "$WORK_DIR/clean-run.json"
python3 - "$WORK_DIR/clean-run.json" "$GRAPH" <<'PY'
import json
import sys

path, graph = sys.argv[1:]
with open(path, "r", encoding="utf-8") as fh:
    data = json.load(fh)

assert data["graph"] == graph, data
assert data["repair"] is False, data
assert data["staleLabels"] == [], data
assert data["staleReltypes"] == [], data
assert data["stalePropidx"] == [], data
assert data["deletingOrphaned"] is False, data
PY
log_pass "Follow-up dry run reported clean metadata"

status="$(post_empty "$REDIS_API_BASE/admin/graphs/${ORPHAN_GRAPH}/audit?repair=true" "$WORK_DIR/orphan-repair.json")"
assert_status "$status" "200" "orphaned deleting repair audit" "$WORK_DIR/orphan-repair.json"
python3 - "$WORK_DIR/orphan-repair.json" "$ORPHAN_GRAPH" <<'PY'
import json
import sys

path, graph = sys.argv[1:]
with open(path, "r", encoding="utf-8") as fh:
    data = json.load(fh)

assert data["graph"] == graph, data
assert data["repair"] is True, data
assert data["staleLabels"] == [], data
assert data["staleReltypes"] == [], data
assert data["stalePropidx"] == [], data
assert data["deletingOrphaned"] is True, data
PY

status="$(post_empty "$REDIS_API_BASE/admin/graphs/${ORPHAN_GRAPH}/audit" "$WORK_DIR/orphan-repeat.json")"
assert_status "$status" "200" "orphaned deleting repeat audit" "$WORK_DIR/orphan-repeat.json"
python3 - "$WORK_DIR/orphan-repeat.json" "$ORPHAN_GRAPH" <<'PY'
import json
import sys

path, graph = sys.argv[1:]
with open(path, "r", encoding="utf-8") as fh:
    data = json.load(fh)

assert data["graph"] == graph, data
assert data["repair"] is False, data
assert data["staleLabels"] == [], data
assert data["staleReltypes"] == [], data
assert data["stalePropidx"] == [], data
assert data["deletingOrphaned"] is True, data
PY
log_pass "Orphan-like deleting marker remained report-only across repeated audits"

status="$(post_empty "$MEMORY_API_BASE/admin/graphs/memory-only/audit" "$WORK_DIR/non-redis.json")"
assert_status "$status" "501" "non-Redis audit route" "$WORK_DIR/non-redis.json"
python3 - "$WORK_DIR/non-redis.json" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    data = json.load(fh)

assert "Redis-backed stores" in data["error"], data
PY
log_pass "Non-Redis backend rejected the audit route with 501"

log_pass "Metadata audit validator passed"
