#!/usr/bin/env bash
# Branch-coverage floor check: fail CI if any tracked crate drops below its
# current measured branch-coverage baseline (with a small tolerance for
# measurement noise).
#
# Branch coverage complements line coverage: an executed line can still
# contain an untaken `if` / `match` arm / short-circuiting boolean branch,
# which line coverage hides but branch coverage exposes. See docs/COVERAGE.md
# "Branch coverage" section for rationale and the toolchain requirement
# (nightly Rust + cargo-llvm-cov --branch).
#
# Floors are the values measured at PR introduction (see docs/COVERAGE.md
# "Branch-coverage baseline"). This is intentionally NOT a 70% target —
# it just prevents regression. A separate ratchet PR should raise individual
# floors as crates improve.
set -uo pipefail

# crate <space> floor%
# 0/0 crates (no branches in --lib): floor 0 ⇒ always passes, kept in the
# table so we notice if branches start being emitted.
CRATES_AND_FLOORS=(
  "cypher-core 60.0"
  "gql-engine 81.0"
  "gremlin-core 57.0"
  "sedan-cypher-embedded 100.0"
  "sedan-gql-embedded 0.0"
  "sedan-gremlin-embedded 0.0"
)

TOLERANCE="1.0"
OUT_DIR="target/llvm-cov-branch-reports"
mkdir -p "$OUT_DIR"

# cargo-llvm-cov's --branch flag is unstable and requires the nightly
# toolchain plus the nightly llvm-tools-preview component.
TOOLCHAIN="${BRANCH_COV_TOOLCHAIN:-nightly}"

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "Installing cargo-llvm-cov..." >&2
  cargo install cargo-llvm-cov --locked
fi
rustup toolchain install "$TOOLCHAIN" --profile minimal >/dev/null 2>&1 || true
rustup component add llvm-tools-preview --toolchain "$TOOLCHAIN" >/dev/null 2>&1 || true

parse_totals() {
  # $1 = json file; prints "<branch_pct> <branch_count>"
  if command -v jq >/dev/null 2>&1; then
    jq -r '"\(.data[0].totals.branches.percent) \(.data[0].totals.branches.count)"' "$1"
  else
    python3 -c "
import json,sys
t=json.load(open(sys.argv[1]))['data'][0]['totals'].get('branches',{})
print(t.get('percent',0.0), t.get('count',0))
" "$1"
  fi
}

failed=0
printf '\n%-28s %10s %10s %10s\n' "CRATE" "MEASURED" "FLOOR" "STATUS"
printf '%-28s %10s %10s %10s\n' "----------------------------" "----------" "----------" "----------"

for entry in "${CRATES_AND_FLOORS[@]}"; do
  crate="${entry% *}"
  floor="${entry##* }"
  json="${OUT_DIR}/${crate}.json"

  echo "--- running rustup run ${TOOLCHAIN} cargo llvm-cov --branch -p ${crate} ---" >&2
  # Match coverage_floor_check.sh: cypher-core uses --lib --tests because
  # Gremlin-only executor modules are exercised by integration tests.
  if [[ "$crate" == "cypher-core" ]]; then
    cov_args=(--lib --tests)
  else
    cov_args=(--lib)
  fi
  if ! rustup run "$TOOLCHAIN" cargo llvm-cov -p "$crate" "${cov_args[@]}" \
        --branch --json --output-path "$json" >&2; then
    printf '%-28s %10s %10s %10s\n' "$crate" "ERROR" "$floor" "FAIL"
    failed=1
    continue
  fi

  read -r measured count < <(parse_totals "$json")
  status="$(python3 -c "
m=float('$measured'); f=float('$floor'); t=float('$TOLERANCE'); n=int('$count')
if n == 0:
    # No instrumented branches in this crate's --lib (e.g., thin re-export
    # shells). Treat as PASS but flag if a floor > 0 is configured.
    if f > 0:
        print(f'WARN (no branches but floor={f})')
    else:
        print('PASS (no branches)')
elif m + 1e-9 >= f - t:
    print('PASS')
else:
    print(f'FAIL (-{(f-t)-m:.2f}pt below floor-tolerance)')
")"
  printf '%-28s %9.2f%% %9.2f%% %s\n' "$crate" "$measured" "$floor" "$status"
  if [[ "$status" == FAIL* ]]; then
    failed=1
  fi
done

echo
if (( failed != 0 )); then
  echo "Branch-coverage floor check FAILED — one or more crates regressed." >&2
  echo "If this is intentional (e.g., a refactor that exposes existing gaps)," >&2
  echo "update the floor table in scripts/branch_coverage_floor_check.sh and" >&2
  echo "the baseline section in docs/COVERAGE.md in the same PR." >&2
  exit 1
fi

echo "Branch-coverage floor check passed — no crate regressed beyond ${TOLERANCE}pt tolerance."
exit 0
