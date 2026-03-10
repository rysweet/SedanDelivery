#!/usr/bin/env bash
# verify.sh — Verify that Neodash is running and accessible
#
# Checks:
#   1. Container is running
#   2. HTTP endpoint responds on port 5005
#   3. SedanDelivery Bolt is reachable on port 7687
#
# Usage:
#   ./verify.sh

set -euo pipefail

CONTAINER_NAME="neodash-sedandelivery"
NEODASH_PORT=5005
BOLT_PORT=7687
PASS=0
FAIL=0

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

pass() { echo -e "  ${GREEN}PASS${NC}  $*"; PASS=$((PASS + 1)); }
fail() { echo -e "  ${RED}FAIL${NC}  $*"; FAIL=$((FAIL + 1)); }
skip() { echo -e "  ${YELLOW}SKIP${NC}  $*"; }

echo -e "${CYAN}Neodash Verification${NC}"
echo "==============================="
echo ""

# ── Check 1: Docker container status ─────────────────────────────────────────
echo "1. Docker container"
if docker ps --format '{{.Names}}' 2>/dev/null | grep -q "^${CONTAINER_NAME}$"; then
    STATUS=$(docker inspect --format='{{.State.Status}}' "$CONTAINER_NAME" 2>/dev/null)
    pass "Container '$CONTAINER_NAME' is $STATUS"
else
    fail "Container '$CONTAINER_NAME' is not running"
    echo "       Start it with: export BOLT_PASSWORD=change-me && ./run.sh"
fi

# ── Check 2: Neodash HTTP endpoint ───────────────────────────────────────────
echo "2. Neodash HTTP endpoint"
HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' "http://localhost:${NEODASH_PORT}" 2>/dev/null || echo "000")
if [[ "$HTTP_CODE" == "200" ]]; then
    pass "http://localhost:${NEODASH_PORT} returned HTTP $HTTP_CODE"
elif [[ "$HTTP_CODE" == "000" ]]; then
    fail "http://localhost:${NEODASH_PORT} is unreachable (connection refused)"
else
    fail "http://localhost:${NEODASH_PORT} returned HTTP $HTTP_CODE (expected 200)"
fi

# ── Check 3: Neodash serves expected content ─────────────────────────────────
echo "3. Neodash content check"
BODY=$(curl -s "http://localhost:${NEODASH_PORT}" 2>/dev/null || echo "")
if echo "$BODY" | grep -qi "neodash\|neo4j\|react"; then
    pass "Page content contains expected Neodash/React markers"
else
    if [[ -z "$BODY" ]]; then
        fail "Empty response from Neodash"
    else
        fail "Page content does not look like Neodash (unexpected HTML)"
    fi
fi

# ── Check 4: SedanDelivery Bolt server ───────────────────────────────────────
echo "4. SedanDelivery Bolt server"
if nc -z localhost "$BOLT_PORT" 2>/dev/null; then
    pass "Bolt server is reachable on localhost:$BOLT_PORT"
elif command -v ss &>/dev/null && ss -tlnp 2>/dev/null | grep -q ":$BOLT_PORT "; then
    pass "Bolt server is reachable on localhost:$BOLT_PORT"
else
    fail "Bolt server is not reachable on localhost:$BOLT_PORT"
    echo "       Neodash will show connection errors until Bolt is running."
fi

# ── Check 5: Container logs (quick sanity) ───────────────────────────────────
echo "5. Container logs (last 5 lines)"
if docker ps --format '{{.Names}}' 2>/dev/null | grep -q "^${CONTAINER_NAME}$"; then
    docker logs --tail 5 "$CONTAINER_NAME" 2>&1 | while IFS= read -r line; do
        echo "       $line"
    done
    pass "Logs retrieved"
else
    skip "Container not running; no logs to check"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "==============================="
TOTAL=$((PASS + FAIL))
echo -e "Results: ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC} / ${TOTAL} checks"

if [[ $FAIL -eq 0 ]]; then
    echo -e "${GREEN}All checks passed.${NC}"
    exit 0
else
    echo -e "${YELLOW}Some checks failed. See details above.${NC}"
    exit 1
fi
