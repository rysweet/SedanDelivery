#!/usr/bin/env bash
#
# run_soak_tests.sh — execute the soak / memory-leak detection harness for
# the SedanDelivery embedded engines. See docs/SOAK_TESTING.md for the full
# rationale, the per-scenario leak budgets, and how to interpret the
# `[soak/scenarioN]` summary lines.
#
# Usage:
#   bash scripts/run_soak_tests.sh
#   SOAK_ITER_SCALE=0.1 bash scripts/run_soak_tests.sh   # quick local smoke
#   SOAK_ITER_SCALE=10  bash scripts/run_soak_tests.sh   # extended overnight
#
# Behaviour:
#   - Runs `cargo test -p soak --release -- --ignored --nocapture --test-threads=1`
#   - Wraps the entire run in a 30-minute hard timeout (default).
#   - Returns nonzero on any scenario failure (assertion, panic, or timeout).
#   - Prints a final summary with extracted `[soak/scenarioN]` lines.
#
# Exit codes:
#   0  — all scenarios pass.
#   1  — at least one scenario failed an assertion or panicked.
#   124 — the overall 30-minute timeout was hit (see GNU coreutils `timeout`).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

OVERALL_TIMEOUT="${SOAK_OVERALL_TIMEOUT:-1800}"   # 30 min default
ITER_SCALE="${SOAK_ITER_SCALE:-1.0}"

LOG_FILE="${SOAK_LOG_FILE:-${REPO_ROOT}/soak-output.log}"

echo "[soak] repo:           ${REPO_ROOT}"
echo "[soak] overall_timeout ${OVERALL_TIMEOUT}s"
echo "[soak] iter_scale:     ${ITER_SCALE}"
echo "[soak] log file:       ${LOG_FILE}"
echo "[soak] starting cargo test -p soak (release, --ignored)..."

set +e
SOAK_ITER_SCALE="${ITER_SCALE}" \
  timeout --foreground --kill-after=30s "${OVERALL_TIMEOUT}" \
  cargo test -p soak --release -- --ignored --nocapture --test-threads=1 \
  2>&1 | tee "${LOG_FILE}"
status=${PIPESTATUS[0]}
set -e

echo
echo "============================== soak summary =============================="
grep -E '\[soak/scenario[0-9]+\]' "${LOG_FILE}" || echo "(no scenario summary lines captured)"
echo "==========================================================================="

if [[ ${status} -eq 0 ]]; then
  echo "[soak] PASS"
elif [[ ${status} -eq 124 ]]; then
  echo "[soak] FAIL — overall timeout (${OVERALL_TIMEOUT}s) hit"
else
  echo "[soak] FAIL — exit status ${status}"
fi

exit ${status}
