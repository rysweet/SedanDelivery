#!/usr/bin/env bash
# run_stub_tests.sh - Run testkit stub tests (tests the Python driver protocol handling)
#
# Stub tests do NOT require sedan-bolt. They use a scripted bolt stub server
# to test the Python driver's Bolt protocol compliance. They are useful for
# verifying driver behavior but do not directly test sedan-bolt.
#
# Prerequisites:
#   1. Clone testkit:          git clone https://github.com/neo4j-drivers/testkit /tmp/testkit
#   2. Clone Python driver:    git clone https://github.com/neo4j/neo4j-python-driver /tmp/neo4j-python-driver
#   3. Install Python deps:    uv pip install neo4j ifaddr freezegun lark pytz
#   4. Install testkit deps:   uv pip install -r /tmp/testkit/requirements.txt
#
# Usage:
#   ./run_stub_tests.sh                    # Run all stub tests
#   ./run_stub_tests.sh transport          # Run only transport tests
#   ./run_stub_tests.sh errors basic_query # Run specific suites

set -euo pipefail

TESTKIT_DIR="${TESTKIT_DIR:-/tmp/testkit}"
PYTHON_DRIVER_DIR="${PYTHON_DRIVER_DIR:-/tmp/neo4j-python-driver}"
BACKEND_PORT="${BACKEND_PORT:-9876}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

BACKEND_PID=""

cleanup() {
    if [[ -n "$BACKEND_PID" ]] && kill -0 "$BACKEND_PID" 2>/dev/null; then
        kill "$BACKEND_PID" 2>/dev/null || true
        wait "$BACKEND_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# All stub test modules
ALL_STUB_SUITES=(
    "tests.stub.authorization.test_auth_token_manager"
    "tests.stub.authorization.test_authorization"
    "tests.stub.authorization.test_basic_auth_manager"
    "tests.stub.authorization.test_bearer_auth_manager"
    "tests.stub.authorization.test_user_switching"
    "tests.stub.basic_query.test_basic_query"
    "tests.stub.bookmarks.test_bookmarks_v3"
    "tests.stub.bookmarks.test_bookmarks_v4"
    "tests.stub.bookmarks.test_bookmarks_v5"
    "tests.stub.connectivity_check.test_get_server_info"
    "tests.stub.connectivity_check.test_verify_connectivity"
    "tests.stub.datatypes.test_temporal_types"
    "tests.stub.datatypes.test_unsupported_type"
    "tests.stub.datatypes.test_vector_types"
    "tests.stub.disconnects.test_disconnects"
    "tests.stub.driver_execute_query.test_driver_execute_query"
    "tests.stub.driver_parameters.test_bookmark_manager"
    "tests.stub.driver_parameters.test_client_agent_strings"
    "tests.stub.driver_parameters.test_max_connection_pool_size"
    "tests.stub.errors.test_errors"
    "tests.stub.session_run.test_session_run"
    "tests.stub.transport.test_handshakes"
    "tests.stub.transport.test_transport"
)

# Start testkit backend
if ! ss -tlnp 2>/dev/null | grep -q ":${BACKEND_PORT} "; then
    echo "Starting testkit backend..."
    (cd "$PYTHON_DRIVER_DIR" && \
        PYTHONPATH="$PYTHON_DRIVER_DIR" \
        python3 -m testkitbackend > /tmp/testkit-stub-backend.log 2>&1) &
    BACKEND_PID=$!
    sleep 2
fi

export TEST_DRIVER_NAME=python
export TEST_BACKEND_TIMEOUT=15
export PYTHONPATH=$TESTKIT_DIR

# Determine suites to run
if [[ $# -gt 0 ]]; then
    MODULES=()
    for s in "$@"; do
        # Find matching modules
        for m in "${ALL_STUB_SUITES[@]}"; do
            if [[ "$m" == *"$s"* ]]; then
                MODULES+=("$m")
            fi
        done
    done
else
    MODULES=("${ALL_STUB_SUITES[@]}")
fi

echo ""
echo "Running ${#MODULES[@]} stub test modules..."
echo ""

TOTAL_PASS=0
TOTAL_FAIL=0

for module in "${MODULES[@]}"; do
    set +e
    OUTPUT=$(cd "$TESTKIT_DIR" && python3 -m unittest "$module" -v 2>&1)
    RC=$?
    set -e

    ran_line=$(echo "$OUTPUT" | grep "^Ran " || echo "Ran 0 tests")
    ran=$(echo "$ran_line" | sed 's/Ran \([0-9]*\).*/\1/')
    failures=$(echo "$OUTPUT" | grep -oP 'failures=\K[0-9]+' 2>/dev/null || echo 0)
    errors=$(echo "$OUTPUT" | grep -oP 'errors=\K[0-9]+' 2>/dev/null || echo 0)
    passed=$((ran - failures - errors))

    if [[ $RC -eq 0 ]]; then
        echo -e "  ${GREEN}PASS${NC} $module ($ran tests)"
    else
        echo -e "  ${RED}FAIL${NC} $module ($passed/$ran passed)"
    fi

    TOTAL_PASS=$((TOTAL_PASS + passed))
    TOTAL_FAIL=$((TOTAL_FAIL + failures + errors))
done

echo ""
echo "Stub tests: $TOTAL_PASS passed, $TOTAL_FAIL failed"
