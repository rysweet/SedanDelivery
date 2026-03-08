#!/usr/bin/env bash
# bench-compare.sh — Compare Criterion benchmark results in bencher format.
# Usage: bench-compare.sh <baseline.txt> <new.txt> [threshold_percent]
#
# Exits non-zero if any benchmark regresses beyond the threshold.
# Threshold defaults to PERF_REGRESSION_THRESHOLD env var, or 10 (percent).

set -euo pipefail

baseline="${1:?Usage: bench-compare.sh <baseline.txt> <new.txt> [threshold%]}"
new="${2:?Usage: bench-compare.sh <baseline.txt> <new.txt> [threshold%]}"
threshold="${3:-${PERF_REGRESSION_THRESHOLD:-10}}"

if [[ ! -f "$baseline" ]]; then
  echo "INFO: No baseline file found at '$baseline' — skipping regression check."
  exit 0
fi

if [[ ! -f "$new" ]]; then
  echo "ERROR: New results file not found at '$new'"
  exit 1
fi

# Parse bencher-format lines: "test <name> ... bench: <ns> ns/iter (+/- <var>)"
# into name→ns associative arrays.
declare -A base_ns new_ns

while IFS= read -r line; do
  if [[ "$line" =~ ^test\ ([^ ]+)\ +\.\.\.\ +bench:\ +([0-9,]+)\ +ns/iter ]]; then
    name="${BASH_REMATCH[1]}"
    val="${BASH_REMATCH[2]//,/}"
    base_ns["$name"]="$val"
  fi
done < "$baseline"

while IFS= read -r line; do
  if [[ "$line" =~ ^test\ ([^ ]+)\ +\.\.\.\ +bench:\ +([0-9,]+)\ +ns/iter ]]; then
    name="${BASH_REMATCH[1]}"
    val="${BASH_REMATCH[2]//,/}"
    new_ns["$name"]="$val"
  fi
done < "$new"

if [[ ${#base_ns[@]} -eq 0 ]]; then
  echo "WARN: Baseline contains no parseable benchmarks — skipping comparison."
  exit 0
fi

failed=0
checked=0

printf "\n%-60s %12s %12s %8s  %s\n" "Benchmark" "Baseline" "New" "Change" "Status"
printf "%-60s %12s %12s %8s  %s\n" "$(printf '%0.s-' {1..60})" "------------" "------------" "--------" "------"

for name in $(printf '%s\n' "${!base_ns[@]}" | sort); do
  b="${base_ns[$name]}"
  n="${new_ns[$name]:-}"

  if [[ -z "$n" ]]; then
    printf "%-60s %12s %12s %8s  %s\n" "$name" "$b" "MISSING" "-" "SKIP"
    continue
  fi

  checked=$((checked + 1))

  if [[ "$b" -eq 0 ]]; then
    pct="0.0"
  else
    # percent change: positive = regression (slower), negative = improvement
    pct=$(awk "BEGIN { printf \"%.1f\", (($n - $b) / $b) * 100 }")
  fi

  pct_abs="${pct#-}"

  if awk "BEGIN { exit !($pct > $threshold) }"; then
    status="FAIL"
    failed=$((failed + 1))
  elif awk "BEGIN { exit !($pct < -$threshold) }"; then
    status="IMPROVED"
  else
    status="OK"
  fi

  printf "%-60s %12s %12s %7s%%  %s\n" "$name" "$b" "$n" "$pct" "$status"
done

# Report new benchmarks not in baseline
for name in $(printf '%s\n' "${!new_ns[@]}" | sort); do
  if [[ -z "${base_ns[$name]:-}" ]]; then
    printf "%-60s %12s %12s %8s  %s\n" "$name" "NEW" "${new_ns[$name]}" "-" "NEW"
  fi
done

echo ""
echo "Checked: $checked | Threshold: ${threshold}% | Regressions: $failed"

if [[ "$failed" -gt 0 ]]; then
  echo "FAIL: $failed benchmark(s) regressed beyond ${threshold}% threshold."
  exit 1
fi

echo "PASS: No regressions detected."
exit 0
