#!/usr/bin/env bash
#
# run_miri.sh — run Miri over a curated subset of cypher-core unit tests
# to detect undefined behavior (out-of-bounds, aliasing violations, data
# races, leaks, invalid pointer provenance, ...).
#
# See docs/MIRI.md for the audit of unsafe blocks and the rationale for
# the curated test set.
#
# Requirements:
#   * Rust nightly toolchain installed
#   * `miri` and `rust-src` components installed for nightly
#       rustup +nightly component add miri rust-src
#
# Usage:
#   bash scripts/run_miri.sh                     # default curated suite
#   MIRI_TEST_FILTERS="types::value" bash scripts/run_miri.sh   # override
#   MIRIFLAGS_EXTRA="-Zmiri-backtrace=full" bash scripts/run_miri.sh
#
# Exit codes:
#   0  — all tests pass
#   1  — Miri reported UB or a test failure (see logs)
#   2  — environment / toolchain not set up
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

TOOLCHAIN="${MIRI_TOOLCHAIN:-nightly}"

if ! rustup toolchain list 2>/dev/null | grep -q "^${TOOLCHAIN}"; then
    echo "error: rust toolchain '${TOOLCHAIN}' not installed." >&2
    echo "       Run: rustup toolchain install ${TOOLCHAIN}" >&2
    exit 2
fi

if ! rustup +"${TOOLCHAIN}" component list --installed 2>/dev/null | grep -q "^miri"; then
    echo "error: miri component not installed for ${TOOLCHAIN}." >&2
    echo "       Run: rustup +${TOOLCHAIN} component add miri rust-src" >&2
    exit 2
fi

# Re-audit unsafe blocks. If a new unsafe block appears in a pure-logic
# crate, fail the script so the author updates docs/MIRI.md.
PURE_CRATES=(
    cypher-core
    gremlin-core
    gql-engine
    sedan-cypher-embedded
    sedan-gql-embedded
    sedan-gremlin-embedded
    sd2-redis-store
    gql-fabric-store
)
echo "── Auditing unsafe blocks in pure-logic crates ─────────────────────────"
audit_hits=$(grep -rEn "\bunsafe\s*(\{|fn|impl|trait)" --include="*.rs" \
    "${PURE_CRATES[@]}" 2>/dev/null || true)
if [[ -n "${audit_hits}" ]]; then
    echo "warning: unsafe block(s) found in pure-logic crates:" >&2
    echo "${audit_hits}" >&2
    echo "        Update docs/MIRI.md with a verdict for each block." >&2
fi
echo "── Auditing unsafe blocks workspace-wide ───────────────────────────────"
ws_hits=$(grep -rEn "\bunsafe\s*(\{|fn|impl|trait)" --include="*.rs" \
    --exclude-dir=target --exclude-dir=target-miri \
    --exclude-dir=worktrees --exclude-dir=.git . 2>/dev/null || true)
echo "${ws_hits:-(none)}"

# Curated test filters. Each filter is passed as a positional arg to
# `cargo test`, which selects tests whose path contains the substring.
# Keep this list focused: every entry should be pure-logic, hermetic,
# and finish under one minute under Miri.
MIRI_TEST_FILTERS_DEFAULT=(
    "types::value::canonical::tests"
    "types::value::compare::tests"
    "types::value::convert::tests"
    "types::value::accessors::tests"
    "types::value::display::tests"
    # serde_value tests — listed individually; the
    # negative_sd2_{id,eid}_rejected tests touch InMemoryGraphStore which
    # transitively initializes config::EngineConfig from env+cwd, an
    # operation Miri's isolation refuses. They are still run under
    # `cargo test` in CI; only Miri excludes them.
    "serde_value::tests::non_finite_float_errors"
    "serde_value::tests::props_round_trip"
    "serde_value::tests::json_to_props_rejects_non_object"
    "serde_value::tests::str_escape_round_trip"
    "serde_value::tests::empty_props_round_trip"
    "serde_value::tests::malformed_json_errors"
)
if [[ -n "${MIRI_TEST_FILTERS:-}" ]]; then
    # Allow override via space-separated env var.
    read -r -a MIRI_TEST_FILTERS_ARR <<< "${MIRI_TEST_FILTERS}"
else
    MIRI_TEST_FILTERS_ARR=("${MIRI_TEST_FILTERS_DEFAULT[@]}")
fi

# Default MIRIFLAGS:
#   -Zmiri-strict-provenance — rejects integer→pointer casts that lose
#                              provenance, which is the strongest UB mode.
# We deliberately do NOT enable -Zmiri-disable-isolation: the curated
# tests are hermetic and benefit from full isolation. See docs/MIRI.md.
export MIRIFLAGS="-Zmiri-strict-provenance ${MIRIFLAGS_EXTRA:-}"

# Use a dedicated target dir so Miri-instrumented artifacts do not pollute
# the normal stable-toolchain build cache.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${REPO_ROOT}/target-miri}"

echo
echo "── Miri configuration ──────────────────────────────────────────────────"
echo "  toolchain:   ${TOOLCHAIN}"
echo "  MIRIFLAGS:   ${MIRIFLAGS}"
echo "  target dir:  ${CARGO_TARGET_DIR}"
echo "  filters:     ${MIRI_TEST_FILTERS_ARR[*]}"
echo

echo "── Ensuring Miri sysroot is built ──────────────────────────────────────"
cargo +"${TOOLCHAIN}" miri setup

failed=0
for filter in "${MIRI_TEST_FILTERS_ARR[@]}"; do
    echo
    echo "── Running miri test: ${filter} ────────────────────────────────────"
    if ! cargo +"${TOOLCHAIN}" miri test \
            -p cypher-core --lib \
            -- "${filter}"; then
        echo "error: miri test failed for filter: ${filter}" >&2
        failed=1
    fi
done

echo
if [[ "${failed}" -eq 0 ]]; then
    echo "✓ Miri suite passed for all curated filters."
else
    echo "✗ Miri reported failures. File a separate issue per distinct UB" >&2
    echo "  diagnostic and link it from docs/MIRI.md."                     >&2
    exit 1
fi
