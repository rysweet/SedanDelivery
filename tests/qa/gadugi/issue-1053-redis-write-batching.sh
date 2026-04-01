#!/usr/bin/env bash
set -euo pipefail

pick_free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

redis_port=$(pick_free_port)
api_port=$(pick_free_port)
api_url="http://127.0.0.1:${api_port}"
redis_url="redis://127.0.0.1:${redis_port}"
startup_timeout_secs="${GQL_API_STARTUP_TIMEOUT_SECS:-180}"
tmp_dir=$(mktemp -d /tmp/sd2-api-1053.XXXXXX)
api_log="${tmp_dir}/sd2-api.log"
health_log="${tmp_dir}/sd2-api-health.json"
create_graph_log="${tmp_dir}/create-graph.json"
create_log="${tmp_dir}/create.json"
merge_log="${tmp_dir}/merge.json"
self_alias_log="${tmp_dir}/self-alias.json"
edge_alias_log="${tmp_dir}/edge-alias.json"
stats_log="${tmp_dir}/stats.json"
batch_count_log="${tmp_dir}/batch-count.json"
edge_count_log="${tmp_dir}/edge-count.json"
merge_count_log="${tmp_dir}/merge-count.json"
self_loop_count_log="${tmp_dir}/self-loop-count.json"
edge_alias_query_log="${tmp_dir}/edge-alias-query.json"

container_id=$(docker run -d -p "${redis_port}:6379" redis:7-alpine)
api_pid=''

cleanup() {
  if [ -n "$api_pid" ]; then
    kill "$api_pid" >/dev/null 2>&1 || true
    wait "$api_pid" >/dev/null 2>&1 || true
  fi
  docker rm -f "$container_id" >/dev/null 2>&1 || true
  rm -rf "$tmp_dir"
}

trap cleanup EXIT

sleep 2
GQL_REDIS_URL="$redis_url" \
GQL_API_BIND_ADDR="127.0.0.1:${api_port}" \
cargo run -p sd2-api >"$api_log" 2>&1 &
api_pid=$!

for i in $(seq 1 "$startup_timeout_secs"); do
  if curl -sf "${api_url}/health" >"$health_log"; then
    echo API_HEALTH_OK
    break
  fi
  sleep 1
  if [ "$i" -eq "$startup_timeout_secs" ]; then
    echo API_HEALTH_TIMEOUT
    cat "$api_log"
    exit 1
  fi
done

curl -sf -X DELETE "${api_url}/graphs/qa-1053" >/dev/null || true
curl -sf -X POST "${api_url}/graphs" \
  -H 'content-type: application/json' \
  -d '{"name":"qa-1053"}' >"$create_graph_log"
echo GRAPH_CREATED

curl -sf -X POST "${api_url}/graphs/qa-1053/mutate" \
  -H 'content-type: application/json' \
  -d '{"cypher":"CREATE (a:BatchPerson {name:\"Alice\"})-[:KNOWS]->(b:BatchPerson {name:\"Bob\"}), (c:BatchPerson {name:\"Charlie\"})-[:KNOWS]->(d:BatchPerson {name:\"Dana\"})"}' \
  >"$create_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/mutate" \
  -H 'content-type: application/json' \
  -d '{"cypher":"UNWIND [1,2] AS i MERGE (n:MergeThing {key:\"shared\"})"}' \
  >"$merge_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/mutate" \
  -H 'content-type: application/json' \
  -d '{"cypher":"CREATE (s:SelfLoop {name:\"self\"})-[r:SELF]->(s)"}' \
  >"$self_alias_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/mutate" \
  -H 'content-type: application/json' \
  -d '{"cypher":"CREATE (x:AliasEdge {name:\"x\"})-[r1:LINK {weight:7}]->(y:AliasEdge {name:\"y\"})-[r2:COPY {copied:r1.weight}]->(z:AliasEdge {name:\"z\"})"}' \
  >"$edge_alias_log"

curl -sf "${api_url}/graphs/qa-1053/stats" >"$stats_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/query" \
  -H 'content-type: application/json' \
  -d '{"cypher":"MATCH (n:BatchPerson) RETURN count(n)"}' \
  >"$batch_count_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/query" \
  -H 'content-type: application/json' \
  -d '{"cypher":"MATCH ()-[r:KNOWS]->() RETURN count(r)"}' \
  >"$edge_count_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/query" \
  -H 'content-type: application/json' \
  -d '{"cypher":"MATCH (n:MergeThing) RETURN count(n)"}' \
  >"$merge_count_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/query" \
  -H 'content-type: application/json' \
  -d '{"cypher":"MATCH (n:SelfLoop)-[r:SELF]->(n) RETURN count(r)"}' \
  >"$self_loop_count_log"

curl -sf -X POST "${api_url}/graphs/qa-1053/query" \
  -H 'content-type: application/json' \
  -d '{"cypher":"MATCH ()-[r:COPY]->() RETURN r.copied"}' \
  >"$edge_alias_query_log"

CREATE_LOG="$create_log" \
MERGE_LOG="$merge_log" \
SELF_ALIAS_LOG="$self_alias_log" \
EDGE_ALIAS_LOG="$edge_alias_log" \
STATS_LOG="$stats_log" \
BATCH_COUNT_LOG="$batch_count_log" \
EDGE_COUNT_LOG="$edge_count_log" \
MERGE_COUNT_LOG="$merge_count_log" \
SELF_LOOP_COUNT_LOG="$self_loop_count_log" \
EDGE_ALIAS_QUERY_LOG="$edge_alias_query_log" \
python3 - <<'PY'
import json
import os
from pathlib import Path

create = json.loads(Path(os.environ['CREATE_LOG']).read_text())
merge = json.loads(Path(os.environ['MERGE_LOG']).read_text())
self_alias = json.loads(Path(os.environ['SELF_ALIAS_LOG']).read_text())
edge_alias = json.loads(Path(os.environ['EDGE_ALIAS_LOG']).read_text())
stats = json.loads(Path(os.environ['STATS_LOG']).read_text())
batch_q = json.loads(Path(os.environ['BATCH_COUNT_LOG']).read_text())
edge_q = json.loads(Path(os.environ['EDGE_COUNT_LOG']).read_text())
merge_q = json.loads(Path(os.environ['MERGE_COUNT_LOG']).read_text())
self_loop_q = json.loads(Path(os.environ['SELF_LOOP_COUNT_LOG']).read_text())
edge_alias_q = json.loads(Path(os.environ['EDGE_ALIAS_QUERY_LOG']).read_text())

assert create['nodesCreated'] == 4, create
assert create['edgesCreated'] == 2, create
print('CREATE_MUTATION_OK')

assert merge['nodesCreated'] == 1, merge
print('MERGE_MUTATION_OK')

assert self_alias['nodesCreated'] == 1 and self_alias['edgesCreated'] == 1, self_alias
print('SELF_ALIAS_CREATE_OK')

assert edge_alias['nodesCreated'] == 3 and edge_alias['edgesCreated'] == 2, edge_alias
print('EDGE_ALIAS_CREATE_OK')

assert stats['nodeCount'] == 9 and stats['edgeCount'] == 5, stats
print('GRAPH_STATS_OK')

assert batch_q['rows'] == [{'count(n)': 4}], batch_q
print('BATCH_NODE_COUNT_OK')

assert edge_q['rows'] == [{'count(r)': 2}], edge_q
print('EDGE_COUNT_OK')

assert merge_q['rows'] == [{'count(n)': 1}], merge_q
print('MERGE_NODE_COUNT_OK')

assert self_loop_q['rows'] == [{'count(r)': 1}], self_loop_q
print('SELF_LOOP_COUNT_OK')

assert edge_alias_q['rows'] == [{'r.copied': 7}], edge_alias_q
print('EDGE_ALIAS_PROPERTY_OK')

print('QA_1053_CREATE_AND_MERGE_OK')
PY
