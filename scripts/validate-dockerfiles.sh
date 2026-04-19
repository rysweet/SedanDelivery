#!/usr/bin/env bash
# validate-dockerfiles.sh — Verify that every workflow-built Rust Dockerfile
# correctly references the workspace and lockfile contract.
#
# Exit code 0 = all checks pass
# Exit code 1 = one or more checks failed
#
# Closes #1000

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FAILURES=0

# ── Helpers ──────────────────────────────────────────────────────────────────

fail() { echo "FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }
pass() { echo "PASS: $*"; }

# ── 1. Extract workspace members from Cargo.toml ─────────────────────────────
# Use a large context window (1000 lines) so the extraction is not silently
# truncated as the file grows.

TOML_LINES=$(wc -l < "$REPO_ROOT/Cargo.toml")
mapfile -t MEMBERS < <(
    grep -A"${TOML_LINES}" '^\[workspace\]' "$REPO_ROOT/Cargo.toml" \
    | sed -n '/members\s*=/,/\]/p' \
    | grep '"' \
    | sed 's/.*"\(.*\)".*/\1/'
)

echo "Workspace members (${#MEMBERS[@]}): ${MEMBERS[*]}"
echo ""

# ── 2. Check each Dockerfile copies Cargo.lock ───────────────────────────────

for df in Dockerfile Dockerfile.bolt Dockerfile.sync Dockerfile.api; do
    FILE="$REPO_ROOT/$df"
    # Accept "COPY Cargo.lock ./" or "COPY Cargo.toml Cargo.lock ./"
    if grep -qE '^COPY[[:space:]]+(Cargo\.lock[[:space:]]|.*[[:space:]]Cargo\.lock([[:space:]]|$))' "$FILE" \
        || grep -qE '^COPY Cargo\.toml Cargo\.lock' "$FILE"; then
        pass "$df copies Cargo.lock"
    else
        fail "$df is missing a COPY instruction for Cargo.lock"
    fi
done
echo ""

# ── 2b. Check Cargo manifests land before the first cargo build layer ─────────

for df in Dockerfile Dockerfile.bolt Dockerfile.sync Dockerfile.api; do
    FILE="$REPO_ROOT/$df"
    toml_line=$(grep -n "COPY Cargo" "$FILE" | grep -i "Cargo\.toml" | head -1 | cut -d: -f1 || true)
    lock_line=$(grep -n "COPY Cargo" "$FILE" | grep -i "Cargo\.lock" | head -1 | cut -d: -f1 || true)
    first_build_line=$(grep -n 'cargo build' "$FILE" | head -1 | cut -d: -f1 || true)

    if [ -z "$toml_line" ] || [ -z "$lock_line" ] || [ -z "$first_build_line" ]; then
        fail "$df: cannot verify Cargo manifest ordering before the first cargo build layer"
        continue
    fi

    if [ "$toml_line" -lt "$first_build_line" ] && [ "$lock_line" -lt "$first_build_line" ]; then
        pass "$df: Cargo.toml and Cargo.lock are copied before the first cargo build layer"
    else
        fail "$df: Cargo manifests appear after the first cargo build layer"
    fi
done
echo ""

# ── 3. Check each Dockerfile copies every workspace member's Cargo.toml ──────

for df in Dockerfile Dockerfile.bolt Dockerfile.sync Dockerfile.api; do
    FILE="$REPO_ROOT/$df"
    first_build_line=$(grep -n 'cargo build' "$FILE" | head -1 | cut -d: -f1 || true)
    df_failures=0

    if [ -z "$first_build_line" ]; then
        fail "$df: cannot verify workspace manifest ordering because no cargo build line was found"
        continue
    fi

    for member in "${MEMBERS[@]}"; do
        member_line=$(grep -n "COPY ${member}/Cargo.toml" "$FILE" | head -1 | cut -d: -f1 || true)
        if [ -z "$member_line" ]; then
            fail "$df: missing 'COPY ${member}/Cargo.toml'"
            df_failures=$((df_failures + 1))
        elif [ "$member_line" -ge "$first_build_line" ]; then
            fail "$df: COPY ${member}/Cargo.toml appears after the first cargo build layer"
            df_failures=$((df_failures + 1))
        fi
    done
    if [ "$df_failures" -eq 0 ]; then
        pass "$df: all ${#MEMBERS[@]} workspace Cargo.toml files appear before the first cargo build layer"
    fi
done
echo ""

# ── 4. Check real cargo build commands use --locked ──────────────────────────
# Every "cargo build --release" line that is NOT a stub step (identified by
# ending in "|| true") must include "--locked".

for df in Dockerfile Dockerfile.bolt Dockerfile.sync Dockerfile.api; do
    FILE="$REPO_ROOT/$df"
    unlocked_real_builds=$(grep -E 'cargo build --release' "$FILE" \
        | grep -v '|| true' \
        | grep -v -- '--locked' || true)
    if [ -z "$unlocked_real_builds" ]; then
        pass "$df: all real cargo build steps use --locked"
    else
        fail "$df: the following real cargo build steps are missing --locked:"$'\n'"$unlocked_real_builds"
    fi
done
echo ""

# ── 5. Check all Dockerfiles stub [[bin]] targets for every workspace member ──
# Any workspace member with a [[bin]] target needs a src/main.rs stub or Cargo
# can fail during workspace resolution in the dependency pre-warm layer.

BIN_MEMBERS=()
for member in "${MEMBERS[@]}"; do
    member_manifest="$REPO_ROOT/$member/Cargo.toml"
    if [ -f "$member_manifest" ] && grep -q '^\[\[bin\]\]' "$member_manifest"; then
        BIN_MEMBERS+=("$member")
    fi
done

for df in Dockerfile Dockerfile.bolt Dockerfile.sync Dockerfile.api; do
    FILE="$REPO_ROOT/$df"
    first_build_line=$(grep -n 'cargo build' "$FILE" | head -1 | cut -d: -f1 || true)
    df_failures=0

    if [ -z "$first_build_line" ]; then
        fail "$df: cannot verify [[bin]] stub ordering because no cargo build line was found"
        continue
    fi

    # Two valid stub strategies are supported:
    #   1. The Dockerfile invokes scripts/generate-cargo-stubs.sh before the
    #      first cargo build layer (preferred — auto-handles new members).
    #   2. The Dockerfile manually COPYs a stub `<member>/src/main.rs` for
    #      each [[bin]] member before the first cargo build layer (legacy).
    stub_script_line=$(grep -n 'scripts/generate-cargo-stubs\.sh' "$FILE" | grep -E 'RUN|^[0-9]+:[[:space:]]*scripts/generate' | head -1 | cut -d: -f1 || true)
    if [ -n "$stub_script_line" ] && [ "$stub_script_line" -lt "$first_build_line" ]; then
        pass "$df: uses scripts/generate-cargo-stubs.sh (auto-stubs all ${#BIN_MEMBERS[@]} [[bin]] members) before the first cargo build layer"
        continue
    fi

    for bin_member in "${BIN_MEMBERS[@]}"; do
        stub_line=$(grep -n "${bin_member}/src/main.rs" "$FILE" | head -1 | cut -d: -f1 || true)
        if [ -z "$stub_line" ]; then
            fail "$df: missing ${bin_member}/src/main.rs stub (required for [[bin]] target)"
            df_failures=$((df_failures + 1))
        elif [ "$stub_line" -ge "$first_build_line" ]; then
            fail "$df: ${bin_member}/src/main.rs stub appears after the first cargo build layer"
            df_failures=$((df_failures + 1))
        fi
    done
    if [ "$df_failures" -eq 0 ]; then
        pass "$df: all ${#BIN_MEMBERS[@]} [[bin]] member stubs (src/main.rs) appear before the first cargo build layer"
    fi
done
echo ""

# ── 6. Check .dockerignore re-includes Cargo.toml for every tests/ member ────
# The .dockerignore file excludes the entire tests/ directory. Any workspace
# member under tests/ must be explicitly re-included. Forgetting this causes
# "file not found in build context" even though the COPY instruction is correct.

DOCKERIGNORE="$REPO_ROOT/.dockerignore"
for member in "${MEMBERS[@]}"; do
    if [[ "$member" == tests/* ]]; then
        if grep -q "!${member}/Cargo.toml" "$DOCKERIGNORE"; then
            pass ".dockerignore re-includes ${member}/Cargo.toml"
        else
            fail ".dockerignore is missing '!${member}/Cargo.toml' (tests/ is excluded by default)"
        fi
    fi
done
echo ""

# ── Summary ──────────────────────────────────────────────────────────────────

if [ "$FAILURES" -eq 0 ]; then
    echo "All checks passed."
    exit 0
else
    echo "$FAILURES check(s) failed." >&2
    exit 1
fi
