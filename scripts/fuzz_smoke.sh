#!/usr/bin/env bash
#
# fuzz_smoke.sh — short fuzzing campaign across all parser fuzz targets.
#
# Runs each target for ~60 seconds via cargo-fuzz. Exits non-zero if any
# target reports a crash, slow input, or build failure.
#
# Requirements:
#   * Rust nightly toolchain installed (cargo-fuzz requires nightly)
#   * cargo-fuzz installed (`cargo install cargo-fuzz`)
#
# Usage:
#   bash scripts/fuzz_smoke.sh                # all targets, 60s each
#   FUZZ_TIME=300 bash scripts/fuzz_smoke.sh  # override duration
#   FUZZ_TARGETS="cypher_parser" bash scripts/fuzz_smoke.sh  # subset

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FUZZ_DIR="${REPO_ROOT}/fuzz"
FUZZ_TIME="${FUZZ_TIME:-60}"
FUZZ_TARGETS="${FUZZ_TARGETS:-cypher_parser gql_parser gremlin_text_parser}"
TOOLCHAIN="${FUZZ_TOOLCHAIN:-nightly}"

if ! command -v cargo-fuzz >/dev/null 2>&1; then
    echo "error: cargo-fuzz is not installed. Run: cargo install cargo-fuzz" >&2
    exit 2
fi

if ! rustup toolchain list 2>/dev/null | grep -q "^${TOOLCHAIN}"; then
    echo "error: rust toolchain '${TOOLCHAIN}' not installed." >&2
    echo "       Run: rustup toolchain install ${TOOLCHAIN}" >&2
    exit 2
fi

cd "${FUZZ_DIR}"

failed=0
for target in ${FUZZ_TARGETS}; do
    echo
    echo "============================================================"
    echo "  Fuzzing target: ${target}  (${FUZZ_TIME}s)"
    echo "============================================================"
    if ! cargo "+${TOOLCHAIN}" fuzz run "${target}" \
            -- -max_total_time="${FUZZ_TIME}" -print_final_stats=1; then
        echo "FAIL: target '${target}' reported a crash or build error." >&2
        failed=1
    fi
done

if [[ ${failed} -ne 0 ]]; then
    echo
    echo "fuzz smoke run FAILED — see crash artifacts under fuzz/artifacts/" >&2
    exit 1
fi

echo
echo "fuzz smoke run OK — no crashes in ${FUZZ_TIME}s × $(echo ${FUZZ_TARGETS} | wc -w) targets."
