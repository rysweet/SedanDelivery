#!/usr/bin/env bash
set -euo pipefail
echo "Installing cargo-tarpaulin if needed..."
cargo install cargo-tarpaulin --version 0.31.4 2>/dev/null || true
echo "Running coverage analysis..."
cargo tarpaulin -p cypher-core --lib --out html --output-dir coverage/ -- --test-threads 1
echo "Coverage report: coverage/tarpaulin-report.html"
