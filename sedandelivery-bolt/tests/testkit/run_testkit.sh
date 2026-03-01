#!/usr/bin/env bash
# run_testkit.sh - Run neo4j-drivers/testkit conformance tests against sedandelivery-bolt
#
# Prerequisites:
#   1. Clone testkit:          git clone https://github.com/neo4j-drivers/testkit /tmp/testkit
#   2. Clone Python driver:    git clone https://github.com/neo4j/neo4j-python-driver /tmp/neo4j-python-driver
#   3. Install Python deps:    uv pip install neo4j ifaddr freezegun lark pytz
#   4. Install testkit deps:   uv pip install -r /tmp/testkit/requirements.txt
#   5. Build sedandelivery-bolt:       cargo build -p sedandelivery-bolt
#
# Usage:
#   ./run_testkit.sh                  # Run all integration tests
#   ./run_testkit.sh session_run      # Run only session_run tests
#   ./run_testkit.sh --passing-only   # Run only tests known to pass
#   ./run_testkit.sh --help           # Show help
#
# The script manages sedandelivery-bolt and testkit backend lifecycle automatically.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SD2_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SEDAN_BOLT_BIN="$SD2_ROOT/target/debug/sedandelivery-bolt"
TESTKIT_DIR="${TESTKIT_DIR:-/tmp/testkit}"
PYTHON_DRIVER_DIR="${PYTHON_DRIVER_DIR:-/tmp/neo4j-python-driver}"

BOLT_PORT="${BOLT_PORT:-7687}"
BACKEND_PORT="${BACKEND_PORT:-9876}"
BACKEND_TIMEOUT="${BACKEND_TIMEOUT:-600}"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

# --- Cleanup ---
SEDAN_BOLT_PID=""
BACKEND_PID=""

cleanup() {
    echo ""
    echo "Cleaning up..."
    if [[ -n "$SEDAN_BOLT_PID" ]] && kill -0 "$SEDAN_BOLT_PID" 2>/dev/null; then
        kill "$SEDAN_BOLT_PID" 2>/dev/null || true
        wait "$SEDAN_BOLT_PID" 2>/dev/null || true
    fi
    if [[ -n "$BACKEND_PID" ]] && kill -0 "$BACKEND_PID" 2>/dev/null; then
        kill "$BACKEND_PID" 2>/dev/null || true
        wait "$BACKEND_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- Test suites ---
ALL_SUITES=(
    "tests.neo4j.test_session_run"
    "tests.neo4j.test_tx_run"
    "tests.neo4j.test_tx_func_run"
    "tests.neo4j.test_authentication"
    "tests.neo4j.test_bookmarks"
    "tests.neo4j.test_summary"
    "tests.neo4j.test_direct_driver"
    "tests.neo4j.datatypes.test_datatypes"
    "tests.neo4j.datatypes.test_spatial_types"
    "tests.neo4j.datatypes.test_temporal_types"
)

# Tests known to pass against sedandelivery-bolt (as of 2026-02-28)
PASSING_TESTS=(
    # session_run (14/19 pass)
    "tests.neo4j.test_session_run.TestSessionRun.test_simple_query"
    "tests.neo4j.test_session_run.TestSessionRun.test_can_return_node"
    "tests.neo4j.test_session_run.TestSessionRun.test_can_return_relationship"
    "tests.neo4j.test_session_run.TestSessionRun.test_fails_on_bad_syntax"
    "tests.neo4j.test_session_run.TestSessionRun.test_iteration_larger_than_fetch_size"
    "tests.neo4j.test_session_run.TestSessionRun.test_iteration_smaller_than_fetch_size"
    "tests.neo4j.test_session_run.TestSessionRun.test_iteration_nested"
    "tests.neo4j.test_session_run.TestSessionRun.test_partial_iteration"
    "tests.neo4j.test_session_run.TestSessionRun.test_recover_from_fail_on_streaming"
    "tests.neo4j.test_session_run.TestSessionRun.test_recover_from_invalid_query"
    "tests.neo4j.test_session_run.TestSessionRun.test_regex_in_parameter"
    "tests.neo4j.test_session_run.TestSessionRun.test_regex_inline"
    "tests.neo4j.test_session_run.TestSessionRun.test_session_reuse"
    "tests.neo4j.test_session_run.TestSessionRun.test_updates_last_bookmark"

    # tx_run (15/21 pass)
    "tests.neo4j.test_tx_run.TestTxRun.test_simple_query"
    "tests.neo4j.test_tx_run.TestTxRun.test_can_commit_transaction"
    "tests.neo4j.test_tx_run.TestTxRun.test_consume_after_commit"
    "tests.neo4j.test_tx_run.TestTxRun.test_broken_transaction_should_not_break_session"
    "tests.neo4j.test_tx_run.TestTxRun.test_does_not_update_last_bookmark_on_failure"
    "tests.neo4j.test_tx_run.TestTxRun.test_does_not_update_last_bookmark_on_rollback"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_be_able_to_rollback_a_failure"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_fail_run_in_a_commited_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_fail_run_in_a_rollbacked_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_not_allow_run_on_a_commited_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_not_allow_run_on_a_rollbacked_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_not_commit_a_commited_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_not_commit_a_failure"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_not_rollback_a_commited_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_not_rollback_a_rollbacked_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_should_not_run_valid_query_in_invalid_tx"
    "tests.neo4j.test_tx_run.TestTxRun.test_updates_last_bookmark_on_commit"

    # tx_func_run (4/8 pass)
    "tests.neo4j.test_tx_func_run.TestTxFuncRun.test_simple_query"
    "tests.neo4j.test_tx_func_run.TestTxFuncRun.test_parameter"
    "tests.neo4j.test_tx_func_run.TestTxFuncRun.test_does_not_update_last_bookmark_on_rollback"
    "tests.neo4j.test_tx_func_run.TestTxFuncRun.test_updates_last_bookmark_on_commit"

    # authentication (2/4 pass)
    "tests.neo4j.test_authentication.TestAuthenticationBasic.test_success_on_basic_token"
    "tests.neo4j.test_authentication.TestAuthenticationBasic.test_success_on_provide_realm_with_basic_token"

    # bookmarks (3/8 pass)
    "tests.neo4j.test_bookmarks.TestBookmarks.test_can_handle_multiple_bookmarks"
    "tests.neo4j.test_bookmarks.TestBookmarks.test_can_obtain_bookmark_after_commit"
    "tests.neo4j.test_bookmarks.TestBookmarks.test_no_bookmark_after_rollback"

    # summary (9/14 pass)
    "tests.neo4j.test_summary.TestSummary.test_address"
    "tests.neo4j.test_summary.TestSummary.test_can_obtain_summary_after_consuming_result"
    "tests.neo4j.test_summary.TestSummary.test_contains_time_information"
    "tests.neo4j.test_summary.TestSummary.test_no_data_status"
    "tests.neo4j.test_summary.TestSummary.test_no_notification_info"
    "tests.neo4j.test_summary.TestSummary.test_no_plan_info"
    "tests.neo4j.test_summary.TestSummary.test_omitted_status"
    "tests.neo4j.test_summary.TestSummary.test_protocol_version_information"
    "tests.neo4j.test_summary.TestSummary.test_success_status"
    "tests.neo4j.test_summary.TestSummary.test_summary_counters_case_1"

    # direct_driver (2/6 pass, 3 skipped)
    "tests.neo4j.test_direct_driver.TestDirectDriver.test_custom_resolver"
    "tests.neo4j.test_direct_driver.TestDirectDriver.test_fail_nicely_when_using_http_port"

    # datatypes (8/11 pass, excludes path echo)
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_list_of_maps"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_map_of_lists"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_nested_lists"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_nested_map"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_node"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_relationship"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_very_long_list"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_very_long_map"
    "tests.neo4j.datatypes.test_datatypes.TestDataTypes.test_should_echo_very_long_string"

    # temporal (5+ pass)
    "tests.neo4j.datatypes.test_temporal_types.TestDataTypes.test_cypher_created_date"
    "tests.neo4j.datatypes.test_temporal_types.TestDataTypes.test_cypher_created_duration"
    "tests.neo4j.datatypes.test_temporal_types.TestDataTypes.test_should_echo_temporal_type"
    "tests.neo4j.datatypes.test_temporal_types.TestDataTypes.test_time_components"
    "tests.neo4j.datatypes.test_temporal_types.TestDataTypes.test_should_echo_all_timezone_ids"
)

usage() {
    echo "Usage: $0 [OPTIONS] [SUITE...]"
    echo ""
    echo "Run neo4j-drivers/testkit conformance tests against sedandelivery-bolt."
    echo ""
    echo "Options:"
    echo "  --passing-only    Run only tests known to pass"
    echo "  --verbose         Enable verbose output (TEST_DEBUG_REQRES=1)"
    echo "  --no-server       Don't start sedandelivery-bolt (assume already running)"
    echo "  --no-backend      Don't start testkit backend (assume already running)"
    echo "  --help            Show this help"
    echo ""
    echo "Suites (default: all):"
    echo "  session_run, tx_run, tx_func_run, authentication,"
    echo "  bookmarks, summary, direct_driver, datatypes,"
    echo "  spatial_types, temporal_types"
    echo ""
    echo "Examples:"
    echo "  $0                        # Run all integration tests"
    echo "  $0 session_run tx_run     # Run session_run and tx_run"
    echo "  $0 --passing-only         # Run only known-passing tests"
}

# --- Parse arguments ---
PASSING_ONLY=false
VERBOSE=false
START_SERVER=true
START_BACKEND=true
SUITES=()

while [[ $# -gt 0 ]]; do
    case $1 in
        --passing-only) PASSING_ONLY=true; shift ;;
        --verbose) VERBOSE=true; shift ;;
        --no-server) START_SERVER=false; shift ;;
        --no-backend) START_BACKEND=false; shift ;;
        --help) usage; exit 0 ;;
        -*) echo "Unknown option: $1"; usage; exit 1 ;;
        *) SUITES+=("$1"); shift ;;
    esac
done

# --- Preflight checks ---
if [[ ! -d "$TESTKIT_DIR" ]]; then
    echo -e "${RED}ERROR: testkit not found at $TESTKIT_DIR${NC}"
    echo "Clone it: git clone https://github.com/neo4j-drivers/testkit $TESTKIT_DIR"
    exit 1
fi

if [[ ! -d "$PYTHON_DRIVER_DIR" ]]; then
    echo -e "${RED}ERROR: neo4j-python-driver not found at $PYTHON_DRIVER_DIR${NC}"
    echo "Clone it: git clone https://github.com/neo4j/neo4j-python-driver $PYTHON_DRIVER_DIR"
    exit 1
fi

if [[ ! -f "$SEDAN_BOLT_BIN" ]]; then
    echo -e "${YELLOW}sedandelivery-bolt binary not found, building...${NC}"
    (cd "$SD2_ROOT" && cargo build -p sedandelivery-bolt)
fi

python3 -c "import neo4j" 2>/dev/null || {
    echo -e "${RED}ERROR: neo4j Python package not installed${NC}"
    echo "Install: uv pip install neo4j"
    exit 1
}

# --- Start sedandelivery-bolt ---
if $START_SERVER; then
    # Check if port is already in use
    if ss -tlnp 2>/dev/null | grep -q ":${BOLT_PORT} "; then
        echo -e "${YELLOW}Port $BOLT_PORT already in use, skipping sedandelivery-bolt start${NC}"
    else
        echo "Starting sedandelivery-bolt on port $BOLT_PORT..."
        BOLT_BIND_ADDR="127.0.0.1:${BOLT_PORT}" BOLT_PASSWORD=pass RUST_LOG=info \
            "$SEDAN_BOLT_BIN" > /tmp/sedandelivery-bolt-testkit.log 2>&1 &
        SEDAN_BOLT_PID=$!
        sleep 2
        if ! kill -0 "$SEDAN_BOLT_PID" 2>/dev/null; then
            echo -e "${RED}ERROR: sedandelivery-bolt failed to start${NC}"
            cat /tmp/sedandelivery-bolt-testkit.log
            exit 1
        fi
        echo -e "${GREEN}sedandelivery-bolt running (PID $SEDAN_BOLT_PID)${NC}"
    fi
fi

# --- Start testkit backend ---
if $START_BACKEND; then
    if ss -tlnp 2>/dev/null | grep -q ":${BACKEND_PORT} "; then
        echo -e "${YELLOW}Port $BACKEND_PORT already in use, skipping backend start${NC}"
    else
        echo "Starting testkit backend on port $BACKEND_PORT..."
        (cd "$PYTHON_DRIVER_DIR" && \
            PYTHONPATH="$PYTHON_DRIVER_DIR" \
            python3 -m testkitbackend > /tmp/testkit-backend.log 2>&1) &
        BACKEND_PID=$!
        sleep 2
        if ! kill -0 "$BACKEND_PID" 2>/dev/null; then
            echo -e "${RED}ERROR: testkit backend failed to start${NC}"
            cat /tmp/testkit-backend.log
            exit 1
        fi
        echo -e "${GREEN}testkit backend running (PID $BACKEND_PID)${NC}"
    fi
fi

# --- Environment ---
export TEST_DRIVER_NAME=python
export TEST_NEO4J_HOST=127.0.0.1
export TEST_NEO4J_PORT=$BOLT_PORT
export TEST_NEO4J_USER=neo4j
export TEST_NEO4J_PASS=pass
export TEST_NEO4J_SCHEME=bolt
export TEST_NEO4J_VERSION=4.4
export TEST_NEO4J_EDITION=community
export TEST_BACKEND_TIMEOUT=$BACKEND_TIMEOUT
export PYTHONPATH=$TESTKIT_DIR

if $VERBOSE; then
    export TEST_DEBUG_REQRES=1
fi

# --- Build test list ---
if $PASSING_ONLY; then
    echo ""
    echo -e "${GREEN}Running ${#PASSING_TESTS[@]} known-passing tests...${NC}"
    echo ""

    cd "$TESTKIT_DIR"
    python3 -m unittest "${PASSING_TESTS[@]}" -v 2>&1
    exit $?
fi

# Map suite names to module paths
suite_to_module() {
    case $1 in
        session_run) echo "tests.neo4j.test_session_run" ;;
        tx_run) echo "tests.neo4j.test_tx_run" ;;
        tx_func_run) echo "tests.neo4j.test_tx_func_run" ;;
        authentication) echo "tests.neo4j.test_authentication" ;;
        bookmarks) echo "tests.neo4j.test_bookmarks" ;;
        summary) echo "tests.neo4j.test_summary" ;;
        direct_driver) echo "tests.neo4j.test_direct_driver" ;;
        datatypes) echo "tests.neo4j.datatypes.test_datatypes" ;;
        spatial_types) echo "tests.neo4j.datatypes.test_spatial_types" ;;
        temporal_types) echo "tests.neo4j.datatypes.test_temporal_types" ;;
        *) echo "$1" ;;
    esac
}

if [[ ${#SUITES[@]} -eq 0 ]]; then
    MODULES=("${ALL_SUITES[@]}")
else
    MODULES=()
    for s in "${SUITES[@]}"; do
        MODULES+=("$(suite_to_module "$s")")
    done
fi

# --- Run tests ---
echo ""
echo "============================================"
echo " TestKit Conformance Tests vs sedandelivery-bolt"
echo "============================================"
echo " Suites: ${MODULES[*]}"
echo " Server: bolt://127.0.0.1:${BOLT_PORT}"
echo " Backend: 127.0.0.1:${BACKEND_PORT}"
echo "============================================"
echo ""

TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_ERROR=0
TOTAL_SKIP=0

for module in "${MODULES[@]}"; do
    echo -e "${YELLOW}--- Running: $module ---${NC}"

    set +e
    OUTPUT=$(cd "$TESTKIT_DIR" && python3 -m unittest "$module" -v 2>&1)
    RC=$?
    set -e

    echo "$OUTPUT" | grep -E "^test_|^Ran |^(OK|FAILED)" || true
    echo ""

    # Parse results from summary line
    if echo "$OUTPUT" | grep -q "^Ran "; then
        ran=$(echo "$OUTPUT" | grep "^Ran " | sed 's/Ran \([0-9]*\).*/\1/')
        failures=$(echo "$OUTPUT" | grep -oP 'failures=\K[0-9]+' || echo 0)
        errors=$(echo "$OUTPUT" | grep -oP 'errors=\K[0-9]+' || echo 0)
        skipped=$(echo "$OUTPUT" | grep -oP 'skipped=\K[0-9]+' || echo 0)
        passed=$((ran - failures - errors - skipped))

        TOTAL_PASS=$((TOTAL_PASS + passed))
        TOTAL_FAIL=$((TOTAL_FAIL + failures))
        TOTAL_ERROR=$((TOTAL_ERROR + errors))
        TOTAL_SKIP=$((TOTAL_SKIP + skipped))
    fi
done

echo ""
echo "============================================"
echo " TestKit Results Summary"
echo "============================================"
echo -e " ${GREEN}PASSED:  $TOTAL_PASS${NC}"
echo -e " ${RED}FAILED:  $TOTAL_FAIL${NC}"
echo -e " ${RED}ERRORS:  $TOTAL_ERROR${NC}"
echo -e " ${YELLOW}SKIPPED: $TOTAL_SKIP${NC}"
TOTAL=$((TOTAL_PASS + TOTAL_FAIL + TOTAL_ERROR + TOTAL_SKIP))
echo " TOTAL:   $TOTAL"
echo "============================================"

if [[ $TOTAL_FAIL -gt 0 ]] || [[ $TOTAL_ERROR -gt 0 ]]; then
    exit 1
fi
