#!/usr/bin/env bash
# Coverage floor check: fail CI if any tracked crate drops below its
# current measured baseline (with a small tolerance for measurement noise).
#
# Floors are the values from docs/COVERAGE.md "Baseline (2026-04-19,
# refreshed post-#1357)", rounded for cypher-core down to the nearest 0.5.
# This is intentionally NOT the 70% target — it just prevents regression.
# A separate ratchet PR should raise individual floors as crates reach 70%.
set -uo pipefail

# crate <space> floor%
CRATES_AND_FLOORS=(
  "cypher-core 58.5"
  "gql-engine 78.35"
  "gremlin-core 58.97"
  "sedan-cypher-embedded 65.48"
  "sedan-gql-embedded 72.92"
  "sedan-gremlin-embedded 96.43"
)

TOLERANCE="1.0"
OUT_DIR="target/llvm-cov-reports"
mkdir -p "$OUT_DIR"

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "Installing cargo-llvm-cov..." >&2
  cargo install cargo-llvm-cov --locked
fi
rustup component add llvm-tools-preview >/dev/null 2>&1 || true

parse_pct() {
  # $1 = json file path
  if command -v jq >/dev/null 2>&1; then
    jq -r '.data[0].totals.lines.percent' "$1"
  else
    python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['data'][0]['totals']['lines']['percent'])" "$1"
  fi
}

failed=0
printf '\n%-28s %10s %10s %10s\n' "CRATE" "MEASURED" "FLOOR" "STATUS"
printf '%-28s %10s %10s %10s\n' "----------------------------" "----------" "----------" "----------"

for entry in "${CRATES_AND_FLOORS[@]}"; do
  crate="${entry% *}"
  floor="${entry##* }"
  json="${OUT_DIR}/${crate}.json"

  echo "--- running cargo llvm-cov -p ${crate} --lib ---" >&2
  if ! cargo llvm-cov -p "$crate" --lib --json --output-path "$json" >&2; then
    printf '%-28s %10s %10s %10s\n' "$crate" "ERROR" "$floor" "FAIL"
    failed=1
    continue
  fi

  measured="$(parse_pct "$json")"
  # status: PASS if measured >= floor - tolerance
  status="$(python3 -c "
m=float('$measured'); f=float('$floor'); t=float('$TOLERANCE')
if m + 1e-9 >= f - t:
    print('PASS')
else:
    print(f'FAIL (-{(f-t)-m:.2f}pt below floor-tolerance)')
")"
  printf '%-28s %9.2f%% %9.2f%% %s\n' "$crate" "$measured" "$floor" "$status"
  if [[ "$status" != "PASS" ]]; then
    failed=1
  fi
done

echo
if (( failed != 0 )); then
  echo "Coverage floor check FAILED — one or more crates regressed." >&2
  echo "If this is intentional (e.g., a refactor that exposes existing gaps)," >&2
  echo "update the floor table in scripts/coverage_floor_check.sh and the" >&2
  echo "baseline section in docs/COVERAGE.md in the same PR." >&2
  exit 1
fi

echo "Coverage floor check passed — no crate regressed beyond ${TOLERANCE}pt tolerance."
exit 0
