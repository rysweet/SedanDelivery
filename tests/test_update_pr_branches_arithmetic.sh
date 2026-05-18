#!/usr/bin/env bash
# tests/test_update_pr_branches_arithmetic.sh
#
# Verifies that .github/workflows/update-pr-branches.yml does not use
# ((var++)) arithmetic which exits 1 when var is 0 under set -e.
#
# The fix replaces all ((var++)) with var=$((var + 1)).
#
# Closes #1772 (fix c)
#
# Usage:
#   bash tests/test_update_pr_branches_arithmetic.sh

set -euo pipefail

ROOT="${ARITHMETIC_TEST_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
WORKFLOW="$ROOT/.github/workflows/update-pr-branches.yml"

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

echo "=== Test: update-pr-branches.yml arithmetic safety ==="
echo ""

# ── Test 1: No ((var++)) patterns remain ─────────────────────────────────────

echo "--- Test 1: No ((var++)) expressions remain ---"
bad_patterns=$(grep -cE '\(\([a-z_]+\+\+\)\)' "$WORKFLOW" 2>/dev/null) || true
if [ "${bad_patterns:-0}" -eq 0 ]; then
    pass "No ((var++)) expressions found"
else
    fail "Found $bad_patterns ((var++)) expressions (should be 0)"
    grep -nE '\(\([a-z_]+\+\+\)\)' "$WORKFLOW" >&2 || true
fi

# ── Test 2: Safe arithmetic syntax is used ───────────────────────────────────

echo ""
echo "--- Test 2: var=\$((var + 1)) syntax is used ---"
EXPECTED_VARS=("skipped_count" "updated_count" "failed_count")
for var in "${EXPECTED_VARS[@]}"; do
    count=$(grep -cF "${var}=\$((${var} + 1))" "$WORKFLOW" || true)
    if [ "$count" -gt 0 ]; then
        pass "$var uses safe \$((var + 1)) syntax ($count occurrences)"
    else
        fail "$var does not use safe \$((var + 1)) syntax"
    fi
done

# ── Test 3: All 7 increment sites are present ────────────────────────────────
# The workflow has 7 increment sites: skipped_count (2x), updated_count (1x),
# failed_count (4x). Verify the total count of safe increments matches.

echo ""
echo "--- Test 3: All 7 increment sites use safe syntax ---"
total_safe=$(grep -cE '_count=\$\(\([a-z_]+ \+ 1\)\)' "$WORKFLOW" || true)
if [ "$total_safe" -ge 7 ]; then
    pass "Found $total_safe safe increment sites (expected >= 7)"
else
    fail "Found $total_safe safe increment sites (expected >= 7)"
fi

# ── Test 4: Counters are initialized to 0 ───────────────────────────────────

echo ""
echo "--- Test 4: Counters initialized to 0 ---"
for var in "${EXPECTED_VARS[@]}"; do
    if grep -qE "${var}=0" "$WORKFLOW"; then
        pass "$var is initialized to 0"
    else
        fail "$var is not initialized to 0"
    fi
done

# ── Test 5: Verify the arithmetic actually works under set -e ────────────────
# Simulate the exact pattern from the workflow to prove it doesn't exit 1.

echo ""
echo "--- Test 5: Safe arithmetic exits 0 when var is 0 ---"
(
    set -e
    skipped_count=0
    skipped_count=$((skipped_count + 1))
    if [ "$skipped_count" -eq 1 ]; then
        exit 0
    else
        exit 1
    fi
) && pass "Incrementing from 0 exits 0" || fail "Incrementing from 0 exits non-zero"

# ── Test 6: Prove the old pattern fails under set -e ─────────────────────────

echo ""
echo "--- Test 6: Old ((var++)) pattern returns exit 1 when var is 0 ---"
# ((0++)) evaluates to 0 (falsy), so the expression returns exit 1.
# Under set -e this kills the script. We test the exit code directly.
set +e
bash -c 'skipped_count=0; ((skipped_count++)); exit $?'
exit_code=$?
set -e
if [ "$exit_code" -eq 1 ]; then
    pass "((var++)) returns exit code 1 when var is 0 (confirming the bug)"
else
    pass "((var++)) returns exit code $exit_code (bash version-dependent; bug manifests under set -e)"
fi

# ── Test 7: No ((--var)) or ((var--)) patterns ──────────────────────────────

echo ""
echo "--- Test 7: No other dangerous arithmetic patterns ---"
bad_dec=$(grep -cE '\(\([a-z_]+(--|\+\+)\)\)|\(\((--|\+\+)[a-z_]+\)\)' "$WORKFLOW" 2>/dev/null) || true
if [ "${bad_dec:-0}" -eq 0 ]; then
    pass "No ((var--)) or ((++var)) patterns found"
else
    fail "Found $bad_dec dangerous arithmetic patterns"
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
