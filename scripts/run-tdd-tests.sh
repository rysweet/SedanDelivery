#!/usr/bin/env bash
# run-tdd-tests.sh — Unified TDD test runner for issue #1021 TLS fail-closed.
#
# Runs all test suites in order and reports a combined pass/fail summary.
#
# Usage:
#   bash scripts/run-tdd-tests.sh [--python-only] [--shell-only]
#
# Options:
#   --python-only  Run only pytest suites
#   --shell-only   Run only shell test suites
#
# Exit: 0 if all suites pass, 1 if any suite fails.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

RUN_PYTHON=true
RUN_SHELL=true

for arg in "$@"; do
    case "$arg" in
        --python-only) RUN_SHELL=false ;;
        --shell-only)  RUN_PYTHON=false ;;
        *) echo "Unknown option: $arg" >&2; exit 1 ;;
    esac
done

SUITE_PASS=0
SUITE_FAIL=0

run_suite() {
    local name="$1"
    shift
    echo ""
    echo -e "${BOLD}── $name ──${NC}"
    if "$@"; then
        echo -e "${GREEN}✓ $name: PASS${NC}"
        ((SUITE_PASS += 1))
    else
        echo -e "${RED}✗ $name: FAIL${NC}"
        ((SUITE_FAIL += 1))
    fi
}

cd "$PROJECT_DIR"

# ── Shell test suites ─────────────────────────────────────────────────────────
if [[ "$RUN_SHELL" == "true" ]]; then
    run_suite "TLS Fail-Closed Shell Tests" \
        bash scripts/test-tls-fail-closed.sh

    run_suite "TLS Mock Harness Shell Tests" \
        bash tests/test_run_tls_case_harness.sh
fi

# ── Python (pytest) test suites ───────────────────────────────────────────────
if [[ "$RUN_PYTHON" == "true" ]]; then
    if ! command -v pytest &>/dev/null && ! python3 -m pytest --version &>/dev/null 2>&1; then
        echo -e "${YELLOW}SKIP${NC} pytest not available — install with: pip install pytest"
    else
        PYTEST_CMD="python3 -m pytest"
        if command -v pytest &>/dev/null; then
            PYTEST_CMD="pytest"
        fi

        run_suite "Deploy Workflow Validator (pytest)" \
            $PYTEST_CMD tests/test_deploy_workflow_validator.py -v --tb=short

        run_suite "TLS Block Extraction (pytest)" \
            $PYTEST_CMD tests/test_tls_block_extraction.py -v --tb=short

        run_suite "Deploy YML Static Analysis (pytest)" \
            $PYTEST_CMD tests/test_deploy_yml_static_analysis.py -v --tb=short
    fi
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}════════════════════════════════════════${NC}"
echo -e "${BOLD}TDD Suite Summary (issue #1021)${NC}"
echo -e "${BOLD}════════════════════════════════════════${NC}"
echo -e "Suites passed: ${GREEN}${SUITE_PASS}${NC}"
echo -e "Suites failed: ${RED}${SUITE_FAIL}${NC}"
echo -e "${BOLD}════════════════════════════════════════${NC}"

if [[ $SUITE_FAIL -gt 0 ]]; then
    echo -e "${RED}OVERALL: FAIL${NC}"
    exit 1
else
    echo -e "${GREEN}OVERALL: PASS${NC}"
    exit 0
fi
