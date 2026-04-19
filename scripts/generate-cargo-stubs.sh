#!/usr/bin/env bash
# Generate Cargo workspace stub source files for Docker dependency pre-caching.
#
# When a Dockerfile pre-warms cargo dependency caches by COPYing only Cargo.toml
# files (without sources), every workspace member needs at least a stub source
# file (lib.rs / main.rs) — and every [[bench]] entry needs a stub bench file —
# or `cargo build` exits 101 with "could not find Cargo.toml" for benches or
# "lib.rs not found" for libs.
#
# This script reads the workspace `Cargo.toml`, enumerates members, and for
# each member reads its own Cargo.toml to determine:
#   - is it a [lib] (default if `src/lib.rs` is referenced or no [[bin]]) → stub `src/lib.rs`
#   - is it a [[bin]] → stub `src/main.rs` (or the configured `path`)
#   - any [[bench]] entries → stub at the configured `path`
#
# Output: writes stub files to the current directory (assumed to be a fresh
# Docker workspace mirror containing only Cargo.toml files).
#
# Usage: scripts/generate-cargo-stubs.sh
#
# This script is invoked from each Dockerfile's pre-warm RUN block. It replaces
# the previously-manual mkdir/echo wall that had to be edited every time a new
# member or bench was added (and which produced merge conflicts on every
# parallel feature branch — see #1272/#1273/#1274 history).

set -euo pipefail

# Discover workspace members from the root Cargo.toml.
# Supports the simple list form:
#   members = [
#     "cypher-core",
#     "sd2-redis-store",
#     ...
#   ]
members=$(awk '
    /^\[workspace\]/         { in_ws=1; next }
    in_ws && /^\[/           { in_ws=0 }
    in_ws && /^members[[:space:]]*=/  { in_members=1; sub(/^[^=]*=[[:space:]]*\[/, "") }
    in_members {
        # extract any quoted strings on this line
        while (match($0, /"[^"]+"/)) {
            s = substr($0, RSTART+1, RLENGTH-2)
            print s
            $0 = substr($0, RSTART + RLENGTH)
        }
        if (index($0, "]") > 0) { in_members=0; in_ws=0 }
    }
' Cargo.toml)

if [ -z "$members" ]; then
    echo "error: no workspace members found in Cargo.toml" >&2
    exit 1
fi

stub_lib() {
    local dir="$1"
    mkdir -p "$dir/src"
    [ -f "$dir/src/lib.rs" ] || echo "pub fn stub() {}" > "$dir/src/lib.rs"
}

stub_bin() {
    local dir="$1" path="${2:-src/main.rs}"
    mkdir -p "$(dirname "$dir/$path")"
    [ -f "$dir/$path" ] || echo "fn main() {}" > "$dir/$path"
}

stub_bench() {
    local dir="$1" path="$2"
    mkdir -p "$(dirname "$dir/$path")"
    [ -f "$dir/$path" ] || echo "fn main() {}" > "$dir/$path"
}

for member in $members; do
    if [ ! -f "$member/Cargo.toml" ]; then
        echo "warn: skipping $member (no Cargo.toml)" >&2
        continue
    fi

    has_lib=0
    has_bin=0

    # Detect [lib] section
    if grep -qE '^\[lib\]' "$member/Cargo.toml"; then
        has_lib=1
    fi

    # Detect [[bin]] sections and extract paths (default src/main.rs if unspecified)
    bin_paths=$(awk '
        function flush() {
            if (in_bin) print path
            in_bin=0; path="src/main.rs"
        }
        /^\[\[bin\]\]/ { flush(); in_bin=1; path="src/main.rs"; next }
        in_bin && /^\[/ { flush(); next }
        in_bin && /^path[[:space:]]*=/ { gsub(/[ "=]/, ""); sub(/^path/, ""); path=$0 }
        END { flush() }
    ' "$member/Cargo.toml")

    if [ -n "$bin_paths" ]; then
        has_bin=1
    fi

    # Always stub src/lib.rs unless the crate is explicitly bin-only (no [lib]
    # AND has [[bin]] AND its source tree has no lib.rs). In practice we cannot
    # know "source tree has no lib.rs" inside the Docker pre-warm where sources
    # are not yet COPYed, so we err on the side of stubbing lib.rs for every
    # member. Cargo treats a crate with both lib.rs and main.rs as having both
    # targets — this is harmless for the dependency cache pre-warm.
    stub_lib "$member"

    if [ -n "$bin_paths" ]; then
        for p in $bin_paths; do
            stub_bin "$member" "$p"
        done
    fi

    # Stub all [[bench]] paths
    bench_paths=$(awk '
        function flush() {
            if (in_bench) {
                if (path == "" && name != "") path = "benches/" name ".rs"
                if (path != "") print path
            }
            in_bench=0; path=""; name=""
        }
        /^\[\[bench\]\]/ { flush(); in_bench=1; next }
        in_bench && /^\[/ { flush(); next }
        in_bench && /^name[[:space:]]*=/ { gsub(/[ "=]/, ""); sub(/^name/, ""); name=$0 }
        in_bench && /^path[[:space:]]*=/ { gsub(/[ "=]/, ""); sub(/^path/, ""); path=$0 }
        END { flush() }
    ' "$member/Cargo.toml")

    if [ -n "$bench_paths" ]; then
        for p in $bench_paths; do
            stub_bench "$member" "$p"
        done
    fi
done

echo "stubs generated for $(echo "$members" | wc -l) workspace members"
