#!/usr/bin/env bash
# Public-API change gate for the embedded crates.
#
# Captures a textual snapshot of every `pub` item in the embedded crate
# surfaces (sedan-cypher-embedded, sedan-gql-embedded, sedan-gremlin-embedded)
# and compares it against committed snapshots in `docs/public-api/`.
#
# Why not cargo-public-api? It requires nightly rustdoc JSON which is a moving
# target and adds a heavyweight tool to CI. A textual `pub`-line snapshot is
# good enough to catch accidental surface changes and is trivially reviewable
# in the PR diff.
#
# Usage:
#   scripts/check-public-api.sh            # compare against committed snapshots
#   scripts/check-public-api.sh --update   # regenerate snapshots
set -euo pipefail

CRATES=(sedan-cypher-embedded sedan-gql-embedded sedan-gremlin-embedded)
SNAPSHOT_DIR="docs/public-api"
MODE="${1:-check}"

mkdir -p "$SNAPSHOT_DIR"

snapshot_crate() {
  local crate="$1"
  # Capture every line that starts with `pub ` from the crate's src/.
  # Sort for deterministic output regardless of source order.
  find "$crate/src" -name '*.rs' -print0 \
    | xargs -0 grep -hE '^[[:space:]]*pub (use|fn|struct|enum|trait|mod|const|static|type|unsafe) ' \
    | sed -E 's/[[:space:]]+/ /g; s/^ //; s/ $//' \
    | sort -u
}

fail=0
for crate in "${CRATES[@]}"; do
  snap_file="$SNAPSHOT_DIR/${crate}.txt"
  current="$(snapshot_crate "$crate")"
  if [ "$MODE" = "--update" ]; then
    printf '%s\n' "$current" > "$snap_file"
    echo "updated: $snap_file"
    continue
  fi
  if [ ! -f "$snap_file" ]; then
    echo "MISSING snapshot: $snap_file (run scripts/check-public-api.sh --update)"
    fail=1
    continue
  fi
  if ! diff -u "$snap_file" <(printf '%s\n' "$current") > /tmp/api-diff.$$; then
    echo "PUBLIC API CHANGED: $crate"
    cat /tmp/api-diff.$$
    rm -f /tmp/api-diff.$$
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo
  echo "Public API drift detected. If intentional, run:"
  echo "    scripts/check-public-api.sh --update"
  echo "and commit the updated docs/public-api/*.txt snapshots."
  exit 1
fi

echo "Public API snapshots match for: ${CRATES[*]}"
