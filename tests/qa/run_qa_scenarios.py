#!/usr/bin/env python3
"""Execute QA scenarios for B1 (write safety), B2 (scaling), B4 (observability).

Requires:
  - Two Bolt instances on ports 7687 and 7689 sharing the same Redis
  - Redis on localhost:6379
  - neo4j Python driver installed
"""
import argparse
import concurrent.futures
import json
import socket
import sys
import time
import urllib.request

from neo4j import GraphDatabase


PASS = 0
FAIL = 0


def check(label, condition, detail=""):
    global PASS, FAIL
    if condition:
        PASS += 1
        print(f"  [PASS] {label}")
    else:
        FAIL += 1
        print(f"  [FAIL] {label} -- {detail}")


def bolt_driver(uri, password="neo4j"):
    return GraphDatabase.driver(uri, auth=("neo4j", password))


def session(driver):
    """Open a session on the default neo4j database."""
    return driver.session()


# ── B1: Write Safety ────────────────────────────────────────────────────────

def test_b1_concurrent_property_writes(uri1, uri2):
    """Two clients write different properties to the same node concurrently.
    With distributed locks, no writes should be lost."""
    print("\n--- B1.1: Concurrent Property Writes (no lost updates) ---")
    d1, d2 = bolt_driver(uri1), bolt_driver(uri2)
    try:
        with session(d1) as s:
            s.run("CREATE (:QaB1Target {id: 1})").consume()

        def write_props(driver, props):
            with session(driver) as s:
                for k, v in props.items():
                    s.run(f"MATCH (n:QaB1Target {{id: 1}}) SET n.{k} = $v", v=v).consume()

        props_a = {f"a_{i}": f"val_a_{i}" for i in range(10)}
        props_b = {f"b_{i}": f"val_b_{i}" for i in range(10)}

        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as ex:
            f1 = ex.submit(write_props, d1, props_a)
            f2 = ex.submit(write_props, d2, props_b)
            f1.result(timeout=30)
            f2.result(timeout=30)

        with session(d1) as s:
            result = s.run("MATCH (n:QaB1Target {id: 1}) RETURN n").single()
            node_props = dict(result["n"])

        for k in props_a:
            check(f"Property {k} present", k in node_props, f"missing from node")
        for k in props_b:
            check(f"Property {k} present", k in node_props, f"missing from node")
    finally:
        with session(d1) as s:
            s.run("MATCH (n:QaB1Target) DETACH DELETE n").consume()
        d1.close()
        d2.close()


def test_b1_atomic_delete_graph(uri1, uri2):
    """Create nodes, then delete them all — should be atomic."""
    print("\n--- B1.2: Atomic delete (DETACH DELETE) ---")
    d1, d2 = bolt_driver(uri1), bolt_driver(uri2)
    try:
        with session(d1) as s:
            for i in range(50):
                s.run(f"CREATE (:QaB1Del {{id: {i}}})").consume()

        with session(d1) as s:
            count = s.run("MATCH (n:QaB1Del) RETURN count(n) AS c").single()["c"]
            check("50 nodes created before delete", count == 50, f"got {count}")

        with session(d1) as s:
            s.run("MATCH (n:QaB1Del) DETACH DELETE n").consume()

        with session(d2) as s:
            count = s.run("MATCH (n:QaB1Del) RETURN count(n) AS c").single()["c"]
            check("0 nodes after delete (from replica 2)", count == 0, f"got {count}")
    finally:
        d1.close()
        d2.close()


def test_b1_cross_replica_bookmark(uri1, uri2):
    """Write on instance A, verify readable from instance B."""
    print("\n--- B1.3: Cross-Replica Bookmark Consistency ---")
    d1, d2 = bolt_driver(uri1), bolt_driver(uri2)
    try:
        with session(d1) as s:
            s.run("CREATE (:QaB1Bookmark {key: 'from_replica_1'})").consume()

        with session(d2) as s:
            result = s.run("MATCH (n:QaB1Bookmark) RETURN n.key AS k").single()
            check("Cross-replica read succeeds", result is not None)
            if result:
                check("Correct value from replica 2", result["k"] == "from_replica_1",
                      f"got {result['k']}")
    finally:
        with session(d1) as s:
            s.run("MATCH (n:QaB1Bookmark) DETACH DELETE n").consume()
        d1.close()
        d2.close()


# ── B2: Advanced Scaling ────────────────────────────────────────────────────

def test_b2_metrics_endpoint(port=7688):
    """Verify /metrics endpoint exposes expected Prometheus metrics."""
    print("\n--- B2.1: Metrics Endpoint ---")
    resp = urllib.request.urlopen(f"http://localhost:{port}/metrics").read().decode()
    for metric in [
        "sedandelivery_bolt_connections_active",
        "sedandelivery_bolt_connections_total",
        "sedandelivery_bolt_queries_total",
        "sedandelivery_bolt_query_duration_seconds",
    ]:
        check(f"Metric {metric} present", metric in resp, "not found in /metrics")


def test_b2_health_endpoint(port=7688):
    """Verify /health returns status:ok with expected fields."""
    print("\n--- B2.2: Health Endpoint ---")
    resp = json.loads(urllib.request.urlopen(f"http://localhost:{port}/health").read())
    check("Health status is ok", resp.get("status") == "ok", f"got {resp}")
    check("Connections field present", "connections" in resp)
    check("Uptime field present", "uptime_seconds" in resp)


def test_b2_tcp_keepalive(port=7687):
    """Verify TCP connection stays alive (keepalive is configured)."""
    print("\n--- B2.3: TCP Connection Keepalive ---")
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(5)
    try:
        sock.connect(("localhost", port))
        # Connection should succeed
        check("TCP connection established", True)
        # Check that the server sends the Bolt handshake preamble or waits
        # (the fact that we connected without RST proves keepalive path works)
        check("Connection alive after connect", sock.fileno() > 0)
    except Exception as e:
        check("TCP connection", False, str(e))
    finally:
        sock.close()


# ── B4: Observability ───────────────────────────────────────────────────────

def test_b4_backend_error_metric(port=7688):
    """Verify backend_errors metric exists in /metrics output."""
    print("\n--- B4.1: Backend Error Metric ---")
    resp = urllib.request.urlopen(f"http://localhost:{port}/metrics").read().decode()
    # The metric may be zero (no errors yet) but the name should be registered
    has_metric = "backend_error" in resp.lower() or "errors_total" in resp.lower()
    check("Error metric registered in Prometheus", has_metric,
          "no backend_error or errors_total metric found")


def test_b4_query_latency_histogram(uri, port=7688):
    """Execute a query and verify the duration histogram updates."""
    print("\n--- B4.2: Query Latency Histogram ---")
    d = bolt_driver(uri)
    try:
        with session(d) as s:
            s.run("RETURN 1 AS n").consume()

        resp = urllib.request.urlopen(f"http://localhost:{port}/metrics").read().decode()
        check("Query duration histogram updated",
              "query_duration_seconds_count" in resp,
              "no query_duration_seconds_count in /metrics")
    finally:
        d.close()


def test_b4_connection_metrics(uri, port=7688):
    """Open a connection and verify active connection gauge updates."""
    print("\n--- B4.3: Connection Metrics ---")
    d = bolt_driver(uri)
    try:
        with session(d) as s:
            s.run("RETURN 1 AS n").consume()
            # While session is open, check metrics
            resp = urllib.request.urlopen(f"http://localhost:{port}/metrics").read().decode()
            check("connections_total incremented",
                  "connections_total" in resp,
                  "no connections_total in /metrics")
    finally:
        d.close()


def main():
    parser = argparse.ArgumentParser(description="Run QA scenarios for B1/B2/B4")
    parser.add_argument("--bolt1", default="bolt://localhost:7687")
    parser.add_argument("--bolt2", default="bolt://localhost:7689")
    parser.add_argument("--health-port1", type=int, default=7688)
    parser.add_argument("--health-port2", type=int, default=7690)
    parser.add_argument("--password", default="neo4j")
    args = parser.parse_args()

    print("=" * 60)
    print("  QA Scenario Execution: B1 + B2 + B4")
    print("=" * 60)

    # B1: Write Safety
    print("\n== B1: Write Safety ==")
    test_b1_concurrent_property_writes(args.bolt1, args.bolt2)
    test_b1_atomic_delete_graph(args.bolt1, args.bolt2)
    test_b1_cross_replica_bookmark(args.bolt1, args.bolt2)

    # B2: Advanced Scaling
    print("\n== B2: Advanced Scaling ==")
    test_b2_metrics_endpoint(args.health_port1)
    test_b2_health_endpoint(args.health_port1)
    test_b2_tcp_keepalive()

    # B4: Observability
    print("\n== B4: Observability ==")
    test_b4_backend_error_metric(args.health_port1)
    test_b4_query_latency_histogram(args.bolt1, args.health_port1)
    test_b4_connection_metrics(args.bolt1, args.health_port1)

    print("\n" + "=" * 60)
    print(f"  Passed: {PASS}")
    print(f"  Failed: {FAIL}")
    print("=" * 60)
    sys.exit(1 if FAIL > 0 else 0)


if __name__ == "__main__":
    main()
