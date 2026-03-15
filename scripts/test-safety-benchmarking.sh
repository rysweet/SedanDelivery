#!/usr/bin/env bash
# test-safety-benchmarking.sh — Outside-in validator for Issue #758.
#
# Verifies from an operator perspective that:
# - representative Redis store safety benchmark IDs run in standalone and cluster mode
# - the Bolt routing microbench is present
# - the public loadtest CLI reports topology throughput split fields with zero errors

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TMP_DIR="$(mktemp -d)"

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*" >&2; }

wait_for_http() {
    local url="$1"
    local max_attempts="${2:-120}"
    local attempt=0
    while [ "$attempt" -lt "$max_attempts" ]; do
        if curl -fsS "$url" >/dev/null 2>&1; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    return 1
}

json_value() {
    local file="$1"
    local path="$2"
    python3 - "$file" "$path" <<'PY'
import json
import sys

text = open(sys.argv[1], "r", encoding="utf-8").read()
start = text.find("{")
end = text.rfind("}")
if start == -1 or end < start:
    raise SystemExit("no JSON object found")
value = json.loads(text[start:end + 1])
for part in sys.argv[2].split("."):
    value = value[part]
if isinstance(value, bool):
    print("true" if value else "false")
else:
    print(value)
PY
}

require_json_eq() {
    local file="$1"
    local path="$2"
    local expected="$3"
    local context="$4"
    local actual
    actual="$(json_value "$file" "$path")"
    if [ "$actual" != "$expected" ]; then
        log_fail "$context: expected $path=$expected, got $actual"
        return 1
    fi
}

require_json_gt_zero() {
    local file="$1"
    local path="$2"
    local context="$3"
    python3 - "$file" "$path" "$context" <<'PY'
import json
import sys

text = open(sys.argv[1], "r", encoding="utf-8").read()
start = text.find("{")
end = text.rfind("}")
if start == -1 or end < start:
    raise SystemExit(f"{sys.argv[3]}: no JSON object found")
value = json.loads(text[start:end + 1])
for part in sys.argv[2].split("."):
    value = value[part]
if float(value) <= 0:
    raise SystemExit(f"{sys.argv[3]}: expected {sys.argv[2]} > 0, got {value}")
PY
}

run_capture() {
    local output_file="$1"
    local timeout_secs="$2"
    shift 2

    local status=0
    timeout --signal=TERM "$timeout_secs" "$@" >"$output_file" 2>&1 || status=$?
    if [ "$status" -eq 124 ]; then
        cat "$output_file" >&2
        log_fail "Command timed out after ${timeout_secs}s: $*"
        return 1
    fi
    if [ "$status" -ne 0 ]; then
        cat "$output_file" >&2
        return "$status"
    fi
}

benchmark_median() {
    local output_file="$1"
    python3 - "$output_file" <<'PY'
import re
import sys

text = open(sys.argv[1], "r", encoding="utf-8").read()
match = re.search(r"time:\s+\[([^\]]+)\]", text)
if not match:
    print("unknown")
    raise SystemExit(0)
parts = match.group(1).split()
print(" ".join(parts[2:4]) if len(parts) >= 4 else match.group(1))
PY
}

render_standalone_compose() {
    python3 - "$PROJECT_DIR/docker-compose.yml" "$TMP_DIR/standalone.compose.yml" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1]).read_text()
source = source.replace('"6379:6379"', '"127.0.0.1::6379"')
source = source.replace('"7687:7687"', '"127.0.0.1::7687"')
source = source.replace('"7688:7688"', '"127.0.0.1::7688"')
Path(sys.argv[2]).write_text(source)
PY
}

render_cluster_compose() {
    python3 - "$PROJECT_DIR/docker-compose.cluster.yml" "$TMP_DIR/cluster.compose.yml" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1]).read_text().splitlines()
dest = Path(sys.argv[2])
node_services = {
    "redis-node-1",
    "redis-node-2",
    "redis-node-3",
    "redis-node-4",
    "redis-node-5",
    "redis-node-6",
}

out = []
current_service = None
for line in source:
    stripped = line.strip()
    indent = len(line) - len(line.lstrip())
    if indent == 2 and stripped.endswith(":"):
        current_service = stripped[:-1]
    out.append(line)
    if current_service in node_services and stripped == "image: redis:7-bookworm":
        out.append(" " * indent + "ports:")
        out.append(" " * indent + '  - "127.0.0.1::6379"')
    if '"7687:7687"' in line:
        out[-1] = line.replace('"7687:7687"', '"127.0.0.1::7687"')
    if '"7688:7688"' in line:
        out[-1] = line.replace('"7688:7688"', '"127.0.0.1::7688"')

dest.write_text("\n".join(out) + "\n")
PY
}

compose_standalone() {
    docker compose \
        --project-directory "$PROJECT_DIR" \
        -p "$STANDALONE_PROJECT" \
        -f "$TMP_DIR/standalone.compose.yml" \
        "$@"
}

compose_cluster() {
    docker compose \
        --project-directory "$PROJECT_DIR" \
        -p "$CLUSTER_PROJECT" \
        -f "$TMP_DIR/cluster.compose.yml" \
        "$@"
}

mapped_port() {
    local topology="$1"
    local service="$2"
    local container_port="$3"
    local line

    if [ "$topology" = "standalone" ]; then
        line="$(compose_standalone port "$service" "$container_port" | head -n1)"
    else
        line="$(compose_cluster port "$service" "$container_port" | head -n1)"
    fi

    python3 - "$line" "$topology" "$service" "$container_port" <<'PY'
import sys

line = sys.argv[1].strip()
topology, service, container_port = sys.argv[2:]
if not line:
    raise SystemExit(
        f"no mapped host port found for {topology} {service}:{container_port}"
    )
print(line.rsplit(":", 1)[1])
PY
}

cleanup() {
    local standalone_output=""
    local cluster_output=""

    if ! standalone_output="$(compose_standalone down -v --remove-orphans --timeout 10 2>&1)"; then
        log_info "Standalone cleanup reported: ${standalone_output}"
    fi
    if ! cluster_output="$(compose_cluster down -v --remove-orphans --timeout 10 2>&1)"; then
        log_info "Cluster cleanup reported: ${cluster_output}"
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

export VALKEY_PASSWORD="${VALKEY_PASSWORD:-issue-758-local-pass}"
export BOLT_PASSWORD="${BOLT_PASSWORD:-$VALKEY_PASSWORD}"
export STANDALONE_PROJECT="issue758-standalone-$RANDOM"
export CLUSTER_PROJECT="issue758-cluster-$RANDOM"
export NODES_COUNT="${NODES_COUNT:-2000}"
export MIXED_DURATION="${MIXED_DURATION:-8}"
export MIXED_CONCURRENCY="${MIXED_CONCURRENCY:-8}"
export BENCH_TIMEOUT_SECS="${BENCH_TIMEOUT_SECS:-180}"
export NODES_TIMEOUT_SECS="${NODES_TIMEOUT_SECS:-180}"
export MIXED_TIMEOUT_SECS="${MIXED_TIMEOUT_SECS:-$((MIXED_DURATION + 30))}"

if ! docker compose version >/dev/null 2>&1; then
    echo "ERROR: docker compose v2 is required but not found."
    exit 1
fi

if ! cargo --version >/dev/null 2>&1; then
    echo "ERROR: cargo is required but not found."
    exit 1
fi

render_standalone_compose
render_cluster_compose

log_info "Building release loadtest binary"
cargo build --release -p loadtest >/dev/null

log_info "Running Bolt routing microbench"
BOLT_BENCH_OUTPUT="$TMP_DIR/bolt-routing.txt"
run_capture \
    "$BOLT_BENCH_OUTPUT" \
    "$BENCH_TIMEOUT_SECS" \
    cargo bench -p sedandelivery-bolt --bench bolt_benchmarks -- read_consistency_recent_write
grep -F "read_consistency_recent_write" "$BOLT_BENCH_OUTPUT" >/dev/null
log_pass "Bolt routing benchmark reported read_consistency_recent_write"

log_info "Starting isolated standalone benchmark environment"
compose_standalone up -d --build >/dev/null
STANDALONE_REDIS_PORT="$(mapped_port standalone valkey 6379)"
STANDALONE_BOLT_PORT="$(mapped_port standalone sedandelivery-bolt 7687)"
STANDALONE_HEALTH_PORT="$(mapped_port standalone sedandelivery-bolt 7688)"
wait_for_http "http://127.0.0.1:$STANDALONE_HEALTH_PORT/health" 180

STANDALONE_BENCH_LOCK="$TMP_DIR/standalone-lock.txt"
run_capture \
    "$STANDALONE_BENCH_LOCK" \
    "$BENCH_TIMEOUT_SECS" \
    env -u GQL_REDIS_CLUSTER_NODES -u REDIS_URL \
    GQL_REDIS_URL="redis://:${VALKEY_PASSWORD}@127.0.0.1:${STANDALONE_REDIS_PORT}" \
    cargo bench -p gql-redis-store --bench store_safety -- store_safety_lock/create_graph/standalone
grep -F "store_safety_lock/create_graph/standalone" "$STANDALONE_BENCH_LOCK" >/dev/null
STANDALONE_LOCK_MEDIAN="$(benchmark_median "$STANDALONE_BENCH_LOCK")"
log_pass "Standalone store benchmark emitted create_graph/standalone (median ${STANDALONE_LOCK_MEDIAN})"

STANDALONE_BENCH_INDEX="$TMP_DIR/standalone-index.txt"
run_capture \
    "$STANDALONE_BENCH_INDEX" \
    "$BENCH_TIMEOUT_SECS" \
    env -u GQL_REDIS_CLUSTER_NODES -u REDIS_URL \
    GQL_REDIS_URL="redis://:${VALKEY_PASSWORD}@127.0.0.1:${STANDALONE_REDIS_PORT}" \
    cargo bench -p gql-redis-store --bench store_safety -- store_safety_index/standalone/1000
grep -F "store_safety_index/standalone/1000" "$STANDALONE_BENCH_INDEX" >/dev/null
STANDALONE_INDEX_MEDIAN="$(benchmark_median "$STANDALONE_BENCH_INDEX")"
log_pass "Standalone store benchmark emitted index/standalone/1000 (median ${STANDALONE_INDEX_MEDIAN})"

STANDALONE_NODES_JSON="$TMP_DIR/standalone-nodes.json"
run_capture "$STANDALONE_NODES_JSON" "$NODES_TIMEOUT_SECS" \
    ./target/release/loadtest \
    --bolt-uri "bolt://127.0.0.1:${STANDALONE_BOLT_PORT}" \
    --user neo4j \
    --password "$BOLT_PASSWORD" \
    --scenario nodes \
    --graph issue758_validator_nodes_standalone \
    --count "$NODES_COUNT" \
    --concurrency 1 \
    --skip-cleanup \
    --format json
require_json_eq "$STANDALONE_NODES_JSON" "count_matches" "true" "standalone nodes"
require_json_eq "$STANDALONE_NODES_JSON" "errors" "0" "standalone nodes"
require_json_gt_zero "$STANDALONE_NODES_JSON" "throughput_nodes_per_sec" "standalone nodes"
log_pass "Standalone nodes loadtest reported count integrity and positive throughput"

STANDALONE_MIXED_JSON="$TMP_DIR/standalone-mixed.json"
run_capture "$STANDALONE_MIXED_JSON" "$MIXED_TIMEOUT_SECS" \
    ./target/release/loadtest \
    --bolt-uri "bolt://127.0.0.1:${STANDALONE_BOLT_PORT}" \
    --user neo4j \
    --password "$BOLT_PASSWORD" \
    --scenario mixed \
    --graph issue758_validator_mixed_standalone \
    --ratio 80 \
    --concurrency "$MIXED_CONCURRENCY" \
    --duration "$MIXED_DURATION" \
    --skip-cleanup \
    --format json
require_json_eq "$STANDALONE_MIXED_JSON" "read_errors" "0" "standalone mixed"
require_json_eq "$STANDALONE_MIXED_JSON" "write_errors" "0" "standalone mixed"
require_json_eq "$STANDALONE_MIXED_JSON" "backend_errors" "0" "standalone mixed"
require_json_gt_zero "$STANDALONE_MIXED_JSON" "total_ops_per_sec" "standalone mixed"
require_json_gt_zero "$STANDALONE_MIXED_JSON" "reads_per_sec" "standalone mixed"
require_json_gt_zero "$STANDALONE_MIXED_JSON" "writes_per_sec" "standalone mixed"
log_pass "Standalone mixed loadtest reported throughput split with zero errors"

log_info "Starting isolated cluster benchmark environment"
compose_cluster up -d --build >/dev/null
CLUSTER_BOLT_PORT="$(mapped_port cluster sedandelivery-bolt 7687)"
CLUSTER_HEALTH_PORT="$(mapped_port cluster sedandelivery-bolt 7688)"
CLUSTER_NODE_1_PORT="$(mapped_port cluster redis-node-1 6379)"
CLUSTER_NODE_2_PORT="$(mapped_port cluster redis-node-2 6379)"
CLUSTER_NODE_3_PORT="$(mapped_port cluster redis-node-3 6379)"
CLUSTER_NODE_4_PORT="$(mapped_port cluster redis-node-4 6379)"
CLUSTER_NODE_5_PORT="$(mapped_port cluster redis-node-5 6379)"
CLUSTER_NODE_6_PORT="$(mapped_port cluster redis-node-6 6379)"
wait_for_http "http://127.0.0.1:$CLUSTER_HEALTH_PORT/health" 240

CLUSTER_NODES="redis://:${VALKEY_PASSWORD}@127.0.0.1:${CLUSTER_NODE_1_PORT},redis://:${VALKEY_PASSWORD}@127.0.0.1:${CLUSTER_NODE_2_PORT},redis://:${VALKEY_PASSWORD}@127.0.0.1:${CLUSTER_NODE_3_PORT},redis://:${VALKEY_PASSWORD}@127.0.0.1:${CLUSTER_NODE_4_PORT},redis://:${VALKEY_PASSWORD}@127.0.0.1:${CLUSTER_NODE_5_PORT},redis://:${VALKEY_PASSWORD}@127.0.0.1:${CLUSTER_NODE_6_PORT}"

CLUSTER_BENCH_LOCK="$TMP_DIR/cluster-lock.txt"
run_capture \
    "$CLUSTER_BENCH_LOCK" \
    "$BENCH_TIMEOUT_SECS" \
    env GQL_REDIS_CLUSTER_NODES="$CLUSTER_NODES" \
    cargo bench -p gql-redis-store --bench store_safety -- store_safety_lock/create_graph/cluster
grep -F "store_safety_lock/create_graph/cluster" "$CLUSTER_BENCH_LOCK" >/dev/null
CLUSTER_LOCK_MEDIAN="$(benchmark_median "$CLUSTER_BENCH_LOCK")"
log_pass "Cluster store benchmark emitted create_graph/cluster (median ${CLUSTER_LOCK_MEDIAN})"

CLUSTER_BENCH_INDEX="$TMP_DIR/cluster-index.txt"
run_capture \
    "$CLUSTER_BENCH_INDEX" \
    "$BENCH_TIMEOUT_SECS" \
    env GQL_REDIS_CLUSTER_NODES="$CLUSTER_NODES" \
    cargo bench -p gql-redis-store --bench store_safety -- store_safety_index/cluster/1000
grep -F "store_safety_index/cluster/1000" "$CLUSTER_BENCH_INDEX" >/dev/null
CLUSTER_INDEX_MEDIAN="$(benchmark_median "$CLUSTER_BENCH_INDEX")"
log_pass "Cluster store benchmark emitted index/cluster/1000 (median ${CLUSTER_INDEX_MEDIAN})"

CLUSTER_NODES_JSON="$TMP_DIR/cluster-nodes.json"
run_capture "$CLUSTER_NODES_JSON" "$NODES_TIMEOUT_SECS" \
    ./target/release/loadtest \
    --bolt-uri "bolt://127.0.0.1:${CLUSTER_BOLT_PORT}" \
    --user neo4j \
    --password "$BOLT_PASSWORD" \
    --scenario nodes \
    --graph issue758_validator_nodes_cluster \
    --count "$NODES_COUNT" \
    --concurrency 1 \
    --skip-cleanup \
    --format json
require_json_eq "$CLUSTER_NODES_JSON" "count_matches" "true" "cluster nodes"
require_json_eq "$CLUSTER_NODES_JSON" "errors" "0" "cluster nodes"
require_json_gt_zero "$CLUSTER_NODES_JSON" "throughput_nodes_per_sec" "cluster nodes"
log_pass "Cluster nodes loadtest reported count integrity and positive throughput"

CLUSTER_MIXED_JSON="$TMP_DIR/cluster-mixed.json"
run_capture "$CLUSTER_MIXED_JSON" "$MIXED_TIMEOUT_SECS" \
    ./target/release/loadtest \
    --bolt-uri "bolt://127.0.0.1:${CLUSTER_BOLT_PORT}" \
    --user neo4j \
    --password "$BOLT_PASSWORD" \
    --scenario mixed \
    --graph issue758_validator_mixed_cluster \
    --ratio 80 \
    --concurrency "$MIXED_CONCURRENCY" \
    --duration "$MIXED_DURATION" \
    --skip-cleanup \
    --format json
require_json_eq "$CLUSTER_MIXED_JSON" "read_errors" "0" "cluster mixed"
require_json_eq "$CLUSTER_MIXED_JSON" "write_errors" "0" "cluster mixed"
require_json_eq "$CLUSTER_MIXED_JSON" "backend_errors" "0" "cluster mixed"
require_json_gt_zero "$CLUSTER_MIXED_JSON" "total_ops_per_sec" "cluster mixed"
require_json_gt_zero "$CLUSTER_MIXED_JSON" "reads_per_sec" "cluster mixed"
require_json_gt_zero "$CLUSTER_MIXED_JSON" "writes_per_sec" "cluster mixed"
log_pass "Cluster mixed loadtest reported throughput split with zero errors"

log_pass "Safety benchmarking validator passed"
