#!/usr/bin/env bash
# test_run_tls_case_harness.sh — Mock harness for TLS decision block scenarios.
#
# Executes a local mock of the fail-closed TLS decision block with 6 test
# scenarios and verifies exit codes and BOLT_TLS_ENABLED values.
#
# This file is an executable specification of the expected TLS block behavior.
# It runs independently of deploy.yml (using a hardcoded mock) to prove the
# harness logic itself is correct before we rely on it to validate deploy.yml.
#
# Usage:
#   bash tests/test_run_tls_case_harness.sh
#
# Exit: 0 if all scenarios pass, 1 if any fail.

set -uo pipefail

PASS=0
FAIL=0

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# The mock TLS decision block — mirrors deploy.yml's fail-closed logic exactly.
# This is the authoritative reference implementation for the test harness.
MOCK_TLS_BLOCK='
mask_multiline_secret() { :; }
BOLT_TLS_ENABLED=false
if [[ -n "${DEPLOY_BOLT_TLS_CERT_PEM}" && -n "${DEPLOY_BOLT_TLS_KEY_PEM}" ]]; then
  if cert_pem_is_current "${DEPLOY_BOLT_TLS_CERT_PEM}"; then
    mask_multiline_secret "${DEPLOY_BOLT_TLS_CERT_PEM}"
    mask_multiline_secret "${DEPLOY_BOLT_TLS_KEY_PEM}"
    BOLT_TLS_ENABLED=true
  else
    echo "::error::DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; provision a current certificate before deploying"
    exit 1
  fi
else
  echo "::error::DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; provision TLS secrets before deploying"
  exit 1
fi
printf "enabled=%s\n" "${BOLT_TLS_ENABLED}"
'

run_scenario() {
    local name="$1"
    local cert_val="$2"
    local key_val="$3"
    local sim_current="$4"
    local expect_exit="$5"
    local expect_enabled="${6:-}"

    local harness
    harness="$(cat <<BASH
set -euo pipefail
cert_pem_is_current() {
  [[ "\${SIM_CERT_CURRENT}" == "true" ]]
}
DEPLOY_BOLT_TLS_CERT_PEM="${cert_val}"
DEPLOY_BOLT_TLS_KEY_PEM="${key_val}"
SIM_CERT_CURRENT="${sim_current}"
${MOCK_TLS_BLOCK}
BASH
)"

    local output exit_code
    output=$(bash -c "$harness" 2>&1) || exit_code=$?
    exit_code="${exit_code:-0}"

    local ok=true

    if [[ "$exit_code" -ne "$expect_exit" ]]; then
        echo -e "${RED}FAIL${NC} [$name]: expected exit=$expect_exit, got exit=$exit_code"
        echo "  output: $output"
        ok=false
    fi

    if [[ -n "$expect_enabled" ]] && ! echo "$output" | grep -q "enabled=${expect_enabled}"; then
        echo -e "${RED}FAIL${NC} [$name]: expected enabled=${expect_enabled} in output"
        echo "  output: $output"
        ok=false
    fi

    if [[ "$ok" == "true" ]]; then
        echo -e "${GREEN}PASS${NC} [$name]: exit=$exit_code$([ -n "$expect_enabled" ] && echo ", enabled=${expect_enabled}" || true)"
        ((PASS += 1))
    else
        ((FAIL += 1))
    fi
}

echo "=== TLS Fail-Closed Mock Harness ==="
echo ""

# Scenario 1: branch env missing certs → exit 1 (both vars empty)
run_scenario \
    "branch env missing certs fails closed" \
    "" "" "false" \
    1

# Scenario 2: branch env expired cert → exit 1 (cert present, current=false)
run_scenario \
    "branch env expired cert fails closed" \
    "configured-cert" "configured-key" "false" \
    1

# Scenario 3: non-branch expired cert → exit 1 (same behavior — no env_type distinction)
run_scenario \
    "non-branch expired cert fails closed" \
    "configured-cert" "configured-key" "false" \
    1

# Scenario 4: valid cert happy path → exit 0, BOLT_TLS_ENABLED=true
run_scenario \
    "valid cert happy path" \
    "configured-cert" "configured-key" "true" \
    0 "true"

# Scenario 5: cert present but key missing → exit 1 (missing certs path)
run_scenario \
    "cert only missing key fails closed" \
    "configured-cert" "" "true" \
    1

# Scenario 6: key present but cert missing → exit 1 (missing certs path)
run_scenario \
    "key only missing cert fails closed" \
    "" "configured-key" "true" \
    1

echo ""
echo "========================================"
echo "Mock Harness Results: ${PASS} pass, ${FAIL} fail"
echo "========================================"

[[ $FAIL -eq 0 ]]
