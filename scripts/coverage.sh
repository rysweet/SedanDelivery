#!/usr/bin/env bash
# Generate baseline coverage (HTML + JSON) for embedded/core crates.
# Output: target/llvm-cov/html/<crate>/html/index.html, target/llvm-cov-reports/<crate>.json
set -euo pipefail

CRATES=(cypher-core gql-engine gremlin-core sedan-cypher-embedded sedan-gql-embedded sedan-gremlin-embedded)

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "Installing cargo-llvm-cov..." >&2
  cargo install cargo-llvm-cov
fi
rustup component add llvm-tools-preview >/dev/null 2>&1 || true

mkdir -p target/llvm-cov-reports target/llvm-cov/html

printf '\n%-30s %10s\n' "CRATE" "LINES%"
printf '%-30s %10s\n' "------------------------------" "----------"
for crate in "${CRATES[@]}"; do
  cargo llvm-cov -p "$crate" --lib --json --output-path "target/llvm-cov-reports/${crate}.json" >/dev/null 2>&1
  cargo llvm-cov -p "$crate" --lib --html --output-dir "target/llvm-cov/html/${crate}" >/dev/null 2>&1
  pct=$(python3 -c "import json; d=json.load(open('target/llvm-cov-reports/${crate}.json'))['data'][0]['totals']['lines']; print(f\"{d['percent']:.2f}\")")
  printf '%-30s %9s%%\n' "$crate" "$pct"
done
