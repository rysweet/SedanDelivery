#!/usr/bin/env bash
# scripts/provision-fabric.sh
# Provisions a Microsoft Fabric Graph Database workspace for SedanDelivery.
# Usage: ./provision-fabric.sh <env-name> <tenant-id> <client-id> <client-secret>
set -euo pipefail

ENV_NAME="${1:?Usage: provision-fabric.sh <env-name> <tenant-id> <client-id> <client-secret> [capacity-id]}"
TENANT_ID="${2:?}"
CLIENT_ID="${3:?}"
CLIENT_SECRET="${4:?}"
CAPACITY_ID="${5:-}"  # Optional: Fabric capacity ID for workspace creation

FABRIC_API="https://api.fabric.microsoft.com/v1"
WORKSPACE_NAME="SD2-${ENV_NAME}"

# Auto-detect capacity if not provided
if [[ -z "$CAPACITY_ID" ]]; then
  echo "No capacity ID provided — will auto-detect first available..."
fi

# -- Get access token ------------------------------------------------------
TOKEN=$(curl -s -X POST "https://login.microsoftonline.com/${TENANT_ID}/oauth2/v2.0/token" \
  --data-urlencode "grant_type=client_credentials" \
  --data-urlencode "client_id=${CLIENT_ID}" \
  --data-urlencode "client_secret=${CLIENT_SECRET}" \
  --data-urlencode "scope=https://api.fabric.microsoft.com/.default" \
  | python3 -c "import json,sys; print(json.load(sys.stdin)['access_token'])")

if [[ -z "$TOKEN" || "$TOKEN" == "null" ]]; then
  echo "::error::Failed to acquire Fabric access token" >&2
  exit 1
fi

AUTH="Authorization: Bearer ${TOKEN}"

# -- Find or create workspace ----------------------------------------------
echo "Looking for workspace '${WORKSPACE_NAME}'..."
# Try both regular and admin API to find workspace (SP may own it but not see it in /workspaces)
WORKSPACE_ID=$(curl -s -H "$AUTH" "${FABRIC_API}/workspaces" \
  | WORKSPACE_NAME="$WORKSPACE_NAME" python3 -c "
import json, sys, os
name = os.environ['WORKSPACE_NAME']
data = json.load(sys.stdin)
for ws in data.get('value', []):
    if ws['displayName'] == name:
        print(ws['id'])
        sys.exit(0)
print('')
")
if [[ -z "$WORKSPACE_ID" ]]; then
  # Fallback: admin API can see workspaces the SP created but isn't a member of
  WORKSPACE_ID=$(curl -s -H "$AUTH" "${FABRIC_API}/admin/workspaces" \
    | WORKSPACE_NAME="$WORKSPACE_NAME" python3 -c "
import json, sys, os
name = os.environ['WORKSPACE_NAME']
data = json.load(sys.stdin)
for ws in data.get('value', []):
    if ws.get('displayName','') == name or ws.get('name','') == name:
        print(ws['id'])
        sys.exit(0)
print('')
" 2>/dev/null || echo "")
fi

if [[ -z "$WORKSPACE_ID" ]]; then
  echo "Workspace '${WORKSPACE_NAME}' not found — creating..."

  # Auto-detect capacity if not provided
  if [[ -z "$CAPACITY_ID" ]]; then
    echo "Querying available Fabric capacities..."
    CAPACITY_ID=$(curl -s -H "$AUTH" "${FABRIC_API}/capacities" \
      | python3 -c "
import json, sys
data = json.load(sys.stdin)
for cap in data.get('value', []):
    state = cap.get('state', '')
    sku = cap.get('sku', '')
    if state == 'Active' and sku.startswith('F'):
        print(cap['id'])
        sys.exit(0)
print('')
" 2>/dev/null || echo "")

    if [[ -z "$CAPACITY_ID" ]]; then
      echo "::warning::No active Fabric capacity found (need F-SKU)"
      echo "FABRIC_ENDPOINT="
      exit 0
    fi
    echo "Auto-detected capacity: ${CAPACITY_ID}"
  fi

  WORKSPACE_ID=$(curl -s -X POST -H "$AUTH" -H "Content-Type: application/json" \
    "${FABRIC_API}/workspaces" \
    -d "{\"displayName\": \"${WORKSPACE_NAME}\", \"capacityId\": \"${CAPACITY_ID}\"}" \
    | python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")

  if [[ -z "$WORKSPACE_ID" ]]; then
    echo "::warning::Failed to create workspace '${WORKSPACE_NAME}'"
    echo "FABRIC_ENDPOINT="
    exit 0
  fi
  echo "Created workspace: ${WORKSPACE_ID}"
else
  echo "Found existing workspace: ${WORKSPACE_ID}"
fi

# -- Ensure SP has workspace access ----------------------------------------
echo "Ensuring SP has workspace member access..."
SP_OBJECT_ID=$(echo "$TOKEN" | python3 -c "
import json, sys, base64
token = sys.stdin.read().strip()
payload = token.split('.')[1]
payload += '=' * (-len(payload) % 4)
data = json.loads(base64.urlsafe_b64decode(payload))
print(data.get('oid', ''))
")
if [[ -n "$SP_OBJECT_ID" ]]; then
  curl -s -X POST -H "$AUTH" -H "Content-Type: application/json" \
    "${FABRIC_API}/workspaces/${WORKSPACE_ID}/roleAssignments" \
    -d "{\"principal\": {\"id\": \"${SP_OBJECT_ID}\", \"type\": \"ServicePrincipal\"}, \"role\": \"Admin\"}" \
    >/dev/null 2>&1 || true
  echo "SP ${SP_OBJECT_ID} granted workspace access (or already had it)"
fi

# -- Find or create Graph item ----------------------------------------------
# Try GraphDatabase first (native Cypher/GQL, preview feature).
# Fall back to GraphQLApi if GraphDatabase is not available on this capacity.
GDB_NAME="sedangraph"
ITEM_TYPE=""
GDB_ID=""

# Helper: find item by type and name
find_item() {
  local type="$1"
  curl -s -H "$AUTH" "${FABRIC_API}/workspaces/${WORKSPACE_ID}/items?type=${type}" \
    | GDB_NAME="$GDB_NAME" python3 -c "
import json, sys, os
name = os.environ['GDB_NAME']
data = json.load(sys.stdin)
for item in data.get('value', []):
    if item['displayName'] == name:
        print(item['id'])
        sys.exit(0)
print('')
" 2>/dev/null || echo ""
}

echo "Looking for Graph Database '${GDB_NAME}'..."

# Check for existing items (try GraphModel first — the native Fabric Graph type)
for SEARCH_TYPE in GraphModel Graph GraphDatabase GraphQLApi; do
  GDB_ID=$(find_item "$SEARCH_TYPE")
  if [[ -n "$GDB_ID" ]]; then
    ITEM_TYPE="$SEARCH_TYPE"
    echo "Found existing ${SEARCH_TYPE}: ${GDB_ID}"
    break
  fi
done

# 3. Create new item (try GraphModel first, fall back to others)
if [[ -z "$GDB_ID" ]]; then
  for TRY_TYPE in GraphModel Graph GraphDatabase GraphQLApi; do
    echo "Creating '${GDB_NAME}' (type=${TRY_TYPE})..."
    CREATE_RESPONSE=$(curl -s -w "\n%{http_code}" -D /tmp/fabric-create-headers -X POST -H "$AUTH" -H "Content-Type: application/json" \
      "${FABRIC_API}/workspaces/${WORKSPACE_ID}/items" \
      -d "{\"displayName\": \"${GDB_NAME}\", \"type\": \"${TRY_TYPE}\"}")
    CREATE_HTTP=$(echo "$CREATE_RESPONSE" | tail -1)
    CREATE_BODY=$(echo "$CREATE_RESPONSE" | head -n -1)

    # 201 Created: item ID in response body
    if [[ "$CREATE_HTTP" == "201" ]]; then
      GDB_ID=$(echo "$CREATE_BODY" | python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
    fi

    # 202 Accepted: async creation — poll for completion
    if [[ "$CREATE_HTTP" == "202" ]]; then
      echo "  ${TRY_TYPE} creation accepted (async) — polling for completion..."
      POLL_URL=$(grep -i "^location:" /tmp/fabric-create-headers | tr -d '\r' | sed 's/^[Ll]ocation: *//')
      RETRY_AFTER=$(grep -i "^retry-after:" /tmp/fabric-create-headers | tr -d '\r' | sed 's/^[Rr]etry-[Aa]fter: *//')
      RETRY_AFTER="${RETRY_AFTER:-5}"

      for ATTEMPT in $(seq 1 12); do
        sleep "${RETRY_AFTER}"
        if [[ -n "$POLL_URL" ]]; then
          POLL_RESP=$(curl -s -H "$AUTH" "$POLL_URL")
          POLL_STATUS=$(echo "$POLL_RESP" | python3 -c "import json,sys; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo "")
          echo "  Poll ${ATTEMPT}/12: status=${POLL_STATUS}"
          if [[ "$POLL_STATUS" == "Succeeded" ]]; then
            # Re-fetch the item to get the ID
            GDB_ID=$(find_item "$TRY_TYPE")
            break
          elif [[ "$POLL_STATUS" == "Failed" ]]; then
            echo "  ${TRY_TYPE} creation failed"
            break
          fi
        else
          # No Location header — try finding the item directly
          GDB_ID=$(find_item "$TRY_TYPE")
          if [[ -n "$GDB_ID" ]]; then
            break
          fi
        fi
      done
    fi

    if [[ -n "$GDB_ID" ]]; then
      ITEM_TYPE="${TRY_TYPE}"
      echo "Created ${TRY_TYPE}: ${GDB_ID}"
      break
    fi
    echo "  ${TRY_TYPE} not available (HTTP ${CREATE_HTTP})"
  done
fi

if [[ -z "$GDB_ID" ]]; then
  echo "::warning::Could not create graph item in Fabric"
  echo "FABRIC_ENDPOINT="
  exit 0
fi

# -- Find or create Lakehouse (required for OneLake data uploads) -----------
LH_NAME="sd2lakehouse"
echo "Looking for Lakehouse '${LH_NAME}'..."
LH_ID=$(curl -s -H "$AUTH" "${FABRIC_API}/workspaces/${WORKSPACE_ID}/items?type=Lakehouse" \
  | LH_NAME="$LH_NAME" python3 -c "
import json, sys, os
name = os.environ['LH_NAME']
data = json.load(sys.stdin)
for item in data.get('value', []):
    if item['displayName'] == name:
        print(item['id'])
        sys.exit(0)
print('')
" 2>/dev/null || echo "")

if [[ -z "$LH_ID" ]]; then
  echo "Creating Lakehouse '${LH_NAME}'..."
  LH_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST -H "$AUTH" -H "Content-Type: application/json" \
    "${FABRIC_API}/workspaces/${WORKSPACE_ID}/items" \
    -d "{\"displayName\": \"${LH_NAME}\", \"type\": \"Lakehouse\"}")
  LH_HTTP=$(echo "$LH_RESPONSE" | tail -1)
  LH_BODY=$(echo "$LH_RESPONSE" | head -n -1)
  LH_ID=$(echo "$LH_BODY" | python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
  if [[ -n "$LH_ID" ]]; then
    echo "Created Lakehouse: ${LH_ID}"
  else
    echo "::warning::Could not create Lakehouse (HTTP ${LH_HTTP})"
  fi
else
  echo "Found existing Lakehouse: ${LH_ID}"
fi

# -- Output the endpoint ---------------------------------------------------
# GraphModel:    /GraphModels/{id}    (GQL via /executeQuery?preview=true — native Fabric Graph)
# Graph:         /graphDatabases/{id} (GQL via /query and /statements)
# GraphDatabase: /graphDatabases/{id} (GQL via /query and /statements)
# GraphQLApi:    /graphqlApis/{id}    (GraphQL via /graphql)
case "$ITEM_TYPE" in
  GraphModel)
    FABRIC_ENDPOINT="${FABRIC_API}/workspaces/${WORKSPACE_ID}/GraphModels/${GDB_ID}"
    ;;
  Graph|GraphDatabase)
    FABRIC_ENDPOINT="${FABRIC_API}/workspaces/${WORKSPACE_ID}/graphDatabases/${GDB_ID}"
    ;;
  GraphQLApi)
    FABRIC_ENDPOINT="${FABRIC_API}/workspaces/${WORKSPACE_ID}/graphqlApis/${GDB_ID}"
    ;;
esac
echo "FABRIC_ENDPOINT=${FABRIC_ENDPOINT}"
echo "FABRIC_ITEM_TYPE=${ITEM_TYPE}"
echo "FABRIC_WORKSPACE_ID=${WORKSPACE_ID}"
echo "FABRIC_LAKEHOUSE_ID=${LH_ID}"
echo "FABRIC_GDB_ID=${GDB_ID}"
