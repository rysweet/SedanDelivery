#!/usr/bin/env bash
# Mutation testing for the embedded engine crates via cargo-mutants.
#
# Mutation testing introduces small semantic-preserving (or semantic-breaking)
# changes — "mutants" — into the source code and re-runs the test suite. A
# mutant that the suite fails to detect is a test gap: the test passes even
# though the code is wrong.
#
# Usage:
#   scripts/mutation_test_embedded.sh                   # all three crates
#   scripts/mutation_test_embedded.sh sedan-gql-embedded  # one crate
#   SHARD="0/4" scripts/mutation_test_embedded.sh       # shard 0 of 4 (CI)
#
# See docs/MUTATION_TESTING.md for interpretation guidance.

set -euo pipefail

CRATES=("$@")
if [ ${#CRATES[@]} -eq 0 ]; then
    CRATES=(sedan-cypher-embedded sedan-gql-embedded sedan-gremlin-embedded)
fi

if ! command -v cargo-mutants >/dev/null 2>&1; then
    echo "cargo-mutants not found; installing (~25)..."
    cargo install cargo-mutants --version "^25" --locked
fi

EXTRA_ARGS=()
if [ -n "${SHARD:-}" ]; then
    EXTRA_ARGS+=("--shard" "$SHARD")
fi

OVERALL=0
for crate in "${CRATES[@]}"; do
    out_dir="mutants.${crate#sedan-}.out"
    echo
    echo "=== cargo mutants -p $crate (output: $out_dir) ==="
    if ! cargo mutants \
        -p "$crate" \
        --no-shuffle \
        --timeout-multiplier 3 \
        --output "$out_dir" \
        "${EXTRA_ARGS[@]}"; then
        echo "  -> $crate: missed/timeout mutants present (see $out_dir)"
        OVERALL=1
    fi

    if [ -f "$out_dir/mutants.out/missed.txt" ]; then
        missed=$(wc -l <"$out_dir/mutants.out/missed.txt" || echo 0)
        caught=$(wc -l <"$out_dir/mutants.out/caught.txt" 2>/dev/null || echo 0)
        unviable=$(wc -l <"$out_dir/mutants.out/unviable.txt" 2>/dev/null || echo 0)
        timeout=$(wc -l <"$out_dir/mutants.out/timeout.txt" 2>/dev/null || echo 0)
        echo "  $crate summary: caught=$caught missed=$missed unviable=$unviable timeout=$timeout"
    fi
done

exit "$OVERALL"
