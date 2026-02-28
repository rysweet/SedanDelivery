#!/usr/bin/env bash
# validate.sh — run the full SedanDelivery validation suite locally.
#
# Mirrors the CI tiers from .github/workflows/ci.yml.
# Exits non-zero on the first failure.
#
# Usage:
#   bash scripts/validate.sh            # all tiers
#   bash scripts/validate.sh --fast     # Tier 1 + 2 only (< 3 min)
#   bash scripts/validate.sh --tck      # add TCK (takes ~3 min extra)
#   bash scripts/validate.sh --bench    # add benchmark smoke-run

set -euo pipefail

FAST=false
WITH_TCK=false
WITH_BENCH=false

for arg in "$@"; do
  case "$arg" in
    --fast)  FAST=true ;;
    --tck)   WITH_TCK=true ;;
    --bench) WITH_BENCH=true ;;
  esac
done

pass() { printf '\033[0;32m✓ %s\033[0m\n' "$*"; }
fail() { printf '\033[0;31m✗ %s\033[0m\n' "$*"; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# ── Tier 1: fast checks ───────────────────────────────────────────────────────

step "Tier 1: formatting"
cargo fmt --check
pass "cargo fmt"

step "Tier 1: clippy (zero warnings)"
cargo clippy -p gql-core -- -D warnings
pass "clippy"

step "Tier 1: unit tests (385 tests)"
cargo test -p gql-core --lib
pass "unit tests"

# ── Tier 2: integration tests ─────────────────────────────────────────────────

step "Tier 2: end-to-end query pipeline"
cargo test -p gql-core --test end_to_end
pass "end_to_end"

step "Tier 2: EXISTS / interactive"
cargo test -p gql-core --test exists_interactive
pass "exists_interactive"

step "Tier 2: data integrity (23 tests)"
cargo test -p gql-core --test data_integrity
pass "data_integrity"

step "Tier 2: adversarial queries (11 tests)"
cargo test -p gql-core --test adversarial
pass "adversarial"

if $FAST; then
  echo ""
  pass "Tier 1 + 2 complete (--fast mode, skipping Tier 3)"
  exit 0
fi

# ── Tier 3: full suite ────────────────────────────────────────────────────────

step "Tier 3: stress tests (8 tests)"
cargo test -p gql-core --test stress
pass "stress"

step "Tier 3: LDBC SNB benchmark queries (37 tests)"
cargo test -p gql-core --test ldbc
pass "ldbc"

step "Tier 3: Redis module integration (18 tests)"
cargo test -p gql-module --test integration
pass "module integration"

step "Tier 3: FalkorDB compatibility (28 tests)"
cargo test -p gql-module --test compatibility
pass "compatibility"

if $WITH_TCK; then
  step "Tier 3: openCypher TCK compliance (3903 scenarios — takes ~3 min)"
  cargo test -p gql-core --test tck
  pass "TCK 3903/3903"
else
  echo ""
  echo "  (Skipping TCK — pass --tck to include it)"
fi

# ── Tier 4: benchmarks (optional) ────────────────────────────────────────────

if $WITH_BENCH; then
  step "Tier 4: benchmark smoke-run (compile + single iteration)"
  cargo bench -p gql-core -- --test
  pass "benchmarks compile and run"
fi

echo ""
pass "All validation tiers complete"
echo ""
echo "Summary:"
echo "  Unit tests:        385"
echo "  Integration tests: 128  (end_to_end + exists + data_integrity + adversarial + stress + ldbc + module + compat)"
if $WITH_TCK;   then echo "  TCK scenarios:     3903"; fi
if $WITH_BENCH; then echo "  Benchmarks:        26 (smoke-run only — run without --test for timing)"; fi
echo ""
echo "To establish a performance baseline:"
echo "  cargo bench -p gql-core -- --save-baseline main"
echo "To compare against it later:"
echo "  cargo bench -p gql-core -- --baseline main"
