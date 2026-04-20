#!/usr/bin/env bash
# perf_regression_check.sh — Per-PR criterion benchmark perf-regression gate.
#
# Runs the curated criterion bench suite against a baseline commit
# (default: origin/main) and the current HEAD, parses the per-bench
# `estimates.json` files criterion writes under `target/criterion/`, and
# fails if any benchmark's mean point estimate regressed by more than
# PERF_REGRESSION_THRESHOLD percent (default 10).
#
# Usage:
#   bash scripts/perf_regression_check.sh
#
# Environment overrides:
#   PERF_REGRESSION_THRESHOLD   percent regression that fails the gate (default 10)
#   BASELINE_REF                git ref for the baseline (default: origin/main)
#   HEAD_REF                    git ref for the candidate (default: HEAD)
#   DRY_RUN                     if "1", point baseline at HEAD (smoke test, expects ~0%)
#   BENCH_SPECS                 space-separated "<pkg>::<bench>" list to run.
#                               Defaults to the curated set below.
#   CRITERION_DIR               criterion output dir (default: target/criterion)
#   CARGO_BENCH_ARGS            extra args appended to each `cargo bench` invocation,
#                               e.g. "--warm-up-time 1 --measurement-time 2".
#
# Exits 0 on PASS or no-regression, 1 on regression, 2 on infra/setup error.

set -euo pipefail

THRESHOLD="${PERF_REGRESSION_THRESHOLD:-10}"
BASELINE_REF="${BASELINE_REF:-origin/main}"
HEAD_REF="${HEAD_REF:-HEAD}"
DRY_RUN="${DRY_RUN:-0}"
CRITERION_DIR="${CRITERION_DIR:-target/criterion}"
CARGO_BENCH_ARGS_DEFAULT="--warm-up-time 1 --measurement-time 2"
CARGO_BENCH_ARGS="${CARGO_BENCH_ARGS:-$CARGO_BENCH_ARGS_DEFAULT}"

# Curated bench list. These are the headline criterion suites that don't
# require external services (Redis, Bolt) — the existing `benchmarks` job
# in .github/workflows/ci.yml continues to handle service-bound benches in
# bencher format. Override with BENCH_SPECS to run a different set.
DEFAULT_BENCH_SPECS=(
  "cypher-core::parser"
  "cypher-core::executor"
  "cypher-core::property_index"
  "gremlin-core::parse_compile"
  "gql-engine::gql_parse"
  "sedan-cypher-embedded::embedded_bench"
  "sedan-gql-embedded::embedded_bench"
  "sedan-gremlin-embedded::embedded_bench"
  "embedded-bench::cypher_bench"
  "embedded-bench::gql_bench"
  "embedded-bench::gremlin_bench"
)
if [[ -n "${BENCH_SPECS:-}" ]]; then
  read -r -a BENCH_SPECS_ARR <<< "$BENCH_SPECS"
else
  BENCH_SPECS_ARR=("${DEFAULT_BENCH_SPECS[@]}")
fi

BASE_LABEL="perfgate-baseline"
HEAD_LABEL="perfgate-head"

log() { echo "[perf-gate] $*"; }
die() { echo "[perf-gate] ERROR: $*" >&2; exit 2; }

command -v cargo  >/dev/null 2>&1 || die "cargo not found"
command -v git    >/dev/null 2>&1 || die "git not found"
command -v python3 >/dev/null 2>&1 || die "python3 not found"

# Refuse to run with a dirty working tree — we need to checkout other refs.
if ! git diff --quiet || ! git diff --cached --quiet; then
  die "Working tree has uncommitted changes; commit/stash them first."
fi

orig_ref="$(git symbolic-ref --quiet --short HEAD || git rev-parse HEAD)"
baseline_sha="$(git rev-parse "$BASELINE_REF")" || die "cannot resolve $BASELINE_REF"
head_sha="$(git rev-parse "$HEAD_REF")"         || die "cannot resolve $HEAD_REF"

if [[ "$DRY_RUN" == "1" ]]; then
  log "DRY_RUN=1 → comparing HEAD against itself ($head_sha)"
  baseline_sha="$head_sha"
fi

log "Baseline ref: $BASELINE_REF ($baseline_sha)"
log "Head ref:     $HEAD_REF ($head_sha)"
log "Threshold:    ${THRESHOLD}% regression"
log "Benches:      ${BENCH_SPECS_ARR[*]}"

restore_ref() {
  git checkout --quiet "$orig_ref" >/dev/null 2>&1 || true
}
trap restore_ref EXIT

run_benches() {
  local label="$1"
  log "Running ${#BENCH_SPECS_ARR[@]} benches with --save-baseline ${label}"
  # Emit cargo's compile messages as JSON so callers can stream/inspect; the
  # actual measurement numbers are read from criterion's estimates.json files.
  for spec in "${BENCH_SPECS_ARR[@]}"; do
    local pkg="${spec%%::*}"
    local bench="${spec##*::}"
    log "  cargo bench -p $pkg --bench $bench -- --save-baseline $label"
    # shellcheck disable=SC2086
    if ! cargo bench --message-format=json -p "$pkg" --bench "$bench" \
         -- --save-baseline "$label" $CARGO_BENCH_ARGS \
         > "/tmp/perfgate-${label}-${pkg}-${bench}.json" 2>&1; then
      log "  WARN: $spec failed (see /tmp/perfgate-${label}-${pkg}-${bench}.json); continuing"
    fi
  done
}

# ── Baseline run ────────────────────────────────────────────────────────────
log "Checking out baseline: $baseline_sha"
git checkout --quiet --detach "$baseline_sha"
run_benches "$BASE_LABEL"

# ── Head run ────────────────────────────────────────────────────────────────
if [[ "$DRY_RUN" == "1" ]]; then
  log "DRY_RUN=1 → reusing baseline checkout for head run"
else
  log "Checking out head: $head_sha"
  git checkout --quiet --detach "$head_sha"
fi
run_benches "$HEAD_LABEL"

# ── Compare via criterion estimates.json ────────────────────────────────────
log "Comparing baseline vs head from $CRITERION_DIR"

python3 - "$CRITERION_DIR" "$BASE_LABEL" "$HEAD_LABEL" "$THRESHOLD" <<'PY'
import json
import os
import sys
from pathlib import Path

criterion_dir, base_label, head_label, threshold_str = sys.argv[1:5]
threshold = float(threshold_str)
root = Path(criterion_dir)

if not root.is_dir():
    print(f"[perf-gate] ERROR: criterion dir not found: {root}", file=sys.stderr)
    sys.exit(2)

# Walk every <group>/<bench>/<label>/estimates.json in the criterion tree.
def collect(label):
    out = {}
    for est in root.glob(f"**/{label}/estimates.json"):
        bench_dir = est.parent.parent
        # Bench name = path from criterion root to bench dir, joined with "/".
        rel = bench_dir.relative_to(root)
        name = "/".join(rel.parts)
        try:
            data = json.loads(est.read_text())
            mean_ns = data["mean"]["point_estimate"]
        except (KeyError, ValueError, OSError) as e:
            print(f"[perf-gate] WARN: cannot parse {est}: {e}", file=sys.stderr)
            continue
        out[name] = mean_ns
    return out

base = collect(base_label)
head = collect(head_label)

if not base:
    print(f"[perf-gate] ERROR: no baseline estimates found under {root}/**/{base_label}/", file=sys.stderr)
    sys.exit(2)
if not head:
    print(f"[perf-gate] ERROR: no head estimates found under {root}/**/{head_label}/", file=sys.stderr)
    sys.exit(2)

names = sorted(set(base) | set(head))

print()
print(f"{'Benchmark':<60} {'Baseline (ms)':>14} {'Head (ms)':>14} {'Delta %':>10}  Status")
print("-" * 110)

regressions = []
checked = 0
for name in names:
    b = base.get(name)
    h = head.get(name)
    if b is None:
        print(f"{name:<60} {'—':>14} {h/1e6:>14.4f} {'—':>10}  NEW")
        continue
    if h is None:
        print(f"{name:<60} {b/1e6:>14.4f} {'—':>14} {'—':>10}  MISSING")
        continue
    checked += 1
    pct = ((h - b) / b) * 100.0 if b > 0 else 0.0
    if pct > threshold:
        status = "FAIL"
        regressions.append((name, b, h, pct))
    elif pct < -threshold:
        status = "IMPROVED"
    else:
        status = "OK"
    print(f"{name:<60} {b/1e6:>14.4f} {h/1e6:>14.4f} {pct:>9.2f}%  {status}")

print()
print(f"Checked: {checked} | Threshold: {threshold:g}% | Regressions: {len(regressions)}")

if regressions:
    print()
    print("FAIL — the following benchmarks regressed:")
    for name, b, h, pct in regressions:
        print(f"  • {name}: {b/1e6:.4f}ms → {h/1e6:.4f}ms  (+{pct:.2f}%)")
    sys.exit(1)

print("PASS — no regressions exceeded the threshold.")
PY

rc=$?
exit "$rc"
