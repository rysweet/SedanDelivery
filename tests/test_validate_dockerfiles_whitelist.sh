#!/usr/bin/env bash
# tests/test_validate_dockerfiles_whitelist.sh
#
# Verifies that scripts/validate-dockerfiles.sh correctly whitelists crates
# that are intentionally excluded from Docker builds (test/bench/example crates).
#
# These crates should NOT cause validation failures even though they lack
# COPY instructions in Dockerfiles:
#   tests/embedded-bench, tests/embedded-fuzz, tests/bolt-fuzz,
#   tests/concurrent-stress, tests/scalability, tests/soak,
#   tests/differential, examples/embedded_quickstart
#
# Closes #1772 (fix a)
#
# Usage:
#   bash tests/test_validate_dockerfiles_whitelist.sh

set -euo pipefail

ROOT="${VALIDATE_TEST_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
SCRIPT="$ROOT/scripts/validate-dockerfiles.sh"

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

echo "=== Test: validate-dockerfiles.sh whitelist ==="
echo ""

# ── Test 1: Whitelist array exists in the script ─────────────────────────────

echo "--- Test 1: DOCKER_WHITELIST array is defined ---"
if grep -q 'DOCKER_WHITELIST' "$SCRIPT"; then
    pass "DOCKER_WHITELIST array is defined"
else
    fail "DOCKER_WHITELIST array is NOT defined in $SCRIPT"
fi

# ── Test 2: All 8 expected crates are in the whitelist ───────────────────────

EXPECTED_WHITELIST=(
    "tests/embedded-bench"
    "tests/embedded-fuzz"
    "tests/bolt-fuzz"
    "tests/concurrent-stress"
    "tests/scalability"
    "tests/soak"
    "tests/differential"
    "examples/embedded_quickstart"
)

echo ""
echo "--- Test 2: All expected crates are whitelisted ---"
for crate in "${EXPECTED_WHITELIST[@]}"; do
    if grep -q "\"$crate\"" "$SCRIPT"; then
        pass "Whitelist contains $crate"
    else
        fail "Whitelist is missing $crate"
    fi
done

# ── Test 3: is_whitelisted helper function exists ────────────────────────────

echo ""
echo "--- Test 3: is_whitelisted helper exists ---"
if grep -q 'is_whitelisted' "$SCRIPT"; then
    pass "is_whitelisted function is defined"
else
    fail "is_whitelisted function is NOT defined"
fi

# ── Test 4: Validator passes when run against the real repo ──────────────────
# This is the integration test: the validator should exit 0 now that
# whitelisted crates are excluded.

echo ""
echo "--- Test 4: validate-dockerfiles.sh exits 0 (integration) ---"
if bash "$SCRIPT" > /dev/null 2>&1; then
    pass "validate-dockerfiles.sh exits 0"
else
    fail "validate-dockerfiles.sh exits non-zero (whitelist may be incomplete)"
fi

# ── Test 5: Whitelisted crates are NOT checked in Check 3 ────────────────────
# Run the validator and capture output. Whitelisted crates should not appear
# in any FAIL lines.

echo ""
echo "--- Test 5: No FAIL lines reference whitelisted crates ---"
validator_output=$(bash "$SCRIPT" 2>&1 || true)
whitelist_failures=0
for crate in "${EXPECTED_WHITELIST[@]}"; do
    if echo "$validator_output" | grep -q "FAIL.*${crate}"; then
        fail "Validator FAIL references whitelisted crate: $crate"
        whitelist_failures=$((whitelist_failures + 1))
    fi
done
if [ "$whitelist_failures" -eq 0 ]; then
    pass "No FAIL lines reference any whitelisted crate"
fi

# ── Test 6: Non-whitelisted crates ARE still checked ─────────────────────────
# Verify that the script still checks crates not in the whitelist (e.g.,
# tests/gremlin-tck, tests/loadtest are workspace members under tests/
# that should still be validated).

echo ""
echo "--- Test 6: Non-whitelisted test crates are still checked ---"
NON_WHITELISTED_TEST_CRATES=(
    "tests/gremlin-tck"
    "tests/loadtest"
)
for crate in "${NON_WHITELISTED_TEST_CRATES[@]}"; do
    # These should still appear in PASS or FAIL lines (i.e., they are checked)
    if echo "$validator_output" | grep -qE "(PASS|FAIL).*${crate}"; then
        pass "Non-whitelisted crate $crate is still validated"
    else
        fail "Non-whitelisted crate $crate is skipped (should be checked)"
    fi
done

# ── Test 7: Whitelist count matches expected ─────────────────────────────────

echo ""
echo "--- Test 7: Whitelist has exactly 9 entries ---"
# Count entries in the DOCKER_WHITELIST array
whitelist_entries=$(sed -n '/DOCKER_WHITELIST=/,/)/p' "$SCRIPT" 2>/dev/null | grep -c '"' || true)
whitelist_entries=${whitelist_entries:-0}
if [ "$whitelist_entries" -eq 9 ]; then
    pass "DOCKER_WHITELIST has exactly 9 entries"
else
    fail "DOCKER_WHITELIST has $whitelist_entries entries (expected 9)"
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
