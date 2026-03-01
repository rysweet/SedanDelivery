# App 1: Neodash — Security Dashboard for SedanDelivery

Neodash is an open-source dashboard builder by Neo4j Labs. This setup connects
it to SedanDelivery's Bolt v4.4 server to visualize the MITRE ATT&CK dataset.

## Prerequisites

- SedanDelivery Bolt server running on `localhost:7687`
- MITRE ATT&CK dataset loaded (86 nodes, 142 relationships)
- Docker installed and running

## Quick Start

```bash
# 1. Start Neodash
./run.sh

# 2. Open in browser
open http://localhost:5005

# 3. Connect to SedanDelivery
#    In the Neodash connection screen:
#      Protocol: bolt
#      Host:     localhost
#      Port:     7687
#      Username: neo4j
#      Password: neo4j

# 4. Import the pre-built dashboard
#    Click the settings/load icon > Load Dashboard > paste dashboard.json contents
```

## Alternative: Docker Run (no Compose)

If `docker compose` is not available, the `run.sh` script uses `docker run`
directly. You can also run it manually:

```bash
docker run -d \
  --name neodash-sedandelivery \
  -p 5005:5005 \
  --add-host=host.docker.internal:host-gateway \
  -e ssoEnabled="false" \
  -e standaloneProtocol="bolt" \
  -e standaloneHost="172.17.0.1" \
  -e standalonePort="7687" \
  -e standaloneDatabase="" \
  -e standaloneUsername="neo4j" \
  -e standalonePassword="neo4j" \
  --restart unless-stopped \
  neo4jlabs/neodash:latest
```

On macOS or WSL, replace `172.17.0.1` with `host.docker.internal`.

## Alternative: Docker Compose

```bash
docker compose up -d
```

## Files

| File                 | Purpose                                        |
|----------------------|------------------------------------------------|
| `docker-compose.yml` | Docker Compose service definition              |
| `dashboard.json`     | Pre-built MITRE ATT&CK security dashboard      |
| `run.sh`             | Launch script with checks and status output     |
| `verify.sh`          | Verification script to confirm everything works |
| `README.md`          | This file                                       |

## Dashboard Pages

### Page 1: Threat Landscape Overview

- **Threat Groups Table** — All threat groups with country, first seen date, and aliases
- **Techniques per Tactic (Bar Chart)** — Count of techniques mapped to each ATT&CK tactic
- **Attack Techniques Graph** — Interactive graph of ThreatGroup -[USES]-> Technique relationships

### Page 2: Attack Paths & Kill Chains

- **Full Attack Graph** — Complete graph with threat groups, techniques, software, and tactics
- **CVE Severity Table** — All CVEs sorted by CVSS score
- **Software & Malware Arsenal** — Catalog of tools, malware, and exploits

### Page 3: Mitigations & Coverage

- **Mitigations Inventory** — All mitigations with the techniques they protect against
- **Mitigation Coverage Graph** — Visual map of mitigation-to-technique-to-tactic relationships

## Importing the Dashboard

1. Open Neodash at `http://localhost:5005`
2. Connect to the database (settings above)
3. Create a new dashboard or open settings
4. Use the Load/Import feature to load `dashboard.json`
5. Each page's cards will auto-execute their Cypher queries

## Verifying the Setup

```bash
./verify.sh
```

This checks:
1. Docker container is running
2. Neodash HTTP endpoint responds
3. Page content looks correct
4. SedanDelivery Bolt server is reachable
5. Container logs show no errors

## Stopping

```bash
./run.sh --stop
# or
docker compose down
```

## Troubleshooting

**Neodash cannot connect to database**
- Verify Bolt server is running: `nc -z localhost 7687`
- Check container can reach host: `docker exec neodash-sedandelivery wget -qO- http://172.17.0.1:7687 || echo "unreachable"`
- On Linux, ensure `--add-host=host.docker.internal:host-gateway` is set

**Dashboard shows empty cards**
- Verify the MITRE ATT&CK data is loaded: connect via `cypher-shell` or the Bolt
  client and run `MATCH (n) RETURN labels(n), count(n)`
- Check that queries match the actual property names in the dataset

**Port 5005 already in use**
- Change the host port in `docker-compose.yml` or `run.sh` (e.g., `5006:5005`)

## Connection Details

| Setting    | Value           |
|------------|-----------------|
| Protocol   | bolt            |
| Host       | localhost       |
| Port       | 7687            |
| Username   | neo4j           |
| Password   | neo4j           |
| Auth       | None (any creds work) |

SedanDelivery's Bolt server accepts any username/password — there is no
authentication. Neodash requires these fields to be filled, so use the
placeholder values above.
