#!/usr/bin/env bash
# test-write-lock-metrics.sh — Outside-in validator for Issue #757.
#
# Verifies from an operator's perspective that:
# - transient write-lock contention is observable via Bolt /metrics
# - sustained contention increments contention-specific metrics without being
#   misclassified as a coordination-path backend failure
# - genuine Redis outages increment acquire-stage backend-failure metrics

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BOLT_BIN="${BOLT_BIN:-$PROJECT_DIR/target/debug/sedandelivery-bolt}"

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
        if curl -fsS "$url" >/dev/null 2>&1; then
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

load_redis_pid() {
    local max_attempts="${1:-1}"
    local attempt=0
    REDIS_PID=""
    while [ "$attempt" -lt "$max_attempts" ]; do
        if [ -f "$REDIS_PID_FILE" ]; then
            REDIS_PID="$(tr -d '[:space:]' <"$REDIS_PID_FILE")"
            if [ -n "$REDIS_PID" ]; then
                return 0
            fi
        fi
        attempt=$((attempt + 1))
        if [ "$attempt" -lt "$max_attempts" ]; then
            sleep 1
        fi
    done
    return 1
}

redis_instance_matches_port() {
    local dir
    dir="$(redis-cli -p "$REDIS_PORT" --raw CONFIG GET dir 2>/dev/null | tail -n1 || true)"
    [ "$dir" = "$WORK_DIR" ]
}

redis_instance_matches_pid() {
    local pid="$1"
    local exe_path
    local cwd_path
    local cmdline

    case "$pid" in
        ''|*[!0-9]*)
            return 1
            ;;
    esac

    if [ ! -e "/proc/$pid" ]; then
        return 1
    fi

    exe_path="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
    if [ "$(basename "$exe_path")" != "redis-server" ]; then
        return 1
    fi

    cwd_path="$(readlink -f "/proc/$pid/cwd" 2>/dev/null || true)"
    if [ "$cwd_path" = "$WORK_DIR" ]; then
        return 0
    fi

    cmdline="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    case "$cmdline" in
        *":$REDIS_PORT"*|*"--port $REDIS_PORT "*)
            return 0
            ;;
    esac

    return 1
}

wait_for_process_exit() {
    local pid="$1"
    local max_attempts="${2:-10}"
    local attempt=0
    while [ "$attempt" -lt "$max_attempts" ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    return 1
}

metric_value_from_text() {
    local metric="$1"
    local labels_json="${2:-[]}"
    python3 -c '
import json
import sys

metric = sys.argv[1]
labels = dict(json.loads(sys.argv[2]))
text = sys.stdin.read()

def parse_labels(raw: str) -> dict[str, str]:
    if not raw:
        return {}
    parsed: dict[str, str] = {}
    for part in raw.split(","):
        key, value = part.split("=", 1)
        parsed[key] = value.strip().strip("\"")
    return parsed

for line in text.splitlines():
    if not line or line.startswith("#") or not line.startswith(metric):
        continue
    rest = line[len(metric):]
    if rest and rest[0] not in {" ", "{"}:
        continue
    line_labels: dict[str, str] = {}
    if rest.startswith("{"):
        label_text = rest[1:].split("}", 1)[0]
        line_labels = parse_labels(label_text)
    if labels and any(line_labels.get(key) != value for key, value in labels.items()):
        continue
    if not labels and line_labels:
        continue
    try:
        print(float(line.split()[-1]))
        break
    except ValueError:
        continue
else:
    print(0.0)
' "$metric" "$labels_json"
}

metric_value() {
    local metric="$1"
    local labels_json="${2:-[]}"
    curl -fsS "$METRICS_URL" | metric_value_from_text "$metric" "$labels_json"
}

number_ge() {
    python3 - "$1" "$2" <<'PY'
import sys
current = float(sys.argv[1])
target = float(sys.argv[2])
raise SystemExit(0 if current >= target else 1)
PY
}

number_eq() {
    python3 - "$1" "$2" <<'PY'
import math
import sys
left = float(sys.argv[1])
right = float(sys.argv[2])
raise SystemExit(0 if math.isclose(left, right, rel_tol=0.0, abs_tol=1e-9) else 1)
PY
}

number_add() {
    python3 - "$1" "$2" <<'PY'
import sys
print(float(sys.argv[1]) + float(sys.argv[2]))
PY
}

wait_for_metric_delta() {
    local metric="$1"
    local before="$2"
    local delta="$3"
    local labels_json="${4:-[]}"
    local max_attempts="${5:-20}"
    local context="$6"
    local delay_secs="${7:-1}"
    local target
    local current="0"

    target="$(number_add "$before" "$delta")"
    for _ in $(seq 1 "$max_attempts"); do
        current="$(metric_value "$metric" "$labels_json")"
        if number_ge "$current" "$target"; then
            echo "$current"
            return 0
        fi
        sleep "$delay_secs"
    done

    log_fail "$context: metric $metric stayed below $target (last=$current)"
    show_logs_on_error "${BASH_LINENO[0]:-unknown}"
    return 1
}

assert_metric_unchanged() {
    local metric="$1"
    local before="$2"
    local labels_json="${3:-[]}"
    local context="$4"
    local current
    current="$(metric_value "$metric" "$labels_json")"
    if ! number_eq "$current" "$before"; then
        log_fail "$context: metric $metric changed from $before to $current"
        show_logs_on_error "${BASH_LINENO[0]:-unknown}"
        return 1
    fi
}

run_bolt_python() {
    local mode="$1"
    python3 - "$mode" <<'PY'
import os
import sys

from neo4j import GraphDatabase

mode = sys.argv[1]
uri = os.environ["BOLT_URL"]
password = os.environ["BOLT_PASSWORD"]
graph = os.environ["GRAPH"]

driver = GraphDatabase.driver(uri, auth=("neo4j", password))

try:
    if mode == "create_graph":
        with driver.session() as session:
            session.run(f"CREATE GRAPH {graph}").consume()
    elif mode == "seed_node":
        node_name = os.environ["NODE_NAME"]
        with driver.session(database=graph) as session:
            session.run("CREATE (:MetricProbe {name: $name})", name=node_name).consume()
    elif mode == "create_index":
        with driver.session(database=graph) as session:
            session.run("CREATE INDEX FOR (n:MetricProbe) ON (n.name)").consume()
    elif mode == "create_node":
        node_name = os.environ["NODE_NAME"]
        with driver.session(database=graph) as session:
            session.run("CREATE (:MetricProbe {name: $name})", name=node_name).consume()
            count = session.run(
                "MATCH (n:MetricProbe {name: $name}) RETURN count(n) AS c",
                name=node_name,
            ).single(strict=True)["c"]
            if count != 1:
                raise SystemExit(f"expected one created MetricProbe node, got {count}")
    elif mode == "expect_write_failure":
        node_name = os.environ["NODE_NAME"]
        exit_code = int(os.environ["FAIL_EXIT_CODE"])
        try:
            with driver.session(database=graph) as session:
                session.run("CREATE (:MetricProbe {name: $name})", name=node_name).consume()
        except Exception as exc:  # noqa: BLE001 - external validator wants any failure
            print(f"EXPECTED_FAILURE: {exc}")
            raise SystemExit(exit_code)
        raise SystemExit("write unexpectedly succeeded")
    elif mode == "expect_create_graph_failure":
        exit_code = int(os.environ["FAIL_EXIT_CODE"])
        try:
            with driver.session() as session:
                session.run(f"CREATE GRAPH {graph}").consume()
        except Exception as exc:  # noqa: BLE001 - external validator wants any failure
            print(f"EXPECTED_FAILURE: {exc}")
            raise SystemExit(exit_code)
        raise SystemExit("CREATE GRAPH unexpectedly succeeded")
    else:
        raise SystemExit(f"unknown mode: {mode}")
finally:
    driver.close()
PY
}

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sedandelivery-issue757.XXXXXX")"
REDIS_PORT="${REDIS_PORT:-$(pick_free_port 16383)}"
BOLT_PORT="${BOLT_PORT:-$(pick_free_port 17687)}"
HEALTH_PORT="${HEALTH_PORT:-$(pick_free_port 17688)}"
BOLT_PASSWORD="${BOLT_PASSWORD:-issue-757-bolt-pass}"
GRAPH="issue757_$(python3 - <<'PY'
import uuid
print(uuid.uuid4().hex[:12])
PY
)"
LOCK_TOKEN="issue757-lock-token"
TRANSIENT_LOCK_TTL_MS=700
KEY_PREFIX="${GQL_REDIS_KEY_PREFIX:-}"
LOCK_KEY="${KEY_PREFIX}G:{${GRAPH}}:write_lock"
REDIS_PID_FILE="$WORK_DIR/redis.pid"
REDIS_LOG="$WORK_DIR/redis.log"
BOLT_LOG="$WORK_DIR/bolt.log"
SUCCESS_LOG="$WORK_DIR/success-write.log"
CONTENTION_FAIL_LOG="$WORK_DIR/contention-fail.log"
BACKEND_FAIL_LOG="$WORK_DIR/backend-fail.log"
REDIS_URL="redis://127.0.0.1:${REDIS_PORT}/"
BOLT_URL="bolt://127.0.0.1:${BOLT_PORT}"
METRICS_URL="http://127.0.0.1:${HEALTH_PORT}/metrics"
HEALTH_URL="http://127.0.0.1:${HEALTH_PORT}/health"

REDIS_PID=""
BOLT_PID=""

cleanup() {
    if [ -n "$BOLT_PID" ] && kill -0 "$BOLT_PID" 2>/dev/null; then
        kill "$BOLT_PID" 2>/dev/null || true
        wait "$BOLT_PID" 2>/dev/null || true
    fi
    load_redis_pid 3 || true
    if redis_instance_matches_port; then
        redis-cli -p "$REDIS_PORT" shutdown nosave >/dev/null 2>&1 || true
    fi
    load_redis_pid 1 || true
    if [ -n "$REDIS_PID" ] \
        && kill -0 "$REDIS_PID" 2>/dev/null \
        && redis_instance_matches_pid "$REDIS_PID"; then
        kill "$REDIS_PID" 2>/dev/null || true
        wait_for_process_exit "$REDIS_PID" 5 || true
    fi
    rm -rf "$WORK_DIR"
}

show_logs_on_error() {
    local line="$1"
    log_fail "Validator failed at line $line"
    if curl -fsS "$METRICS_URL" >/tmp/issue757-metrics.$$ 2>/dev/null; then
        log_info "Relevant /metrics lines:"
        grep -E '^(redis_write_lock|sedandelivery_bolt_backend_errors_total)' \
            /tmp/issue757-metrics.$$ >&2 || true
        rm -f /tmp/issue757-metrics.$$
    fi
    if [ -f "$BOLT_LOG" ]; then
        log_info "Bolt log tail:"
        tail -80 "$BOLT_LOG" >&2 || true
    fi
    if [ -f "$SUCCESS_LOG" ]; then
        log_info "Successful contention write output:"
        tail -20 "$SUCCESS_LOG" >&2 || true
    fi
    if [ -f "$CONTENTION_FAIL_LOG" ]; then
        log_info "Contention failure write output:"
        tail -20 "$CONTENTION_FAIL_LOG" >&2 || true
    fi
    if [ -f "$BACKEND_FAIL_LOG" ]; then
        log_info "Backend failure write output:"
        tail -20 "$BACKEND_FAIL_LOG" >&2 || true
    fi
    if [ -f "$REDIS_LOG" ]; then
        log_info "Redis log tail:"
        tail -50 "$REDIS_LOG" >&2 || true
    fi
}

trap cleanup EXIT

if [ ! -x "$BOLT_BIN" ]; then
    echo "ERROR: sedandelivery-bolt binary not found at $BOLT_BIN" >&2
    echo "Build it first with: cargo build -p sedandelivery-bolt" >&2
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

if ! python3 -c "import neo4j" 2>/dev/null; then
    echo "ERROR: Python neo4j driver is required. Install with: pip install neo4j" >&2
    exit 1
fi

log_info "Starting isolated Redis for write-lock telemetry validation..."
redis-server \
    --port "$REDIS_PORT" \
    --bind 127.0.0.1 \
    --save "" \
    --appendonly no \
    --dir "$WORK_DIR" \
    --pidfile "$REDIS_PID_FILE" \
    --logfile "$REDIS_LOG" \
    --daemonize yes

load_redis_pid 5
wait_for_redis "$REDIS_PORT" 30 || {
    log_fail "Redis did not become ready on port $REDIS_PORT"
    exit 1
}

log_info "Starting Bolt with Redis backend..."
(
    export GQL_REDIS_URL="$REDIS_URL"
    export BOLT_BIND_ADDR="127.0.0.1:${BOLT_PORT}"
    export BOLT_HEALTH_PORT="$HEALTH_PORT"
    export BOLT_PASSWORD="$BOLT_PASSWORD"
    export RUST_LOG="${RUST_LOG:-info}"
    "$BOLT_BIN"
) >"$BOLT_LOG" 2>&1 &
BOLT_PID=$!

wait_for_http "$HEALTH_URL" 120 || {
    log_fail "Bolt health endpoint did not become ready at $HEALTH_URL"
    exit 1
}
wait_for_http "$METRICS_URL" 30 || {
    log_fail "Bolt metrics endpoint did not become ready at $METRICS_URL"
    exit 1
}

log_info "Seeding a graph and one MetricProbe node for index creation..."
BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" GRAPH="$GRAPH" \
    run_bolt_python create_graph
BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" GRAPH="$GRAPH" NODE_NAME="seed-node" \
    run_bolt_python seed_node

log_info "Capturing baseline metrics..."
before_attempts="$(metric_value "redis_write_lock_acquire_attempts_total")"
before_successes="$(metric_value "redis_write_lock_acquire_successes_total")"
before_contention="$(metric_value "redis_write_lock_acquire_contention_total")"
before_contention_failures="$(metric_value "redis_write_lock_contention_failures_total")"
before_release_successes="$(metric_value "redis_write_lock_release_successes_total")"
before_wait_count="$(metric_value "redis_write_lock_acquire_wait_seconds_count")"
before_hold_count="$(metric_value "redis_write_lock_hold_seconds_count")"
before_backend_gauge="$(metric_value "sedandelivery_bolt_backend_errors_total")"
before_backend_acquire="$(metric_value "redis_write_lock_backend_failures_total" '[["stage","acquire"]]')"

log_info "Forcing transient contention around CREATE INDEX with a short-lived seeded lock..."
redis-cli -p "$REDIS_PORT" SET "$LOCK_KEY" "$LOCK_TOKEN" NX PX "$TRANSIENT_LOCK_TTL_MS" >/dev/null
BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" GRAPH="$GRAPH" \
    run_bolt_python create_index >"$SUCCESS_LOG" 2>&1 &
success_pid=$!
if ! wait "$success_pid"; then
    log_fail "create_index failed unexpectedly under transient contention"
    show_logs_on_error "$LINENO"
    exit 1
fi

wait_for_metric_delta "redis_write_lock_acquire_attempts_total" "$before_attempts" "1" "[]" 20 \
    "transient contention acquire attempts" >/dev/null
wait_for_metric_delta "redis_write_lock_acquire_successes_total" "$before_successes" "1" "[]" 20 \
    "transient contention acquire successes" >/dev/null
wait_for_metric_delta "redis_write_lock_acquire_contention_total" "$before_contention" "1" "[]" 20 \
    "transient contention classification" >/dev/null
wait_for_metric_delta "redis_write_lock_release_successes_total" "$before_release_successes" "1" "[]" 20 \
    "transient contention release successes" >/dev/null
wait_for_metric_delta "redis_write_lock_acquire_wait_seconds_count" "$before_wait_count" "1" "[]" 20 \
    "transient contention wait histogram" >/dev/null
wait_for_metric_delta "redis_write_lock_hold_seconds_count" "$before_hold_count" "1" "[]" 20 \
    "transient contention hold histogram" >/dev/null

log_pass "Transient contention surfaced acquire, wait, hold, and release metrics"

assert_metric_unchanged "sedandelivery_bolt_backend_errors_total" "$before_backend_gauge" "[]" \
    "successful transient contention should not increment generic backend errors"
assert_metric_unchanged "redis_write_lock_contention_failures_total" "$before_contention_failures" "[]" \
    "successful transient contention should not increment contention-failure totals"
assert_metric_unchanged "redis_write_lock_backend_failures_total" "$before_backend_acquire" \
    '[["stage","acquire"]]' \
    "successful transient contention should not increment acquire-stage backend failures"

contention_fail_baseline_attempts="$(metric_value "redis_write_lock_acquire_attempts_total")"
contention_fail_baseline_contention="$(metric_value "redis_write_lock_acquire_contention_total")"
contention_fail_baseline_failures="$(metric_value "redis_write_lock_contention_failures_total")"
contention_fail_baseline_wait_count="$(metric_value "redis_write_lock_acquire_wait_seconds_count")"
contention_fail_baseline_backend_gauge="$(metric_value "sedandelivery_bolt_backend_errors_total")"
contention_fail_baseline_backend_acquire="$(metric_value "redis_write_lock_backend_failures_total" '[["stage","acquire"]]')"

log_info "Forcing sustained contention on a write query..."
redis-cli -p "$REDIS_PORT" SET "$LOCK_KEY" "$LOCK_TOKEN" NX PX 5000 >/dev/null
set +e
BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" GRAPH="$GRAPH" \
    NODE_NAME="contention-timeout-node" FAIL_EXIT_CODE="17" \
    run_bolt_python expect_write_failure >"$CONTENTION_FAIL_LOG" 2>&1
status=$?
set -e
if [ "$status" -eq 0 ]; then
    log_fail "Expected contention timeout write to fail, but it succeeded"
    show_logs_on_error "$LINENO"
    exit 1
fi
if [ "$status" -ne 17 ]; then
    log_fail "Expected contention timeout write to exit 17, got $status"
    cat "$CONTENTION_FAIL_LOG" >&2
    show_logs_on_error "$LINENO"
    exit 1
fi
redis-cli -p "$REDIS_PORT" DEL "$LOCK_KEY" >/dev/null

wait_for_metric_delta "redis_write_lock_acquire_attempts_total" "$contention_fail_baseline_attempts" "1" "[]" 20 \
    "contention-timeout acquire attempts" >/dev/null
wait_for_metric_delta "redis_write_lock_acquire_contention_total" "$contention_fail_baseline_contention" "1" "[]" 20 \
    "contention-timeout contention counter" >/dev/null
wait_for_metric_delta "redis_write_lock_contention_failures_total" "$contention_fail_baseline_failures" "1" "[]" 20 \
    "contention-timeout failure counter" >/dev/null
wait_for_metric_delta "redis_write_lock_acquire_wait_seconds_count" "$contention_fail_baseline_wait_count" "1" "[]" 20 \
    "contention-timeout wait histogram" >/dev/null
assert_metric_unchanged "sedandelivery_bolt_backend_errors_total" "$contention_fail_baseline_backend_gauge" "[]" \
    "contention timeout should not increment generic backend errors"
assert_metric_unchanged "redis_write_lock_backend_failures_total" "$contention_fail_baseline_backend_acquire" \
    '[["stage","acquire"]]' \
    "contention timeout should not increment acquire-stage backend failures"

log_pass "Sustained contention incremented contention-specific metrics without backend-failure noise"

backend_fail_baseline_gauge="$(metric_value "sedandelivery_bolt_backend_errors_total")"
backend_fail_baseline_acquire="$(metric_value "redis_write_lock_backend_failures_total" '[["stage","acquire"]]')"
backend_fail_baseline_contention_failures="$(metric_value "redis_write_lock_contention_failures_total")"
backend_fail_baseline_wait_count="$(metric_value "redis_write_lock_acquire_wait_seconds_count")"

log_info "Stopping Redis to force a real acquire-stage backend failure..."
redis-cli -p "$REDIS_PORT" shutdown nosave >/dev/null 2>&1 || true
load_redis_pid 1 || true
if [ -n "$REDIS_PID" ]; then
    wait_for_process_exit "$REDIS_PID" 10 || true
fi

set +e
BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" GRAPH="$GRAPH" \
    FAIL_EXIT_CODE="18" run_bolt_python expect_create_graph_failure >"$BACKEND_FAIL_LOG" 2>&1
status=$?
set -e
if [ "$status" -eq 0 ]; then
    log_fail "Expected backend-failure CREATE GRAPH to fail, but it succeeded"
    show_logs_on_error "$LINENO"
    exit 1
fi
if [ "$status" -ne 18 ]; then
    log_fail "Expected backend-failure CREATE GRAPH to exit 18, got $status"
    cat "$BACKEND_FAIL_LOG" >&2
    show_logs_on_error "$LINENO"
    exit 1
fi

wait_for_metric_delta "sedandelivery_bolt_backend_errors_total" "$backend_fail_baseline_gauge" "1" "[]" 20 \
    "generic backend error gauge after Redis outage" >/dev/null
wait_for_metric_delta "redis_write_lock_backend_failures_total" "$backend_fail_baseline_acquire" "1" \
    '[["stage","acquire"]]' 20 \
    "acquire-stage backend failure metric after Redis outage" >/dev/null
wait_for_metric_delta "redis_write_lock_acquire_wait_seconds_count" "$backend_fail_baseline_wait_count" "1" "[]" 20 \
    "acquire wait histogram after Redis outage" >/dev/null
assert_metric_unchanged "redis_write_lock_contention_failures_total" "$backend_fail_baseline_contention_failures" "[]" \
    "Redis outage should not look like additional lock contention failures"

log_pass "Redis outage incremented acquire-stage backend failure metrics separately from contention failures"
log_pass "Write-lock telemetry validator passed"
