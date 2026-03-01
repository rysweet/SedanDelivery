#!/usr/bin/env bash
# run.sh — Launch Neodash dashboard connected to SedanDelivery Bolt server
#
# Prerequisites:
#   - SedanDelivery Bolt server running on localhost:7687
#   - Docker installed and running
#
# Usage:
#   ./run.sh          # Start Neodash on port 5005
#   ./run.sh --stop   # Stop and remove the container

set -euo pipefail

CONTAINER_NAME="neodash-sedandelivery"
NEODASH_PORT=5005
BOLT_PORT=7687
IMAGE="neo4jlabs/neodash:latest"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

info()  { echo -e "${CYAN}[INFO]${NC}  $*"; }
ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; }

# ── Handle --stop flag ────────────────────────────────────────────────────────
if [[ "${1:-}" == "--stop" ]]; then
    info "Stopping Neodash container..."
    docker stop "$CONTAINER_NAME" 2>/dev/null && ok "Container stopped." || warn "Container was not running."
    docker rm "$CONTAINER_NAME" 2>/dev/null && ok "Container removed." || true
    exit 0
fi

# ── Step 1: Check that SedanDelivery Bolt is running ─────────────────────────
info "Checking SedanDelivery Bolt server on port $BOLT_PORT..."
if nc -z localhost "$BOLT_PORT" 2>/dev/null; then
    ok "Bolt server is running on localhost:$BOLT_PORT"
elif command -v ss &>/dev/null && ss -tlnp 2>/dev/null | grep -q ":$BOLT_PORT "; then
    ok "Bolt server is running on localhost:$BOLT_PORT"
elif command -v lsof &>/dev/null && lsof -i :"$BOLT_PORT" &>/dev/null; then
    ok "Bolt server is running on localhost:$BOLT_PORT"
else
    error "No service detected on port $BOLT_PORT."
    echo "  Start the SedanDelivery Bolt server first:"
    echo "    cargo run --release -p sedandelivery-bolt"
    echo "  Or via Docker:"
    echo "    docker run -p 7687:7687 sedandelivery-bolt"
    exit 1
fi

# ── Step 2: Check Docker ─────────────────────────────────────────────────────
info "Checking Docker availability..."
if ! command -v docker &>/dev/null; then
    error "Docker is not installed. Please install Docker first."
    exit 1
fi

if ! docker info &>/dev/null 2>&1; then
    error "Docker daemon is not running. Please start Docker."
    exit 1
fi
ok "Docker is available."

# ── Step 3: Clean up any existing container ──────────────────────────────────
if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    info "Removing existing container '$CONTAINER_NAME'..."
    docker stop "$CONTAINER_NAME" 2>/dev/null || true
    docker rm "$CONTAINER_NAME" 2>/dev/null || true
    ok "Old container removed."
fi

# ── Step 4: Pull the Neodash image ───────────────────────────────────────────
info "Pulling Neodash image: $IMAGE ..."
docker pull "$IMAGE"
ok "Image pulled."

# ── Step 5: Determine host address for container-to-host networking ──────────
# On Linux, host.docker.internal may not resolve; use the docker bridge IP.
BOLT_HOST="172.17.0.1"
if [[ "$(uname -s)" == "Darwin" ]] || grep -qi microsoft /proc/version 2>/dev/null; then
    BOLT_HOST="host.docker.internal"
fi
info "Container will connect to Bolt at bolt://${BOLT_HOST}:${BOLT_PORT}"

# ── Step 6: Start Neodash ────────────────────────────────────────────────────
info "Starting Neodash container..."
docker run -d \
    --name "$CONTAINER_NAME" \
    -p "${NEODASH_PORT}:5005" \
    --add-host="host.docker.internal:host-gateway" \
    -e ssoEnabled="false" \
    -e standaloneProtocol="bolt" \
    -e standaloneHost="${BOLT_HOST}" \
    -e standalonePort="${BOLT_PORT}" \
    -e standaloneDatabase="" \
    -e standaloneUsername="neo4j" \
    -e standalonePassword="neo4j" \
    --restart unless-stopped \
    "$IMAGE"

ok "Neodash container started."

# ── Step 7: Wait for Neodash to become healthy ───────────────────────────────
info "Waiting for Neodash to start (up to 30s)..."
for i in $(seq 1 30); do
    if curl -s -o /dev/null -w '%{http_code}' "http://localhost:${NEODASH_PORT}" 2>/dev/null | grep -q '200'; then
        ok "Neodash is ready!"
        break
    fi
    if [[ $i -eq 30 ]]; then
        warn "Neodash did not respond within 30s. It may still be starting."
        warn "Check with: docker logs $CONTAINER_NAME"
    fi
    sleep 1
done

# ── Step 8: Print access info ────────────────────────────────────────────────
echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Neodash is running!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "  Dashboard URL:  ${CYAN}http://localhost:${NEODASH_PORT}${NC}"
echo ""
echo -e "  Connection settings in Neodash UI:"
echo -e "    Protocol:  bolt"
echo -e "    Host:      localhost"
echo -e "    Port:      7687"
echo -e "    Username:  neo4j"
echo -e "    Password:  neo4j"
echo ""
echo -e "  Pre-built dashboard:"
echo -e "    ${CYAN}${SCRIPT_DIR}/dashboard.json${NC}"
echo -e "    Import via Neodash: Settings icon > Load Dashboard > paste JSON"
echo ""
echo -e "  Stop:  ${YELLOW}./run.sh --stop${NC}"
echo -e "  Logs:  ${YELLOW}docker logs -f $CONTAINER_NAME${NC}"
echo ""
