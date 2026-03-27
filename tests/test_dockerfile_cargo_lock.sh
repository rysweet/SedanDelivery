#!/usr/bin/env bash
# tests/test_dockerfile_cargo_lock.sh
#
# Verifies that every workflow-built Rust Dockerfile copies Cargo.lock into the
# build context before the dependency pre-warm step. Without Cargo.lock, Cargo
# resolves dependency versions freely and may select incompatible crates,
# causing exit code 101 (SD #1000).
#
# Usage:
#   bash tests/test_dockerfile_cargo_lock.sh
#
#   # With custom root (CI, isolated environments):
#   DOCKERFILE_TEST_ROOT=/path/to/repo bash tests/test_dockerfile_cargo_lock.sh

set -euo pipefail

ROOT="${DOCKERFILE_TEST_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }

check_dockerfile() {
    local file="$1"
    local path="$ROOT/$file"

    echo ""
    echo "=== $file ==="

    if [[ ! -f "$path" ]]; then
        fail "$file not found at $path"
        fail "$file ordering: cannot check (file missing)"
        return
    fi

    # Test 1: COPY Cargo.lock ./ must be present
    if grep -qE '^COPY[[:space:]]+(Cargo\.lock[[:space:]]|.*[[:space:]]Cargo\.lock[[:space:]])' "$path" \
        || grep -qE '^COPY Cargo\.lock \.\/' "$path" \
        || grep -qE '^COPY Cargo\.toml Cargo\.lock' "$path"; then
        pass "$file contains COPY Cargo.lock"
    else
        fail "$file missing 'COPY Cargo.lock ./'"
    fi

    # Test 2: COPY Cargo.lock must appear on the same line as or after COPY Cargo.toml
    local toml_line lock_line first_build_line
    # Find line number of COPY Cargo.toml (first occurrence in the builder stage)
    toml_line=$(grep -n "COPY Cargo" "$path" | grep -i "Cargo\.toml" | head -1 | cut -d: -f1 || true)
    lock_line=$(grep -n "COPY Cargo" "$path" | grep -i "Cargo\.lock" | head -1 | cut -d: -f1 || true)
    first_build_line=$(grep -n 'cargo build' "$path" | head -1 | cut -d: -f1 || true)

    if [[ -z "$toml_line" ]]; then
        fail "$file ordering: COPY Cargo.toml not found"
        return
    fi

    if [[ -z "$lock_line" ]]; then
        fail "$file ordering: COPY Cargo.lock not found (cannot check order)"
        return
    fi

    if [[ "$lock_line" -ge "$toml_line" ]]; then
        pass "$file Cargo.lock (line $lock_line) copied at or after Cargo.toml (line $toml_line)"
    else
        fail "$file Cargo.lock (line $lock_line) appears BEFORE Cargo.toml (line $toml_line) — wrong layer order"
    fi

    # Test 3: both COPY lines must land before the first cargo build/pre-warm layer
    if [[ -z "$first_build_line" ]]; then
        fail "$file ordering: first cargo build layer not found"
        return
    fi

    if [[ "$toml_line" -lt "$first_build_line" && "$lock_line" -lt "$first_build_line" ]]; then
        pass "$file copies Cargo.toml/Cargo.lock before first cargo build layer (line $first_build_line)"
    else
        fail "$file copies Cargo manifests after first cargo build layer (build line $first_build_line)"
    fi
}

echo "Dockerfile Cargo.lock verification tests"
echo "Root: $ROOT"

check_dockerfile "Dockerfile"
check_dockerfile "Dockerfile.bolt"
check_dockerfile "Dockerfile.sync"
check_dockerfile "Dockerfile.api"

echo ""
echo "Results: $PASS passed, $FAIL failed"

if [[ "$FAIL" -gt 0 ]]; then
    echo ""
    echo "FAILED — Add 'COPY Cargo.lock ./' immediately after 'COPY Cargo.toml ./' in the"
    echo "         listed Dockerfile(s). Without the lockfile, Cargo resolves dependency"
    echo "         versions at build time and may select incompatible crates (exit 101)."
    exit 1
fi

echo "OK"
exit 0
