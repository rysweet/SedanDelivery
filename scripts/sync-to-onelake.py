#!/usr/bin/env python3
"""Sync SedanDelivery graph data to Microsoft Fabric via OneLake.

Reads all nodes and edges from the Valkey GQL module, writes NDJSON files,
uploads to a Fabric Lakehouse via the OneLake DataLake API, and triggers
LoadTable + GraphModel refresh.

This is the bridge between the SedanDelivery graph engine (Valkey+GQL module)
and Microsoft Fabric Graph analytics.

Environment variables (required):
    REDIS_HOST              Valkey hostname
    REDIS_PORT              Valkey port (default 6379)
    REDIS_PASSWORD          Valkey password
    FABRIC_TENANT_ID        Azure AD tenant ID
    FABRIC_CLIENT_ID        Service principal app ID
    FABRIC_CLIENT_SECRET    Service principal secret
    FABRIC_WORKSPACE_ID     Fabric workspace ID
    FABRIC_LAKEHOUSE_ID     Fabric lakehouse ID
    FABRIC_GRAPHMODEL_ID    Fabric GraphModel ID (optional, for refresh)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
from typing import Optional


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

def require_env(key: str) -> str:
    val = os.environ.get(key)
    if not val:
        print(f"ERROR: {key} not set", file=sys.stderr)
        sys.exit(1)
    return val


# ---------------------------------------------------------------------------
# Auth
# ---------------------------------------------------------------------------

def get_fabric_token(tenant_id: str, client_id: str, client_secret: str) -> str:
    """Acquire bearer token for Fabric API + OneLake."""
    import requests
    url = f"https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token"
    resp = requests.post(url, data={
        "grant_type": "client_credentials",
        "client_id": client_id,
        "client_secret": client_secret,
        "scope": "https://storage.azure.com/.default",
    }, timeout=30)
    resp.raise_for_status()
    return resp.json()["access_token"]


def get_fabric_api_token(tenant_id: str, client_id: str, client_secret: str) -> str:
    """Acquire bearer token for Fabric REST API (LoadTable, RefreshGraph)."""
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


# ---------------------------------------------------------------------------
# Valkey data extraction
# ---------------------------------------------------------------------------

def redis_cli(host: str, port: int, password: Optional[str], *args: str) -> str:
    """Execute a redis-cli command and return stdout."""
    cmd = ["redis-cli", "-h", host, "-p", str(port)]
    if password:
        cmd.extend(["-a", password, "--no-auth-warning"])
    cmd.extend(list(args))
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    if result.returncode != 0:
        raise RuntimeError(f"redis-cli failed: {result.stderr.strip()}")
    return result.stdout.strip()


def extract_nodes(host: str, port: int, password: Optional[str], graph: str = "default") -> list[dict]:
    """Extract all nodes from a SedanDelivery graph via GQL.QUERY."""
    raw = redis_cli(host, port, password,
                    "GQL.QUERY", graph,
                    "MATCH (n) RETURN id(n) AS node_id, labels(n) AS labels, properties(n) AS props")
    if not raw:
        return []

    nodes = []
    for line in raw.split("\n"):
        line = line.strip()
        if not line or line.startswith("node_id") or line.startswith("---"):
            continue
        # GQL.QUERY returns tab-separated values
        parts = line.split("\t")
        if len(parts) >= 3:
            try:
                node_id = parts[0].strip()
                labels_str = parts[1].strip()
                props_str = parts[2].strip()
                node = {
                    "node_id": node_id,
                    "labels": labels_str,
                    "properties_json": props_str,
                }
                # Try to parse properties JSON and flatten common fields
                try:
                    props = json.loads(props_str) if props_str.startswith("{") else {}
                    for k, v in props.items():
                        if isinstance(v, (str, int, float, bool)):
                            node[k] = v
                except (json.JSONDecodeError, TypeError):
                    pass
                nodes.append(node)
            except (IndexError, ValueError):
                continue
    return nodes


def extract_edges(host: str, port: int, password: Optional[str], graph: str = "default") -> list[dict]:
    """Extract all edges from a SedanDelivery graph via GQL.QUERY."""
    raw = redis_cli(host, port, password,
                    "GQL.QUERY", graph,
                    "MATCH (s)-[r]->(t) RETURN id(r) AS edge_id, type(r) AS edge_type, "
                    "id(s) AS source_id, id(t) AS target_id, properties(r) AS props")
    if not raw:
        return []

    edges = []
    for line in raw.split("\n"):
        line = line.strip()
        if not line or line.startswith("edge_id") or line.startswith("---"):
            continue
        parts = line.split("\t")
        if len(parts) >= 5:
            try:
                edge = {
                    "edge_id": parts[0].strip(),
                    "edge_type": parts[1].strip(),
                    "source_id": parts[2].strip(),
                    "target_id": parts[3].strip(),
                    "properties_json": parts[4].strip(),
                }
                edges.append(edge)
            except (IndexError, ValueError):
                continue
    return edges


# ---------------------------------------------------------------------------
# NDJSON writing
# ---------------------------------------------------------------------------

def write_ndjson(rows: list[dict], path: str) -> int:
    """Write rows as newline-delimited JSON. Returns row count."""
    with open(path, "w") as f:
        for row in rows:
            f.write(json.dumps(row, default=str) + "\n")
    return len(rows)


# ---------------------------------------------------------------------------
# OneLake upload
# ---------------------------------------------------------------------------

def upload_to_onelake(
    token: str,
    workspace_id: str,
    lakehouse_id: str,
    local_path: str,
    remote_path: str,
) -> None:
    """Upload a file to OneLake via DataLake Storage API."""
    import requests

    onelake_base = "https://onelake.dfs.fabric.microsoft.com"
    # Create file
    create_url = (
        f"{onelake_base}/{workspace_id}/{lakehouse_id}/{remote_path}"
        f"?resource=file&overwrite=true"
    )
    resp = requests.put(create_url, headers={
        "Authorization": f"Bearer {token}",
        "Content-Length": "0",
    }, timeout=30)
    resp.raise_for_status()

    # Upload content
    with open(local_path, "rb") as f:
        data = f.read()

    append_url = (
        f"{onelake_base}/{workspace_id}/{lakehouse_id}/{remote_path}"
        f"?action=append&position=0"
    )
    resp = requests.patch(append_url, headers={
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/octet-stream",
    }, data=data, timeout=60)
    resp.raise_for_status()

    # Flush
    flush_url = (
        f"{onelake_base}/{workspace_id}/{lakehouse_id}/{remote_path}"
        f"?action=flush&position={len(data)}"
    )
    resp = requests.patch(flush_url, headers={
        "Authorization": f"Bearer {token}",
    }, timeout=30)
    resp.raise_for_status()
    print(f"  Uploaded {len(data)} bytes → {remote_path}")


# ---------------------------------------------------------------------------
# Fabric Lakehouse LoadTable API
# ---------------------------------------------------------------------------

def load_table(
    token: str,
    workspace_id: str,
    lakehouse_id: str,
    table_name: str,
    relative_path: str,
) -> None:
    """Load files from OneLake Files into a Delta table via LoadTable API."""
    import requests

    url = (
        f"https://api.fabric.microsoft.com/v1/workspaces/{workspace_id}"
        f"/lakehouses/{lakehouse_id}/tables/{table_name}/load"
    )
    body = {
        "relativePath": relative_path,
        "pathType": "File",
        "mode": "Overwrite",
        "formatOptions": {
            "format": "Csv",
            "header": True,
            "delimiter": "\n",
        },
    }
    # NDJSON needs special handling — use Csv with newline delimiter as workaround,
    # or just load as Parquet if we convert.
    # Actually, LoadTable supports Parquet natively. Let's use that.
    # For NDJSON, we need "format": "Json"  (unsupported in some versions)
    # Simplest: use Parquet via pyarrow if available, else raw JSON.
    body = {
        "relativePath": relative_path,
        "pathType": "File",
        "mode": "Overwrite",
    }
    resp = requests.post(url, json=body, headers={
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/json",
    }, timeout=60)
    if resp.status_code == 202:
        print(f"  LoadTable accepted for {table_name} (async)")
    else:
        resp.raise_for_status()


# ---------------------------------------------------------------------------
# GraphModel refresh
# ---------------------------------------------------------------------------

def refresh_graph_model(token: str, workspace_id: str, graph_model_id: str) -> None:
    """Trigger a GraphModel refresh so GQL queries see updated data."""
    import requests

    url = (
        f"https://api.fabric.microsoft.com/v1/workspaces/{workspace_id}"
        f"/GraphModels/{graph_model_id}/jobs/RefreshGraph/instances"
    )
    resp = requests.post(url, headers={
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/json",
    }, json={}, timeout=30)
    if resp.status_code in (200, 202):
        print("  GraphModel refresh triggered")
    else:
        print(f"  GraphModel refresh: HTTP {resp.status_code} — {resp.text[:200]}")


# ---------------------------------------------------------------------------
# Parquet conversion (if pyarrow available)
# ---------------------------------------------------------------------------

def write_parquet(rows: list[dict], path: str) -> bool:
    """Write rows as Parquet. Returns True if successful, False if pyarrow unavailable."""
    try:
        import pyarrow as pa
        import pyarrow.parquet as pq
    except ImportError:
        return False

    if not rows:
        # Write empty Parquet with minimal schema
        schema = pa.schema([("_empty", pa.string())])
        table = pa.table({"_empty": []}, schema=schema)
        pq.write_table(table, path)
        return True

    # Build columns from first row's keys
    columns = {}
    for key in rows[0].keys():
        columns[key] = [str(row.get(key, "")) for row in rows]

    table = pa.table(columns)
    pq.write_table(table, path)
    return True


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> int:
    redis_host = require_env("REDIS_HOST")
    redis_port = int(os.environ.get("REDIS_PORT", "6379"))
    redis_password = os.environ.get("REDIS_PASSWORD")
    tenant_id = require_env("FABRIC_TENANT_ID")
    client_id = require_env("FABRIC_CLIENT_ID")
    client_secret = require_env("FABRIC_CLIENT_SECRET")
    workspace_id = require_env("FABRIC_WORKSPACE_ID")
    lakehouse_id = require_env("FABRIC_LAKEHOUSE_ID")
    graph_model_id = os.environ.get("FABRIC_GRAPHMODEL_ID", "")

    print("=" * 60)
    print("SedanDelivery → OneLake Sync")
    print("=" * 60)

    # 1. Extract data from Valkey
    print("\n[EXTRACT] Reading graph data from Valkey...")
    nodes = extract_nodes(redis_host, redis_port, redis_password)
    edges = extract_edges(redis_host, redis_port, redis_password)
    print(f"  Nodes: {len(nodes)}, Edges: {len(edges)}")

    if not nodes and not edges:
        print("  No data to sync — done")
        return 0

    # 2. Write to temp files (prefer Parquet, fall back to NDJSON)
    with tempfile.TemporaryDirectory(prefix="sd2-sync-") as tmpdir:
        nodes_path = os.path.join(tmpdir, "sd2_nodes.parquet")
        edges_path = os.path.join(tmpdir, "sd2_edges.parquet")

        use_parquet = write_parquet(nodes, nodes_path)
        if use_parquet:
            write_parquet(edges, edges_path)
            fmt = "Parquet"
            print(f"  Format: Parquet")
        else:
            nodes_path = os.path.join(tmpdir, "sd2_nodes.json")
            edges_path = os.path.join(tmpdir, "sd2_edges.json")
            write_ndjson(nodes, nodes_path)
            write_ndjson(edges, edges_path)
            fmt = "Json"
            print(f"  Format: NDJSON (pyarrow not available)")

        # 3. Get tokens
        print("\n[AUTH] Acquiring tokens...")
        onelake_token = get_fabric_token(tenant_id, client_id, client_secret)
        fabric_token = get_fabric_api_token(tenant_id, client_id, client_secret)
        print("  Tokens acquired")

        # 4. Upload to OneLake
        print("\n[UPLOAD] Uploading to OneLake...")
        ext = "parquet" if use_parquet else "json"
        nodes_remote = f"Files/graph-import/sd2_nodes.{ext}"
        edges_remote = f"Files/graph-import/sd2_edges.{ext}"

        upload_to_onelake(onelake_token, workspace_id, lakehouse_id, nodes_path, nodes_remote)
        upload_to_onelake(onelake_token, workspace_id, lakehouse_id, edges_path, edges_remote)

        # 5. LoadTable
        print("\n[LOAD] Loading tables in Lakehouse...")
        load_table(fabric_token, workspace_id, lakehouse_id, "sd2_nodes", nodes_remote)
        load_table(fabric_token, workspace_id, lakehouse_id, "sd2_edges", edges_remote)

        # 6. Wait for tables to materialize
        print("  Waiting 15s for Delta tables...")
        time.sleep(15)

        # 7. Refresh GraphModel
        if graph_model_id:
            print("\n[REFRESH] Triggering GraphModel refresh...")
            refresh_graph_model(fabric_token, workspace_id, graph_model_id)

    print("\n" + "=" * 60)
    print(f"SUCCESS: Synced {len(nodes)} nodes + {len(edges)} edges to OneLake")
    print("=" * 60)
    return 0


if __name__ == "__main__":
    sys.exit(main())
