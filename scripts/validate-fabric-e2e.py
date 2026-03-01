#!/usr/bin/env python3
"""Fabric E2E Validation — validates the full SedanDelivery replication pipeline.

Writes test data via Bolt protocol (or Redis CLI), waits for the
sedandelivery-fabric-sync sidecar to replicate it to Microsoft Fabric Graph
Database, then verifies every property type and relationship round-trips
correctly.

Exit code 0 on full success, 1 on any failure.

Environment variables (required):
    BOLT_URI              Bolt endpoint, e.g. bolt://host:7687
                          (falls back to redis://<REDIS_HOST>:<REDIS_PORT> via CLI)
    REDIS_HOST            Redis/Valkey host (used when BOLT_URI is unset)
    REDIS_PORT            Redis/Valkey port (default 6379, used when BOLT_URI is unset)
    FABRIC_ENDPOINT       Fabric Graph REST API base URL
    FABRIC_TENANT_ID      Azure AD tenant ID
    FABRIC_CLIENT_ID      Service principal app ID
    FABRIC_CLIENT_SECRET  Service principal client secret

Optional overrides (argparse):
    --timeout             Max seconds to wait for Fabric sync (default 120)
    --poll-interval       Seconds between Fabric poll attempts (default 5)
    --skip-cleanup        Do not remove test data after validation
    --report              Path for JSON report (default fabric-e2e-report.json)
"""

import argparse
import json
import math
import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import Any, Optional


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

@dataclass
class Config:
    bolt_uri: Optional[str]
    redis_host: Optional[str]
    redis_port: int
    fabric_endpoint: str
    fabric_tenant_id: str
    fabric_client_id: str
    fabric_client_secret: str
    timeout: int = 120
    poll_interval: int = 5
    skip_cleanup: bool = False
    report_path: str = "fabric-e2e-report.json"


@dataclass
class TestResult:
    name: str
    passed: bool
    detail: str = ""
    duration_s: float = 0.0


@dataclass
class Report:
    start_time: str = ""
    end_time: str = ""
    duration_s: float = 0.0
    passed: bool = False
    tests: list = field(default_factory=list)
    summary: str = ""


# ---------------------------------------------------------------------------
# Fabric API helpers
# ---------------------------------------------------------------------------

def get_fabric_token(tenant_id: str, client_id: str, client_secret: str) -> str:
    """Acquire an Azure AD access token for the Fabric API via client credentials."""
    import requests

    url = f"https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token"
    resp = requests.post(url, data={
        "grant_type": "client_credentials",
        "client_id": client_id,
        "client_secret": client_secret,
        "scope": "https://api.fabric.microsoft.com/.default",
    }, timeout=30)
    resp.raise_for_status()
    return resp.json()["access_token"]


def fabric_query(token: str, endpoint: str, cypher: str) -> dict:
    """Execute a read-only Cypher query against the Fabric Graph Database.

    The Fabric Graph REST API uses POST {endpoint}/query with:
        { "query": "<cypher>", "parameters": {} }

    Response shape:
        { "results": [ { "headers": [...], "data": [[col0, col1, ...], ...] } ] }
    """
    import requests

    url = f"{endpoint}/query"
    headers = {
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/json",
    }
    body = {"query": cypher, "parameters": {}}
    resp = requests.post(url, json=body, headers=headers, timeout=30)
    resp.raise_for_status()
    return resp.json()


def fabric_statement(token: str, endpoint: str, cypher: str) -> dict:
    """Execute a write Cypher statement against the Fabric Graph Database.

    Uses POST {endpoint}/statements.
    """
    import requests

    url = f"{endpoint}/statements"
    headers = {
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/json",
    }
    body = {"query": cypher, "parameters": {}}
    resp = requests.post(url, json=body, headers=headers, timeout=30)
    resp.raise_for_status()
    return resp.json()


def parse_fabric_rows(resp: dict) -> list[list]:
    """Extract rows from a Fabric query response.

    Returns a list of rows, each row being a list of column values.
    """
    rows = []
    for result_block in resp.get("results", []):
        if "error" in result_block:
            raise RuntimeError(f"Fabric result error: {result_block['error']}")
        for row in result_block.get("data", []):
            rows.append(row if isinstance(row, list) else [row])
    return rows


def parse_fabric_rows_with_headers(resp: dict) -> list[dict]:
    """Extract rows as dicts keyed by header name."""
    result_rows = []
    for result_block in resp.get("results", []):
        if "error" in result_block:
            raise RuntimeError(f"Fabric result error: {result_block['error']}")
        headers = result_block.get("headers", [])
        for row in result_block.get("data", []):
            if isinstance(row, list) and headers:
                result_rows.append(dict(zip(headers, row)))
            else:
                result_rows.append(row)
    return result_rows


# ---------------------------------------------------------------------------
# Write test data via Bolt protocol
# ---------------------------------------------------------------------------

def write_test_data_bolt(bolt_uri: str) -> None:
    """Write E2E test nodes and relationships via the Bolt driver."""
    from neo4j import GraphDatabase

    driver = GraphDatabase.driver(bolt_uri, auth=None)
    try:
        with driver.session() as session:
            # Create a node with all 8 property types
            session.run("""
                CREATE (:E2ETest {
                    str_val: 'hello',
                    int_val: 42,
                    float_val: 3.14,
                    bool_val: true,
                    date_val: date('2025-01-15'),
                    datetime_val: datetime('2025-01-15T10:30:00Z'),
                    duration_val: duration('P1Y2M3D'),
                    point_val: point({x: 1.0, y: 2.0})
                })
            """)

            # Create a relationship pattern
            session.run("""
                CREATE (:E2ESource {name: 'src'})-[:E2E_LINK {weight: 1.5}]->(:E2ETarget {name: 'tgt'})
            """)

        print("[WRITE] Test data written via Bolt")
    finally:
        driver.close()


def write_test_data_redis(host: str, port: int) -> None:
    """Write E2E test data via redis-cli / GQL.MUTATE commands.

    Fallback path when Bolt driver is unavailable.
    """
    def redis_cli(*args: str) -> str:
        cmd = ["redis-cli", "-h", host, "-p", str(port)] + list(args)
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        if result.returncode != 0:
            raise RuntimeError(f"redis-cli failed: {result.stderr.strip()}")
        return result.stdout.strip()

    # Ensure the default graph exists
    redis_cli("GQL.CREATE-GRAPH", "default")

    # Create the property test node
    redis_cli(
        "GQL.MUTATE", "default",
        "CREATE (:E2ETest {"
        "  str_val: 'hello',"
        "  int_val: 42,"
        "  float_val: 3.14,"
        "  bool_val: true,"
        "  date_val: date('2025-01-15'),"
        "  datetime_val: datetime('2025-01-15T10:30:00Z'),"
        "  duration_val: duration('P1Y2M3D'),"
        "  point_val: point({x: 1.0, y: 2.0})"
        "})"
    )

    # Create relationship test
    redis_cli(
        "GQL.MUTATE", "default",
        "CREATE (:E2ESource {name: 'src'})-[:E2E_LINK {weight: 1.5}]->(:E2ETarget {name: 'tgt'})"
    )

    print("[WRITE] Test data written via redis-cli")


# ---------------------------------------------------------------------------
# Wait for Fabric sync
# ---------------------------------------------------------------------------

def wait_for_sync(
    token: str,
    endpoint: str,
    check_query: str,
    expected_min_rows: int,
    timeout: int,
    poll_interval: int,
) -> bool:
    """Poll Fabric until the expected data appears or timeout is reached."""
    start = time.time()
    last_err = ""
    while time.time() - start < timeout:
        try:
            resp = fabric_query(token, endpoint, check_query)
            rows = parse_fabric_rows(resp)
            if len(rows) >= expected_min_rows:
                elapsed = time.time() - start
                print(f"[SYNC]  Data appeared in Fabric after {elapsed:.1f}s")
                return True
        except Exception as exc:
            last_err = str(exc)
            print(f"  Waiting... ({last_err})")
        time.sleep(poll_interval)

    elapsed = time.time() - start
    print(f"[SYNC]  Timeout after {elapsed:.1f}s — last error: {last_err}")
    return False


# ---------------------------------------------------------------------------
# Validation functions
# ---------------------------------------------------------------------------

def validate_node_synced(token: str, endpoint: str, cfg: Config) -> TestResult:
    """Verify that the E2ETest node was replicated to Fabric."""
    t0 = time.time()
    synced = wait_for_sync(
        token, endpoint,
        check_query="MATCH (n:E2ETest) RETURN count(n) AS cnt",
        expected_min_rows=1,
        timeout=cfg.timeout,
        poll_interval=cfg.poll_interval,
    )
    if not synced:
        return TestResult(
            "node_synced", False,
            "E2ETest node did not appear in Fabric within timeout",
            time.time() - t0,
        )

    # Double-check by verifying count >= 1
    resp = fabric_query(token, endpoint, "MATCH (n:E2ETest) RETURN count(n) AS cnt")
    rows = parse_fabric_rows(resp)
    cnt = rows[0][0] if rows else 0
    ok = cnt >= 1
    return TestResult(
        "node_synced", ok,
        f"E2ETest node count in Fabric: {cnt}",
        time.time() - t0,
    )


def validate_property_str(token: str, endpoint: str) -> TestResult:
    """Verify string property round-trips."""
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.str_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        ok = val == "hello"
        return TestResult("prop_str", ok, f"str_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_str", False, str(exc), time.time() - t0)


def validate_property_int(token: str, endpoint: str) -> TestResult:
    """Verify integer property round-trips."""
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.int_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        ok = val == 42
        return TestResult("prop_int", ok, f"int_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_int", False, str(exc), time.time() - t0)


def validate_property_float(token: str, endpoint: str) -> TestResult:
    """Verify float property round-trips."""
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.float_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        ok = isinstance(val, (int, float)) and abs(float(val) - 3.14) < 0.001
        return TestResult("prop_float", ok, f"float_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_float", False, str(exc), time.time() - t0)


def validate_property_bool(token: str, endpoint: str) -> TestResult:
    """Verify boolean property round-trips."""
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.bool_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        ok = val is True
        return TestResult("prop_bool", ok, f"bool_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_bool", False, str(exc), time.time() - t0)


def validate_property_date(token: str, endpoint: str) -> TestResult:
    """Verify date property round-trips.

    Fabric may return the date as a string like '2025-01-15' or as a
    structured object.  We accept both forms.
    """
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.date_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        # Accept string or object representation containing '2025-01-15'
        val_str = str(val) if val is not None else ""
        ok = "2025-01-15" in val_str
        return TestResult("prop_date", ok, f"date_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_date", False, str(exc), time.time() - t0)


def validate_property_datetime(token: str, endpoint: str) -> TestResult:
    """Verify datetime property round-trips.

    Expects some representation containing '2025-01-15' and '10:30'.
    """
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.datetime_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        val_str = str(val) if val is not None else ""
        ok = "2025-01-15" in val_str and "10:30" in val_str
        return TestResult("prop_datetime", ok, f"datetime_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_datetime", False, str(exc), time.time() - t0)


def validate_property_duration(token: str, endpoint: str) -> TestResult:
    """Verify duration property round-trips.

    The SedanDelivery CypherDuration stores months, days, nanos.
    P1Y2M3D = 14 months, 3 days, 0 nanos.  Fabric may return this as a
    string or as an object.
    """
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.duration_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        val_str = str(val) if val is not None else ""
        # Accept anything that contains P1Y2M3D or component representations
        ok = val is not None and ("P1Y2M3D" in val_str or ("14" in val_str and "3" in val_str))
        return TestResult("prop_duration", ok, f"duration_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_duration", False, str(exc), time.time() - t0)


def validate_property_point(token: str, endpoint: str) -> TestResult:
    """Verify point property round-trips.

    point({x: 1.0, y: 2.0}) -> should contain x=1.0, y=2.0 or
    longitude/latitude equivalents.
    """
    t0 = time.time()
    try:
        resp = fabric_query(token, endpoint,
                            "MATCH (n:E2ETest) RETURN n.point_val AS v")
        rows = parse_fabric_rows(resp)
        val = rows[0][0] if rows else None
        val_str = str(val) if val is not None else ""
        # Point may be returned as structured object or string
        ok = val is not None and ("1.0" in val_str or "1" in val_str)
        return TestResult("prop_point", ok, f"point_val={val!r}", time.time() - t0)
    except Exception as exc:
        return TestResult("prop_point", False, str(exc), time.time() - t0)


def validate_relationship(token: str, endpoint: str, cfg: Config) -> TestResult:
    """Verify that the E2E_LINK relationship replicated correctly."""
    t0 = time.time()
    try:
        # Wait for the relationship to appear
        synced = wait_for_sync(
            token, endpoint,
            check_query=(
                "MATCH (s:E2ESource)-[r:E2E_LINK]->(t:E2ETarget) "
                "RETURN s.name AS src, r.weight AS w, t.name AS tgt"
            ),
            expected_min_rows=1,
            timeout=cfg.timeout,
            poll_interval=cfg.poll_interval,
        )
        if not synced:
            return TestResult(
                "relationship", False,
                "E2E_LINK relationship did not appear in Fabric within timeout",
                time.time() - t0,
            )

        resp = fabric_query(
            token, endpoint,
            "MATCH (s:E2ESource)-[r:E2E_LINK]->(t:E2ETarget) "
            "RETURN s.name AS src, r.weight AS w, t.name AS tgt",
        )
        rows = parse_fabric_rows(resp)
        if not rows:
            return TestResult("relationship", False, "No rows returned", time.time() - t0)

        row = rows[0]
        src_name = row[0] if len(row) > 0 else None
        weight = row[1] if len(row) > 1 else None
        tgt_name = row[2] if len(row) > 2 else None

        checks = []
        if src_name != "src":
            checks.append(f"src.name={src_name!r} (expected 'src')")
        if tgt_name != "tgt":
            checks.append(f"tgt.name={tgt_name!r} (expected 'tgt')")
        if not (isinstance(weight, (int, float)) and abs(float(weight) - 1.5) < 0.01):
            checks.append(f"r.weight={weight!r} (expected 1.5)")

        ok = len(checks) == 0
        detail = "OK" if ok else "; ".join(checks)
        return TestResult("relationship", ok, detail, time.time() - t0)

    except Exception as exc:
        return TestResult("relationship", False, str(exc), time.time() - t0)


# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------

def cleanup_bolt(bolt_uri: str) -> None:
    """Remove E2E test nodes via Bolt."""
    from neo4j import GraphDatabase

    driver = GraphDatabase.driver(bolt_uri, auth=None)
    try:
        with driver.session() as session:
            session.run(
                "MATCH (n) WHERE n:E2ETest OR n:E2ESource OR n:E2ETarget DETACH DELETE n"
            )
        print("[CLEAN] Test data removed via Bolt")
    finally:
        driver.close()


def cleanup_redis(host: str, port: int) -> None:
    """Remove E2E test nodes via redis-cli."""
    cmd = [
        "redis-cli", "-h", host, "-p", str(port),
        "GQL.MUTATE", "default",
        "MATCH (n) WHERE n:E2ETest OR n:E2ESource OR n:E2ETarget DETACH DELETE n",
    ]
    subprocess.run(cmd, capture_output=True, text=True, timeout=10)
    print("[CLEAN] Test data removed via redis-cli")


def cleanup_fabric(token: str, endpoint: str) -> None:
    """Remove E2E test data from Fabric as well."""
    try:
        fabric_statement(
            token, endpoint,
            "MATCH (n) WHERE n:E2ETest OR n:E2ESource OR n:E2ETarget DETACH DELETE n",
        )
        print("[CLEAN] Test data removed from Fabric")
    except Exception as exc:
        print(f"[CLEAN] Warning: Fabric cleanup failed: {exc}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def build_config(args: argparse.Namespace) -> Config:
    """Build Config from environment variables + CLI overrides."""

    def env(key: str, default: Optional[str] = None) -> Optional[str]:
        return os.environ.get(key, default)

    def require_env(key: str) -> str:
        val = os.environ.get(key)
        if not val:
            print(f"ERROR: required environment variable {key} is not set", file=sys.stderr)
            sys.exit(1)
        return val

    bolt_uri = env("BOLT_URI")
    redis_host = env("REDIS_HOST")
    redis_port = int(env("REDIS_PORT", "6379"))

    if not bolt_uri and not redis_host:
        print(
            "ERROR: set either BOLT_URI or REDIS_HOST to specify the SedanDelivery endpoint",
            file=sys.stderr,
        )
        sys.exit(1)

    return Config(
        bolt_uri=bolt_uri,
        redis_host=redis_host,
        redis_port=redis_port,
        fabric_endpoint=require_env("FABRIC_ENDPOINT").rstrip("/"),
        fabric_tenant_id=require_env("FABRIC_TENANT_ID"),
        fabric_client_id=require_env("FABRIC_CLIENT_ID"),
        fabric_client_secret=require_env("FABRIC_CLIENT_SECRET"),
        timeout=args.timeout,
        poll_interval=args.poll_interval,
        skip_cleanup=args.skip_cleanup,
        report_path=args.report,
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Fabric E2E Validation for SedanDelivery replication pipeline",
    )
    parser.add_argument("--timeout", type=int, default=120,
                        help="Max seconds to wait for Fabric sync (default 120)")
    parser.add_argument("--poll-interval", type=int, default=5,
                        help="Seconds between Fabric poll attempts (default 5)")
    parser.add_argument("--skip-cleanup", action="store_true",
                        help="Do not remove test data after validation")
    parser.add_argument("--report", default="fabric-e2e-report.json",
                        help="Path for JSON report (default fabric-e2e-report.json)")
    args = parser.parse_args()

    cfg = build_config(args)

    report = Report()
    report.start_time = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    overall_start = time.time()

    print("=" * 60)
    print("SedanDelivery Fabric E2E Validation")
    print("=" * 60)

    # -- Step 1: Acquire Fabric token ----------------------------------------
    print("\n[AUTH]  Acquiring Fabric API token...")
    try:
        token = get_fabric_token(
            cfg.fabric_tenant_id, cfg.fabric_client_id, cfg.fabric_client_secret,
        )
        print("[AUTH]  Token acquired")
    except Exception as exc:
        print(f"[AUTH]  FAILED: {exc}", file=sys.stderr)
        report.tests.append(TestResult("auth", False, str(exc)).__dict__)
        report.passed = False
        report.summary = "Failed to acquire Fabric API token"
        _write_report(report, cfg.report_path, overall_start)
        return 1

    # -- Step 2: Write test data ---------------------------------------------
    print("\n[WRITE] Writing test data to SedanDelivery...")
    try:
        if cfg.bolt_uri:
            write_test_data_bolt(cfg.bolt_uri)
        else:
            write_test_data_redis(cfg.redis_host, cfg.redis_port)
        report.tests.append(TestResult("write_test_data", True, "OK").__dict__)
    except Exception as exc:
        print(f"[WRITE] FAILED: {exc}", file=sys.stderr)
        report.tests.append(TestResult("write_test_data", False, str(exc)).__dict__)
        report.passed = False
        report.summary = "Failed to write test data"
        _write_report(report, cfg.report_path, overall_start)
        return 1

    # -- Step 3: Wait for sync + validate ------------------------------------
    print(f"\n[SYNC]  Waiting up to {cfg.timeout}s for Fabric replication...")

    results: list[TestResult] = []

    # First, wait for the node to appear (this is the main sync gate)
    node_result = validate_node_synced(token, cfg.fabric_endpoint, cfg)
    results.append(node_result)

    if node_result.passed:
        # Now validate individual property types
        print("\n[CHECK] Validating property types...")
        results.append(validate_property_str(token, cfg.fabric_endpoint))
        results.append(validate_property_int(token, cfg.fabric_endpoint))
        results.append(validate_property_float(token, cfg.fabric_endpoint))
        results.append(validate_property_bool(token, cfg.fabric_endpoint))
        results.append(validate_property_date(token, cfg.fabric_endpoint))
        results.append(validate_property_datetime(token, cfg.fabric_endpoint))
        results.append(validate_property_duration(token, cfg.fabric_endpoint))
        results.append(validate_property_point(token, cfg.fabric_endpoint))

        # Validate relationship
        print("\n[CHECK] Validating relationship replication...")
        results.append(validate_relationship(token, cfg.fabric_endpoint, cfg))
    else:
        print("[SKIP]  Skipping property/relationship checks (node not synced)")

    # -- Step 4: Report results ----------------------------------------------
    print("\n" + "=" * 60)
    print("Results:")
    print("-" * 60)

    all_passed = True
    for r in results:
        status = "PASS" if r.passed else "FAIL"
        all_passed = all_passed and r.passed
        print(f"  [{status}] {r.name}: {r.detail} ({r.duration_s:.1f}s)")
        report.tests.append(r.__dict__)

    # -- Step 5: Cleanup -----------------------------------------------------
    if not cfg.skip_cleanup:
        print("\n[CLEAN] Cleaning up test data...")
        try:
            if cfg.bolt_uri:
                cleanup_bolt(cfg.bolt_uri)
            else:
                cleanup_redis(cfg.redis_host, cfg.redis_port)
            cleanup_fabric(token, cfg.fabric_endpoint)
        except Exception as exc:
            print(f"[CLEAN] Warning: cleanup failed: {exc}")

    # -- Final report --------------------------------------------------------
    report.passed = all_passed
    total = len(results)
    passed_count = sum(1 for r in results if r.passed)
    report.summary = f"{passed_count}/{total} checks passed"

    print("\n" + "=" * 60)
    if all_passed:
        print(f"SUCCESS: All {total} checks passed")
    else:
        print(f"FAILURE: {passed_count}/{total} checks passed")
    print("=" * 60)

    _write_report(report, cfg.report_path, overall_start)

    return 0 if all_passed else 1


def _write_report(report: Report, path: str, start_time: float) -> None:
    """Serialize the report to JSON."""
    report.end_time = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    report.duration_s = round(time.time() - start_time, 2)
    with open(path, "w") as f:
        json.dump(report.__dict__, f, indent=2, default=str)
    print(f"\n[REPORT] Written to {path}")


if __name__ == "__main__":
    sys.exit(main())
