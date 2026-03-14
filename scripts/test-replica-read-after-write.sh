#!/usr/bin/env bash
# test-replica-read-after-write.sh — Outside-in validator for Issue #737.
#
# Verifies from a user's perspective that:
# - a Bolt session can read its own writes while replicas are unavailable
# - a fresh session without recent writes cannot do the same
# - the 5-second read-after-write window expires and the original session
#   resumes the default replica-routed behavior

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SOURCE_COMPOSE_FILE="$PROJECT_DIR/docker-compose.replica.yml"

if [ -z "${VALKEY_PASSWORD:-}" ]; then
    echo "ERROR: VALKEY_PASSWORD must be set before running the replica read-after-write validator."
    echo "Example: export VALKEY_PASSWORD='change-me-for-local-dev'"
    exit 1
fi

export VALKEY_PASSWORD
export BOLT_PASSWORD="${BOLT_PASSWORD:-$VALKEY_PASSWORD}"

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

MASTER_HOST_PORT="${MASTER_HOST_PORT:-$(pick_free_port 16379)}"
REPLICA1_HOST_PORT="${REPLICA1_HOST_PORT:-$(pick_free_port 16380)}"
REPLICA2_HOST_PORT="${REPLICA2_HOST_PORT:-$(pick_free_port 16381)}"
BOLT_HOST_PORT="${BOLT_HOST_PORT:-$(pick_free_port 17687)}"
HEALTH_HOST_PORT="${HEALTH_HOST_PORT:-$(pick_free_port 17688)}"
COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-issue737-raw-$$}"
COMPOSE_FILE="$(mktemp "$PROJECT_DIR/.replica-compose.XXXXXX.yml")"

sed \
    -e "s/\"6379:6379\"/\"${MASTER_HOST_PORT}:6379\"/" \
    -e "s/\"6380:6379\"/\"${REPLICA1_HOST_PORT}:6379\"/" \
    -e "s/\"6381:6379\"/\"${REPLICA2_HOST_PORT}:6379\"/" \
    -e "s/\"7687:7687\"/\"${BOLT_HOST_PORT}:7687\"/" \
    -e "s/\"7688:7688\"/\"${HEALTH_HOST_PORT}:7688\"/" \
    "$SOURCE_COMPOSE_FILE" >"$COMPOSE_FILE"

export COMPOSE_FILE
export COMPOSE_PROJECT_NAME
export BOLT_URL="bolt://localhost:${BOLT_HOST_PORT}"
export BOLT_HEALTH_URL="http://localhost:${HEALTH_HOST_PORT}/health"

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }

teardown_stack() {
    docker compose -f "$COMPOSE_FILE" down -v --remove-orphans --timeout 10 >/dev/null 2>&1 || true
}

cleanup() {
    teardown_stack
    rm -f "$COMPOSE_FILE"
}

wait_for_bolt() {
    local max_attempts="${1:-90}"
    local attempt=0
    while [ "$attempt" -lt "$max_attempts" ]; do
        if curl -sf "$BOLT_HEALTH_URL" >/dev/null 2>&1; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    return 1
}

trap cleanup EXIT

if ! command -v docker >/dev/null 2>&1; then
    echo "ERROR: docker is required"
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required"
    exit 1
fi

python3 -c "import neo4j" >/dev/null 2>&1 || {
    echo "ERROR: python3 neo4j driver is required for this validator"
    exit 1
}

log_info "Starting replica topology from $SOURCE_COMPOSE_FILE"
log_info "Using host ports master=$MASTER_HOST_PORT replica1=$REPLICA1_HOST_PORT replica2=$REPLICA2_HOST_PORT bolt=$BOLT_HOST_PORT health=$HEALTH_HOST_PORT"
teardown_stack
docker compose -f "$COMPOSE_FILE" up -d --build >/dev/null

log_info "Waiting for Bolt health endpoint"
if ! wait_for_bolt 120; then
    log_fail "Bolt server did not become healthy"
    docker compose -f "$COMPOSE_FILE" logs --tail=50 sedandelivery-bolt || true
    exit 1
fi

python3 - <<'PY'
import os
import subprocess
import sys
import time

from neo4j import GraphDatabase


COMPOSE_FILE = os.environ["COMPOSE_FILE"]
COMPOSE_PROJECT_NAME = os.environ["COMPOSE_PROJECT_NAME"]
BOLT_PASSWORD = os.environ["BOLT_PASSWORD"]
BOLT_URL = os.environ["BOLT_URL"]
RUN_ID = f"wsb-{int(time.time())}"
DB_NAME = f"pinning_{int(time.time())}"
LABEL = "WsBPinTest"
REPLICA1_CONTAINER = f"{COMPOSE_PROJECT_NAME}-replica1-1"
REPLICA2_CONTAINER = f"{COMPOSE_PROJECT_NAME}-replica2-1"


def shell(cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True)


def stop_replicas():
    # Use direct container control instead of `docker compose kill/start` so the
    # validator matches the fastest and least disruptive restart path we know
    # passes under debug tracing.
    shell(["docker", "kill", REPLICA1_CONTAINER])
    shell(["docker", "kill", REPLICA2_CONTAINER])


def start_replicas():
    shell(["docker", "start", REPLICA1_CONTAINER])
    shell(["docker", "start", REPLICA2_CONTAINER])


def run_system(query):
    with driver_a.session(database="system") as system_session:
        system_session.run(query).consume()


def wait_for_database(database):
    for _ in range(30):
        try:
            with driver_a.session(database=database) as database_session:
                database_session.run("RETURN 1 AS ok").consume()
            return
        except Exception:
            time.sleep(1)
    raise AssertionError(f"database {database!r} did not become available")


def expect_failure(action, description):
    try:
        action()
    except Exception as exc:
        print("[INFO]", description, "failed as expected:", exc.__class__.__name__, str(exc))
        return
    raise AssertionError(f"{description} unexpectedly succeeded")


def read_value(session, key):
    result = session.run(
        f"MATCH (n:{LABEL} {{key: $key}}) RETURN n.value AS val",
        key=key,
    )
    record = result.single()
    if record is None:
        raise AssertionError("expected one row, got none")
    return record["val"]


driver_a = GraphDatabase.driver(BOLT_URL, auth=("neo4j", BOLT_PASSWORD))
driver_b = GraphDatabase.driver(BOLT_URL, auth=("neo4j", BOLT_PASSWORD))

try:
    run_system(f"DROP DATABASE {DB_NAME} IF EXISTS")
    run_system(f"CREATE DATABASE {DB_NAME} IF NOT EXISTS")
    wait_for_database(DB_NAME)

    with driver_a.session(database=DB_NAME) as session_a, driver_b.session(database=DB_NAME) as session_b:
        session_a.run(
            f"CREATE (n:{LABEL} {{key: $key, value: 42}}) RETURN n.value AS val",
            key=RUN_ID,
        ).consume()

        stop_replicas()

        immediate = read_value(session_a, RUN_ID)
        if immediate != 42:
            raise AssertionError(f"same-session immediate read returned {immediate!r}, expected 42")
        print("[INFO] same-session immediate read on a non-default database succeeded while replicas were stopped")

        expect_failure(
            lambda: read_value(session_b, RUN_ID),
            "fresh-session non-default-database read while replicas were stopped",
        )

        time.sleep(6)

        expect_failure(
            lambda: read_value(session_a, RUN_ID),
            "same-session non-default-database read after the 5-second window",
        )

    start_replicas()
    wait_for_database(DB_NAME)

    recovered = False
    for _ in range(30):
        try:
            with driver_b.session(database=DB_NAME) as recovery_session:
                if read_value(recovery_session, RUN_ID) == 42:
                    recovered = True
                    break
        except Exception:
            time.sleep(1)

    if not recovered:
        raise AssertionError("replica-backed read did not recover after restarting replicas")

    with driver_a.session(database=DB_NAME) as ddl_session:
        ddl_session.run(f"CREATE INDEX FOR (n:{LABEL}) ON (n.key)").consume()

        stop_replicas()

        immediate = read_value(ddl_session, RUN_ID)
        if immediate != 42:
            raise AssertionError(
                f"same-session read after CREATE INDEX returned {immediate!r}, expected 42"
            )
        print("[INFO] same-session read after CREATE INDEX succeeded while replicas were stopped")

        with driver_b.session(database=DB_NAME) as ddl_fresh_session:
            expect_failure(
                lambda: read_value(ddl_fresh_session, RUN_ID),
                "fresh-session read after CREATE INDEX while replicas were stopped",
            )

        time.sleep(6)

        expect_failure(
            lambda: read_value(ddl_session, RUN_ID),
            "same-session read after CREATE INDEX once the 5-second window expired",
        )

    start_replicas()
    wait_for_database(DB_NAME)

    print("[PASS] replica read-after-write pinning passed")
finally:
    try:
        run_system(f"DROP DATABASE {DB_NAME} IF EXISTS")
    except Exception as exc:
        print("[WARN] cleanup query failed:", exc, file=sys.stderr)

    driver_a.close()
    driver_b.close()
PY
