#!/usr/bin/env bash
# Run the SedanDelivery cross-engine differential parity suite.
#
# Asserts that cypher-core, gql-engine, and gremlin-core produce
# semantically equivalent results for every case in
# tests/differential/src/cases.rs.
#
# Exits non-zero if any "strict" parity case diverges across engines.
# Documented divergences are tracked but do not fail the run — see
# docs/DIFFERENTIAL_TESTING.md for the rationale on each.

set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> running differential parity suite"
cargo test -p differential -- --nocapture
echo "==> differential parity suite passed"
