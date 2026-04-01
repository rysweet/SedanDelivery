#!/usr/bin/env bash
# scripts/fabric-sync-status.sh
# Quick health-check for the SedanDelivery Fabric replication pipeline.
#
# Inspects the Valkey __sd2:changelog stream and the
# sedandelivery-fabric-sync consumer group to report:
#   - Stream length and metadata
#   - Consumer group state
#   - Pending (unacknowledged) entries
#   - Per-consumer lag
#
# Usage:
#   ./scripts/fabric-sync-status.sh <redis-host> [redis-port] [password]
#
# Exit codes:
#   0 — healthy (stream exists, no pending entries)
#   1 — degraded (pending entries or consumer group issues)
#   2 — stream not found (changelog disabled or no writes yet)

set -euo pipefail

REDIS_HOST="${1:?Usage: fabric-sync-status.sh <redis-host> [port] [password]}"
REDIS_PORT="${2:-6379}"
REDIS_PASSWORD="${3:-}"
STREAM="__sd2:changelog"
GROUP="sedandelivery-fabric-sync"

EXIT_CODE=0

cli() {
  if [ -n "$REDIS_PASSWORD" ]; then
    redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" -a "$REDIS_PASSWORD" --no-auth-warning "$@" 2>/dev/null
  else
    redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" "$@" 2>/dev/null
  fi
}

echo "=========================================="
echo "Fabric Sync Pipeline Status"
echo "  Host: ${REDIS_HOST}:${REDIS_PORT}"
echo "  Stream: ${STREAM}"
echo "  Consumer group: ${GROUP}"
echo "=========================================="

# -- Stream existence check ------------------------------------------------

STREAM_LEN=$(cli XLEN "$STREAM" || echo "")
if [[ -z "$STREAM_LEN" || "$STREAM_LEN" == "(nil)" ]]; then
  echo ""
  echo "Stream '${STREAM}' not found."
  echo "This means either:"
  echo "  - GQL_FABRIC_CHANGELOG=1 is not set on the sd2-redis-module"
  echo "  - No write operations have been executed yet"
  exit 2
fi

# -- Stream info -----------------------------------------------------------

echo ""
echo "=== Changelog Stream ==="
echo "  Length: ${STREAM_LEN}"
cli XINFO STREAM "$STREAM" || true

# -- Consumer group info ---------------------------------------------------

echo ""
echo "=== Consumer Group: ${GROUP} ==="
GROUP_INFO=$(cli XINFO GROUPS "$STREAM" 2>/dev/null || echo "")
if [[ -z "$GROUP_INFO" || "$GROUP_INFO" == *"ERR"* ]]; then
  echo "  Consumer group '${GROUP}' does not exist."
  echo "  The sedandelivery-fabric-sync sidecar has not started yet."
  EXIT_CODE=1
else
  echo "$GROUP_INFO"
fi

# -- Pending entries -------------------------------------------------------

echo ""
echo "=== Pending Entries (unacknowledged) ==="
PENDING=$(cli XPENDING "$STREAM" "$GROUP" 2>/dev/null || echo "")
if [[ -z "$PENDING" || "$PENDING" == *"ERR"* ]]; then
  echo "  No pending entries (or group does not exist)"
else
  echo "$PENDING"
  # Parse the count from the first line of XPENDING summary
  PENDING_COUNT=$(echo "$PENDING" | head -1 | tr -d '(integer) ')
  if [[ "$PENDING_COUNT" =~ ^[0-9]+$ ]] && [[ "$PENDING_COUNT" -gt 0 ]]; then
    echo ""
    echo "  WARNING: ${PENDING_COUNT} entries pending acknowledgement"
    echo "  These events have been delivered but not yet confirmed."

    # Show detailed pending entries (up to 10)
    echo ""
    echo "  Oldest pending entries (up to 10):"
    cli XPENDING "$STREAM" "$GROUP" "-" "+" "10" || true
    EXIT_CODE=1
  fi
fi

# -- Consumer details ------------------------------------------------------

echo ""
echo "=== Consumers ==="
cli XINFO CONSUMERS "$STREAM" "$GROUP" 2>/dev/null || echo "  No consumers registered"

# -- Summary ---------------------------------------------------------------

echo ""
echo "=========================================="
if [[ "$EXIT_CODE" -eq 0 ]]; then
  echo "Status: HEALTHY"
  echo "  Stream length: ${STREAM_LEN}"
  echo "  No pending entries"
else
  echo "Status: DEGRADED"
  echo "  Stream length: ${STREAM_LEN}"
  echo "  Check pending entries above for details"
fi
echo "=========================================="

exit "$EXIT_CODE"
