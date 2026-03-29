#!/usr/bin/env bash
# test-bolt-deploy-secrets.sh — Outside-in validator for Issue #736.
#
# Verifies from an operator's perspective that:
# - the deploy workflow contract provisions and wires a reusable bolt-password secret
# - external Bolt deploys fail closed unless operator-managed TLS PEM secrets are present
# - a local production Bolt instance starts with in-process TLS and password auth
# - TLS handshakes and authenticated queries succeed
# - production refuses to start without TLS material

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BOLT_BIN="${BOLT_BIN:-$PROJECT_DIR/target/debug/sedandelivery-bolt}"
LOADTEST_BIN="${LOADTEST_BIN:-$PROJECT_DIR/target/debug/loadtest}"

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

wait_for_log() {
    local file="$1"
    local pattern="$2"
    local max_attempts="${3:-30}"
    local attempt=0
    while [ "$attempt" -lt "$max_attempts" ]; do
        if [ -f "$file" ] && grep -Fq "$pattern" "$file"; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    return 1
}

generate_tls_material() {
    cat >"$TLS_CONFIG_FILE" <<CFG
[req]
distinguished_name = req_distinguished_name
x509_extensions = v3_req
prompt = no

[req_distinguished_name]
CN = localhost

[v3_req]
subjectAltName = @alt_names

[alt_names]
DNS.1 = localhost
IP.1 = 127.0.0.1
CFG

    openssl req -x509 -nodes -newkey rsa:2048 -days 2 \
        -keyout "$TLS_KEY_FILE" \
        -out "$TLS_CERT_FILE" \
        -config "$TLS_CONFIG_FILE" >/dev/null 2>&1
}

run_bolt_python() {
    local mode="$1"
    python3 - "$mode" <<'PY'
import os
import sys

from neo4j import GraphDatabase

mode = sys.argv[1]
uri = os.environ["BOLT_URL"]
good_password = os.environ["BOLT_PASSWORD"]

if mode == "success":
    driver = GraphDatabase.driver(uri, auth=("neo4j", good_password))
    try:
        with driver.session() as session:
            value = session.run("RETURN 1 AS result").single(strict=True)["result"]
            if value != 1:
                raise SystemExit(f"expected result=1, got {value!r}")
    finally:
        driver.close()
elif mode == "wrong_password":
    bad_password = f"{good_password}-wrong"
    driver = GraphDatabase.driver(uri, auth=("neo4j", bad_password))
    try:
        with driver.session() as session:
            session.run("RETURN 1 AS result").single(strict=True)
    except Exception as exc:  # noqa: BLE001 - outside-in validator accepts driver-specific auth errors
        message = str(exc)
        if "unauthorized" not in message.lower() and "authentication" not in message.lower():
            raise
        print(message)
        raise SystemExit(17)
    finally:
        driver.close()
    raise SystemExit("wrong password unexpectedly succeeded")
else:
    raise SystemExit(f"unknown mode: {mode}")
PY
}

ensure_loadtest_bin() {
    if [ -x "$LOADTEST_BIN" ]; then
        return 0
    fi

    log_info "Python neo4j driver missing; building repo-native loadtest auth probe..."
    cargo build -q -p loadtest
}

run_bolt_loadtest() {
    local mode="$1"
    local auth_password="$BOLT_PASSWORD"
    local output_file="$WORK_DIR/loadtest-${mode}.log"
    local timeout_secs=30

    ensure_loadtest_bin

    if [ "$mode" = "wrong_password" ]; then
        auth_password="${BOLT_PASSWORD}-wrong"
        timeout_secs=8
    fi

    local status
    if timeout "$timeout_secs" "$LOADTEST_BIN" \
        --scenario nodes \
        --count 1 \
        --concurrency 1 \
        --skip-cleanup \
        --bolt-uri "$BOLT_URL" \
        --ca-cert "$TLS_CERT_FILE" \
        --user neo4j \
        --password "$auth_password" \
        --graph issue_736_auth_probe \
        --format json >"$output_file" 2>&1; then
        status=0
    else
        status=$?
    fi

    if [ "$mode" = "success" ]; then
        if [ "$status" -ne 0 ]; then
            cat "$output_file" >&2 || true
            return "$status"
        fi

        python3 - "$output_file" <<'PY'
import json
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text()
start = text.find("{")
end = text.rfind("}")
if start == -1 or end == -1 or end < start:
    raise SystemExit("loadtest success probe did not emit JSON output")
result = json.loads(text[start:end + 1])
if not result.get("count_matches"):
    raise SystemExit(f"loadtest success probe reported count_matches={result.get('count_matches')!r}")
if result.get("errors") not in (0, 0.0):
    raise SystemExit(f"loadtest success probe reported errors={result.get('errors')!r}")
PY
        return 0
    fi

    if [ "$status" -eq 0 ]; then
        cat "$output_file" >&2 || true
        echo "wrong password unexpectedly succeeded" >&2
        return 1
    fi

    if ! grep -Eqi 'auth|unauthor|credential' "$output_file"; then
        cat "$output_file" >&2 || true
        echo "wrong-password probe failed without an authentication-shaped error" >&2
        return 1
    fi

    return 17
}

run_bolt_probe() {
    local mode="$1"

    if python3 -c "import neo4j" >/dev/null 2>&1; then
        run_bolt_python "$mode"
        return $?
    fi

    run_bolt_loadtest "$mode"
}

check_deploy_workflow() {
    python3 - "$PROJECT_DIR/.github/workflows/deploy.yml" <<'PY'
from pathlib import Path
import subprocess
import sys

text = Path(sys.argv[1]).read_text()

# COUPLING WARNING: Every string in required_snippets must appear VERBATIM in
# deploy.yml. Update both files atomically. Never change one without the other.
required_snippets = {
    "TLS cert GitHub secret": 'DEPLOY_BOLT_TLS_CERT_PEM: ${{ secrets.DEPLOY_BOLT_TLS_CERT_PEM }}',
    "TLS key GitHub secret": 'DEPLOY_BOLT_TLS_KEY_PEM: ${{ secrets.DEPLOY_BOLT_TLS_KEY_PEM }}',
    "password reuse lookup": '--secret-name bolt-password',
    "password migration inventory check": 'properties.configuration.secrets[].name',
    "one-time password migration": 'Existing Bolt app has no bolt-password secret; generating one-time migration password',
    "fail closed on password lookup": 'Existing Bolt app has bolt-password configured but secret lookup failed',
    "cert secret wiring": 'bolt-tls-cert=${DEPLOY_BOLT_TLS_CERT_PEM}',
    "key secret wiring": 'bolt-tls-key=${DEPLOY_BOLT_TLS_KEY_PEM}',
    "secret volume mount": '--secret-volume-mount "${BOLT_SECRET_MOUNT_PATH}"',
    "mounted cert path": 'BOLT_TLS_CERT=${BOLT_TLS_CERT_PATH}',
    "mounted key path": 'BOLT_TLS_KEY=${BOLT_TLS_KEY_PATH}',
    "strict TLS env": 'BOLT_REQUIRE_TLS=1',
    "bolt fqdn output": 'echo "bolt_fqdn=${BOLT_FQDN}" >> "$GITHUB_OUTPUT"',
    "mask multiline cert": 'mask_multiline_secret "${DEPLOY_BOLT_TLS_CERT_PEM}"',
    "mask multiline key": 'mask_multiline_secret "${DEPLOY_BOLT_TLS_KEY_PEM}"',
    "non-branch expired cert fail closed": 'DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; provision a current certificate before deploying',
    "fail closed on expired cert": 'DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; provision a current certificate before deploying',
    "fail closed on missing certs": 'DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; provision TLS secrets before deploying',
    "driver smoke password lookup": 'Failed to resolve deployed bolt-password secret',
    "driver smoke password mask": 'echo "::add-mask::${BOLT_PASSWORD}"',
    "driver smoke pip install": 'python3 -m pip install --quiet neo4j',
    "driver smoke connectivity": 'driver.verify_connectivity()',
    "driver smoke query": 'session.run("RETURN 1 AS ok").single(strict=True)',
    "TLS driver example": '"bolt+ssc://${boltFqdn}:7687"',
    "trust override warning": 'This ACA-generated hostname does **not** provide production-authenticated',
    "internal short-hostname comment": 'Use short hostname (app name) for internal Container Apps communication',
    "cert freshness helper": 'cert_pem_is_current() {',
}

missing = [name for name, snippet in required_snippets.items() if snippet not in text]
if missing:
    raise SystemExit("missing deploy workflow snippets: " + ", ".join(missing))

# COUPLING WARNING: These patterns must NOT appear in deploy.yml.
# Any match causes CI to fail as a regression guard against re-introducing
# fallback/self-healing TLS behavior (issue #1021).
for label, forbidden in (
    ("no ephemeral cert generation", "openssl req -x509 -nodes -newkey"),
    ("no fallback function", "generate_bolt_tls_fallback"),
    ("no self-healing prose", "branch-environment fallback TLS"),
    ("no pr fallback", "self-signed TLS material for pull_request"),
    ("no insecure auth", "auth=None"),
    ("no plaintext bolt fqdn", "bolt://${fqdn}:7687"),
    ("no exposed redis port", "--exposed-port 6379"),
    ("no exposed bolt port", "--exposed-port 7687"),
):
    if forbidden in text:
        raise SystemExit(f"[{label}] forbidden pattern still present in deploy.yml: {forbidden!r}")

tls_block_start = text.index("BOLT_TLS_ENABLED=false")
tls_block_end = text.index("# Reuse the existing Bolt password", tls_block_start)
tls_block = text[tls_block_start:tls_block_end].rstrip()


def run_tls_case(name: str, *, cert_present: bool, cert_current: bool, expect_exit: int, expect_enabled: str | None) -> None:
    cert_value = "configured-cert" if cert_present else ""
    key_value = "configured-key" if cert_present else ""
    harness = f"""set -euo pipefail
mask_multiline_secret() {{ :; }}
cert_pem_is_current() {{
  if [[ "${{SIM_CERT_CURRENT}}" == "true" ]]; then
    return 0
  fi
  return 1
}}
DEPLOY_BOLT_TLS_CERT_PEM="{cert_value}"
DEPLOY_BOLT_TLS_KEY_PEM="{key_value}"
SIM_CERT_CURRENT={"true" if cert_current else "false"}
{tls_block}
printf 'enabled=%s\\n' "${{BOLT_TLS_ENABLED}}"
"""
    result = subprocess.run(["bash", "-lc", harness], capture_output=True, text=True)
    if result.returncode != expect_exit:
        raise SystemExit(
            f"{name}: expected exit {expect_exit}, got {result.returncode}\\nstdout:\\n{result.stdout}\\nstderr:\\n{result.stderr}"
        )
    if expect_enabled is not None and f"enabled={expect_enabled}" not in result.stdout:
        raise SystemExit(f"{name}: missing enabled={expect_enabled} in stdout\\n{result.stdout}")


run_tls_case(
    "branch env missing certs fails closed",
    cert_present=False,
    cert_current=False,
    expect_exit=1,
    expect_enabled=None,
)
run_tls_case(
    "branch env expired cert fails closed",
    cert_present=True,
    cert_current=False,
    expect_exit=1,
    expect_enabled=None,
)
run_tls_case(
    "non-branch expired cert fails closed",
    cert_present=True,
    cert_current=False,
    expect_exit=1,
    expect_enabled=None,
)
run_tls_case(
    "valid cert happy path",
    cert_present=True,
    cert_current=True,
    expect_exit=0,
    expect_enabled="true",
)
PY
}

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sedandelivery-issue736.XXXXXX")"
REDIS_PORT="${REDIS_PORT:-$(pick_free_port 16382)}"
BOLT_PORT="${BOLT_PORT:-$(pick_free_port 17687)}"
HEALTH_PORT="${HEALTH_PORT:-$(pick_free_port 17688)}"
MISSING_TLS_BOLT_PORT="${MISSING_TLS_BOLT_PORT:-$(pick_free_port 17689)}"
MISSING_TLS_HEALTH_PORT="${MISSING_TLS_HEALTH_PORT:-$(pick_free_port 17690)}"
BOLT_PASSWORD="${BOLT_PASSWORD:-issue-736-bolt-pass}"
TLS_CERT_FILE="$WORK_DIR/bolt-cert.pem"
TLS_KEY_FILE="$WORK_DIR/bolt-key.pem"
TLS_CONFIG_FILE="$WORK_DIR/openssl.cnf"
REDIS_PID_FILE="$WORK_DIR/redis.pid"
REDIS_LOG="$WORK_DIR/redis.log"
BOLT_LOG="$WORK_DIR/bolt.log"
MISSING_TLS_LOG="$WORK_DIR/bolt-missing-tls.log"
REDIS_URL="redis://127.0.0.1:${REDIS_PORT}/"
BOLT_URL="bolt+ssc://localhost:${BOLT_PORT}"
HEALTH_URL="http://127.0.0.1:${HEALTH_PORT}/health"
MISSING_TLS_PID=""
BOLT_PID=""

cleanup() {
    if [ -n "$MISSING_TLS_PID" ] && kill -0 "$MISSING_TLS_PID" 2>/dev/null; then
        kill "$MISSING_TLS_PID" 2>/dev/null || true
        wait "$MISSING_TLS_PID" 2>/dev/null || true
    fi
    if [ -n "$BOLT_PID" ] && kill -0 "$BOLT_PID" 2>/dev/null; then
        kill "$BOLT_PID" 2>/dev/null || true
        wait "$BOLT_PID" 2>/dev/null || true
    fi
    if [ -f "$REDIS_PID_FILE" ]; then
        REDIS_PID="$(tr -d '[:space:]' <"$REDIS_PID_FILE")"
        if [ -n "$REDIS_PID" ] && kill -0 "$REDIS_PID" 2>/dev/null; then
            redis-cli -p "$REDIS_PORT" shutdown nosave >/dev/null 2>&1 || true
            if kill -0 "$REDIS_PID" 2>/dev/null; then
                kill "$REDIS_PID" 2>/dev/null || true
                wait "$REDIS_PID" 2>/dev/null || true
            fi
        fi
    fi
    rm -rf "$WORK_DIR"
}

show_logs_on_error() {
    local line="$1"
    log_fail "Validator failed at line $line"
    if [ -f "$BOLT_LOG" ]; then
        log_info "Bolt log tail:"
        tail -80 "$BOLT_LOG" >&2 || true
    fi
    if [ -f "$MISSING_TLS_LOG" ]; then
        log_info "Missing-TLS log tail:"
        tail -80 "$MISSING_TLS_LOG" >&2 || true
    fi
    if [ -f "$REDIS_LOG" ]; then
        log_info "Redis log tail:"
        tail -50 "$REDIS_LOG" >&2 || true
    fi
}

trap cleanup EXIT
trap 'show_logs_on_error "$LINENO"' ERR

if [ ! -x "$BOLT_BIN" ]; then
    echo "ERROR: sedandelivery-bolt binary not found at $BOLT_BIN" >&2
    echo "Build it first with: cargo build -p sedandelivery-bolt" >&2
    exit 1
fi

for cmd in redis-server redis-cli curl openssl cargo; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "ERROR: $cmd is required" >&2
        exit 1
    fi
done

check_deploy_workflow
log_pass "Static deploy workflow contract reuses or generates bolt-password secret"
log_pass "Static deploy workflow contract requires operator-managed Bolt TLS secrets for external TCP"
log_pass "Static deploy workflow contract mounts TLS secrets and enables in-process Bolt TLS"

log_info "Starting isolated Redis for Bolt deploy validation..."
redis-server \
    --port "$REDIS_PORT" \
    --bind 127.0.0.1 \
    --save "" \
    --appendonly no \
    --dir "$WORK_DIR" \
    --pidfile "$REDIS_PID_FILE" \
    --logfile "$REDIS_LOG" \
    --daemonize yes

wait_for_redis "$REDIS_PORT" 30 || {
    log_fail "Redis did not become ready on port $REDIS_PORT"
    exit 1
}

generate_tls_material

log_info "Starting production Bolt with in-process TLS..."
(
    export ENVIRONMENT=production
    export GQL_REDIS_URL="$REDIS_URL"
    export BOLT_BIND_ADDR="127.0.0.1:${BOLT_PORT}"
    export BOLT_HEALTH_BIND_ADDR="127.0.0.1"
    export BOLT_HEALTH_PORT="$HEALTH_PORT"
    export BOLT_PASSWORD="$BOLT_PASSWORD"
    export BOLT_TLS_CERT="$TLS_CERT_FILE"
    export BOLT_TLS_KEY="$TLS_KEY_FILE"
    export BOLT_REQUIRE_TLS=1
    export RUST_LOG="${RUST_LOG:-info}"
    "$BOLT_BIN"
) >"$BOLT_LOG" 2>&1 &
BOLT_PID=$!

wait_for_http "$HEALTH_URL" 120 || {
    log_fail "Bolt health endpoint did not become ready at $HEALTH_URL"
    exit 1
}
wait_for_log "$BOLT_LOG" "TLS enabled for Bolt connections" 30 || {
    log_fail "Bolt did not report TLS startup"
    exit 1
}
if grep -Fq "credentials will be transmitted in plaintext" "$BOLT_LOG"; then
    log_fail "Bolt emitted a plaintext-credential warning despite TLS being configured"
    exit 1
fi
log_pass "Production Bolt starts with in-process TLS and password auth"

if ! timeout 10 openssl s_client \
    -connect "127.0.0.1:${BOLT_PORT}" \
    -servername localhost \
    -verify_return_error \
    -CAfile "$TLS_CERT_FILE" \
    </dev/null >"$WORK_DIR/openssl-handshake.log" 2>&1; then
    log_fail "TLS handshake failed"
    cat "$WORK_DIR/openssl-handshake.log" >&2 || true
    exit 1
fi
log_pass "TLS handshake succeeds with configured certificate"

BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" run_bolt_probe success
log_pass "Correct BOLT_PASSWORD authenticates successfully over TLS"

trap - ERR
set +e
BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" run_bolt_probe wrong_password >"$WORK_DIR/wrong-password.log" 2>&1
status=$?
set -e
trap 'show_logs_on_error "$LINENO"' ERR
if [ "$status" -ne 17 ]; then
    log_fail "Expected wrong-password probe to exit 17, got $status"
    cat "$WORK_DIR/wrong-password.log" >&2 || true
    exit 1
fi

BOLT_URL="$BOLT_URL" BOLT_PASSWORD="$BOLT_PASSWORD" run_bolt_probe success
log_pass "Incorrect BOLT_PASSWORD is rejected while valid credentials still work"

log_info "Starting production Bolt without TLS config..."
(
    unset BOLT_TLS_CERT BOLT_TLS_KEY BOLT_REQUIRE_TLS
    export ENVIRONMENT=production
    export GQL_REDIS_URL="$REDIS_URL"
    export BOLT_BIND_ADDR="127.0.0.1:${MISSING_TLS_BOLT_PORT}"
    export BOLT_HEALTH_BIND_ADDR="127.0.0.1"
    export BOLT_HEALTH_PORT="$MISSING_TLS_HEALTH_PORT"
    export BOLT_PASSWORD="$BOLT_PASSWORD"
    export RUST_LOG="${RUST_LOG:-info}"
    "$BOLT_BIN"
) >"$MISSING_TLS_LOG" 2>&1 &
MISSING_TLS_PID=$!

wait_for_process_exit "$MISSING_TLS_PID" 20 || {
    log_fail "Production instance without TLS did not exit"
    exit 1
}
wait "$MISSING_TLS_PID" || true
wait_for_log "$MISSING_TLS_LOG" "BOLT_TLS_CERT and BOLT_TLS_KEY are required when ENVIRONMENT=production" 10 || {
    log_fail "Missing production TLS failure message"
    exit 1
}
log_pass "Production Bolt refuses startup without TLS configuration"

log_pass "Bolt deploy secret validator passed"
