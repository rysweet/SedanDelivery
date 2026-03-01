#!/usr/bin/env python3
"""Lightweight proxy server for neovis.js gallery app.

The browser neo4j-driver requires WebSocket transport, which sedandelivery-bolt
does not yet support. This proxy bridges the gap: it queries Bolt from Python
(TCP) and serves results as JSON to the browser frontend via HTTP.

Routes:
    GET /             Serve the visualization HTML
    POST /query       Execute a Cypher query and return vis.js-formatted JSON
    GET /health       Health check
"""
import json
import os
from http.server import HTTPServer, SimpleHTTPRequestHandler
from urllib.parse import parse_qs

from neo4j import GraphDatabase

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:7687")
PORT = int(os.environ.get("PORT", "8080"))

driver = GraphDatabase.driver(BOLT_URI, auth=None)


def run_cypher(cypher):
    """Execute Cypher and return nodes + edges for vis.js."""
    nodes = []
    edges = []
    node_set = set()

    with driver.session() as session:
        result = session.run(cypher)
        for record in result:
            for value in record.values():
                if hasattr(value, "labels"):  # Node
                    nid = value.element_id if hasattr(value, "element_id") else str(value.id)
                    if nid not in node_set:
                        node_set.add(nid)
                        labels = list(value.labels)
                        props = dict(value)
                        label_text = props.get("name", props.get("id", labels[0] if labels else str(nid)))
                        nodes.append({
                            "id": nid,
                            "label": str(label_text),
                            "group": labels[0] if labels else "Unknown",
                            "title": "<br>".join(f"<b>{k}</b>: {v}" for k, v in props.items()),
                        })
                elif hasattr(value, "type"):  # Relationship
                    start = value.start_node.element_id if hasattr(value.start_node, "element_id") else str(value.start_node.id)
                    end = value.end_node.element_id if hasattr(value.end_node, "element_id") else str(value.end_node.id)
                    edges.append({
                        "from": start,
                        "to": end,
                        "label": value.type,
                        "arrows": "to",
                    })

    return {"nodes": nodes, "edges": edges}


class GalleryHandler(SimpleHTTPRequestHandler):
    def do_POST(self):
        if self.path == "/query":
            length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(length).decode()
            try:
                data = json.loads(body)
                cypher = data.get("cypher", "")
                if not cypher:
                    self._json_response({"error": "No cypher provided"}, 400)
                    return
                result = run_cypher(cypher)
                self._json_response(result)
            except Exception as e:
                self._json_response({"error": str(e), "nodes": [], "edges": []}, 500)
        else:
            self.send_error(404)

    def do_GET(self):
        if self.path == "/health":
            self._json_response({"status": "ok"})
        else:
            super().do_GET()

    def _json_response(self, data, code=200):
        payload = json.dumps(data).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, format, *args):
        pass  # Suppress request logs


if __name__ == "__main__":
    print(f"[*] neovis.js gallery server on http://localhost:{PORT}")
    print(f"[*] Bolt backend: {BOLT_URI}")
    httpd = HTTPServer(("0.0.0.0", PORT), GalleryHandler)
    httpd.serve_forever()
