#!/usr/bin/env bash
# test-cluster-bolt-smoke.sh — Validate the public Bolt path against Redis Cluster.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="$PROJECT_DIR/docker-compose.cluster.yml"

if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' NC=''
fi

log_info()  { echo -e "${YELLOW}[INFO]${NC}  $*"; }
log_pass()  { echo -e "${GREEN}[PASS]${NC}  $*"; }
log_fail()  { echo -e "${RED}[FAIL]${NC}  $*"; }

export VALKEY_PASSWORD="${VALKEY_PASSWORD:-local-cluster-pass}"
export BOLT_PASSWORD="${BOLT_PASSWORD:-$VALKEY_PASSWORD}"

if ! docker compose version &>/dev/null; then
    echo "ERROR: docker compose v2 is required but not found."
    exit 1
fi

if ! python3 -c "import neo4j" 2>/dev/null; then
    echo "ERROR: Python neo4j driver is required. Install with: pip install neo4j"
    exit 1
fi

cleanup() {
    log_info "Cleaning up..."
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" down -v --remove-orphans --timeout 10 >/dev/null 2>&1 || true
}
trap cleanup EXIT

main() {
    echo "================================================================"
    echo "  Cluster Bolt Smoke Test"
    echo "================================================================"

    cd "$PROJECT_DIR"
    cleanup

    log_info "Building and starting Redis Cluster + Bolt..."
    VALKEY_PASSWORD="$VALKEY_PASSWORD" BOLT_PASSWORD="$BOLT_PASSWORD" \
        docker compose -f "$COMPOSE_FILE" up -d --build

    log_info "Waiting for Bolt health endpoint..."
    for i in $(seq 1 180); do
        if curl -sf http://localhost:7688/health >/dev/null; then
            log_info "Bolt is healthy (after ${i}s)"
            break
        fi
        if [ "$i" -eq 180 ]; then
            log_fail "Bolt did not become healthy within 180s"
            docker compose -f "$COMPOSE_FILE" logs --tail=80 sedandelivery-bolt 2>/dev/null || true
            exit 1
        fi
        sleep 1
    done

    log_info "Running Bolt smoke query flow with session(database=graph)..."
    BOLT_PASSWORD="$BOLT_PASSWORD" python3 - <<'PYEOF'
import os
import uuid

from neo4j import GraphDatabase

password = os.environ["BOLT_PASSWORD"]
uri = "bolt://localhost:7687"
graph = "__cluster_smoke_" + uuid.uuid4().hex[:8]
driver = GraphDatabase.driver(uri, auth=("neo4j", password))

try:
    with driver.session() as session:
        session.run(f"CREATE GRAPH {graph}").consume()

    with driver.session(database=graph) as session:
        session.run("CREATE (:Person {name: $name})", name="Alice").consume()
        session.run("CREATE (:Person {name: $name})", name="Bob").consume()
        node_count = session.run(
            "MATCH (n:Person) RETURN count(n) AS c"
        ).single(strict=True)["c"]
        if node_count != 2:
            raise SystemExit(f"expected 2 Person nodes, got {node_count}")

        session.run(
            "MATCH (n:Person {name: $name}) SET n:Manager",
            name="Bob",
        ).consume()
        manager_count = session.run(
            "MATCH (n:Manager {name: $name}) RETURN count(n) AS c",
            name="Bob",
        ).single(strict=True)["c"]
        if manager_count != 1:
            raise SystemExit(f"expected Bob to be relabeled as Manager, got {manager_count} matches")

        session.run(
            "MATCH (n:Manager {name: $name}) REMOVE n:Manager",
            name="Bob",
        ).consume()
        manager_count = session.run(
            "MATCH (n:Manager {name: $name}) RETURN count(n) AS c",
            name="Bob",
        ).single(strict=True)["c"]
        if manager_count != 0:
            raise SystemExit(f"expected Bob to no longer have Manager label, got {manager_count} matches")

        session.run(
            "MATCH (a:Person {name: $a}), (b:Person {name: $b}) "
            "CREATE (a)-[:KNOWS]->(b)",
            a="Alice",
            b="Bob",
        ).consume()
        edge_count = session.run(
            "MATCH ()-[r:KNOWS]->() RETURN count(r) AS c"
        ).single(strict=True)["c"]
        if edge_count != 1:
            raise SystemExit(f"expected 1 KNOWS edge, got {edge_count}")

    with driver.session() as session:
        session.run(f"DROP GRAPH {graph}").consume()
finally:
    driver.close()
PYEOF

    log_pass "bolt cluster smoke passed"
}

main "$@"
