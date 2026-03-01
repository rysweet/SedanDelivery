#!/bin/bash
# scripts/provision-fabric.sh
# Provisions a Microsoft Fabric Graph Database workspace for SedanDelivery.
# Usage: ./provision-fabric.sh <env-name> <tenant-id> <client-id> <client-secret>
set -euo pipefail

ENV_NAME="${1:?Usage: provision-fabric.sh <env-name> <tenant-id> <client-id> <client-secret>}"
TENANT_ID="${2:?}"
CLIENT_ID="${3:?}"
CLIENT_SECRET="${4:?}"

FABRIC_API="https://api.fabric.microsoft.com/v1"
WORKSPACE_NAME="SedanDelivery-${ENV_NAME}"

# -- Get access token ------------------------------------------------------
TOKEN=$(curl -s -X POST "https://login.microsoftonline.com/${TENANT_ID}/oauth2/v2.0/token" \
  -d "grant_type=client_credentials&client_id=${CLIENT_ID}&client_secret=${CLIENT_SECRET}&scope=https%3A%2F%2Fapi.fabric.microsoft.com%2F.default" \
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
  | python3 -c "
import json, sys
data = json.load(sys.stdin)
for ws in data.get('value', []):
    if ws['displayName'] == '${WORKSPACE_NAME}':
        print(ws['id'])
        sys.exit(0)
print('')
")
if [[ -z "$WORKSPACE_ID" ]]; then
  # Fallback: admin API can see workspaces the SP created but isn't a member of
  WORKSPACE_ID=$(curl -s -H "$AUTH" "${FABRIC_API}/admin/workspaces" \
    | python3 -c "
import json, sys
data = json.load(sys.stdin)
for ws in data.get('workspaces', data.get('value', [])):
    if ws.get('displayName', ws.get('name', '')) == '${WORKSPACE_NAME}':
        print(ws['id'])
        sys.exit(0)
print('')
" 2>/dev/null || echo "")
fi

if [[ -z "$WORKSPACE_ID" ]]; then
  echo "Creating workspace '${WORKSPACE_NAME}'..."
  WORKSPACE_ID=$(curl -s -X POST -H "$AUTH" -H "Content-Type: application/json" \
    "${FABRIC_API}/workspaces" \
    -d "{\"displayName\": \"${WORKSPACE_NAME}\"}" \
    | python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))")

  if [[ -z "$WORKSPACE_ID" ]]; then
    # Name already exists but we couldn't find it — try admin API
    echo "Create returned empty ID — looking up via admin API..."
    WORKSPACE_ID=$(curl -s -H "$AUTH" "${FABRIC_API}/admin/workspaces" \
      | python3 -c "
import json, sys
data = json.load(sys.stdin)
for ws in data.get('workspaces', data.get('value', [])):
    if ws.get('displayName', ws.get('name', '')) == '${WORKSPACE_NAME}':
        print(ws['id'])
        sys.exit(0)
print('')
" 2>/dev/null || echo "")
    if [[ -z "$WORKSPACE_ID" ]]; then
      echo "::error::Failed to create or find Fabric workspace" >&2
      exit 1
    fi
  fi
  echo "Created workspace: ${WORKSPACE_ID}"
else
  echo "Found existing workspace: ${WORKSPACE_ID}"
fi

# -- Ensure SP is a workspace member (needed to list/create items) ----------------
echo "Ensuring SP has workspace member access..."
# Get the SP's own object ID from the token
SP_OBJECT_ID=$(python3 -c "
import json, base64, sys
token = '${TOKEN}'
payload = token.split('.')[1]
payload += '=' * (4 - len(payload) % 4)  # pad base64
data = json.loads(base64.urlsafe_b64decode(payload))
print(data.get('oid', ''))
")
if [[ -n "$SP_OBJECT_ID" ]]; then
  curl -s -X POST -H "$AUTH" -H "Content-Type: application/json"     "${FABRIC_API}/workspaces/${WORKSPACE_ID}/roleAssignments"     -d "{\"principal\": {\"id\": \"${SP_OBJECT_ID}\", \"type\": \"ServicePrincipal\"}, \"role\": \"Admin\"}"     >/dev/null 2>&1 || true
  echo "SP ${SP_OBJECT_ID} granted workspace access (or already had it)"
fi

# -- Find or create Graph Database -----------------------------------------
GDB_NAME="sedangraph"
echo "Looking for Graph Database '${GDB_NAME}'..."
GDB_ID=$(curl -s -H "$AUTH" "${FABRIC_API}/workspaces/${WORKSPACE_ID}/items?type=GraphQLApi" \
  | python3 -c "
import json, sys
data = json.load(sys.stdin)
for item in data.get('value', []):
    if item['displayName'] == '${GDB_NAME}':
        print(item['id'])
        sys.exit(0)
print('')
" 2>/dev/null || echo "")

if [[ -z "$GDB_ID" ]]; then
  echo "Creating Graph Database '${GDB_NAME}'..."
  GDB_ID=$(curl -s -X POST -H "$AUTH" -H "Content-Type: application/json" \
    "${FABRIC_API}/workspaces/${WORKSPACE_ID}/items" \
    -d "{\"displayName\": \"${GDB_NAME}\", \"type\": \"GraphQLApi\"}" \
    | python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")

  if [[ -z "$GDB_ID" ]]; then
    echo "::warning::Could not create Graph Database — Fabric capacity may not be provisioned"
    echo "FABRIC_ENDPOINT="
    exit 0
  fi
  echo "Created Graph Database: ${GDB_ID}"
else
  echo "Found existing Graph Database: ${GDB_ID}"
fi

# -- Output the endpoint ---------------------------------------------------
FABRIC_ENDPOINT="${FABRIC_API}/workspaces/${WORKSPACE_ID}/graphDatabases/${GDB_ID}"
echo "FABRIC_ENDPOINT=${FABRIC_ENDPOINT}"
echo "FABRIC_WORKSPACE_ID=${WORKSPACE_ID}"
echo "FABRIC_GDB_ID=${GDB_ID}"
