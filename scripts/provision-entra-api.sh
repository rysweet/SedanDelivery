#!/usr/bin/env bash
# Configure the existing Entra ID SPA app registration to also serve as the
# sd2-api JWT audience. This allows the frontend's access token to authenticate
# against the backend API — single app registration, no extra secrets.
#
# Idempotent: safe to run multiple times. Only adds the API scope if missing.
#
# Usage:
#   ./scripts/provision-entra-api.sh [client-id]
#
# Arguments:
#   client-id  — The SPA app registration's Application (client) ID.
#                Defaults to ENTRA_CLIENT_ID env var or the well-known
#                SedanDelivery SPA app registration.
#
# Outputs (to stdout, for capture by CI):
#   GQL_API_ENTRA_TENANT_ID=<tenant-id>
#   GQL_API_ENTRA_CLIENT_ID=<client-id>
#
# Prerequisites:
#   - Azure CLI (az) installed and logged in
#   - Permissions to update the app registration

set -euo pipefail

CLIENT_ID="${1:-${ENTRA_CLIENT_ID:-97926b31-e1a3-4a76-b09d-eed5aeeabc9f}}"
TENANT_ID=$(az account show --query tenantId -o tsv)

echo "Using app registration: ${CLIENT_ID}" >&2

# Ensure Application ID URI is set (required for api:// audience format)
APP_ID_URI="api://${CLIENT_ID}"
CURRENT_URI=$(az ad app show --id "$CLIENT_ID" --query "identifierUris[0]" -o tsv 2>/dev/null || echo "")
if [[ "$CURRENT_URI" != "$APP_ID_URI" ]]; then
  echo "Setting Application ID URI: ${APP_ID_URI}" >&2
  az ad app update --id "$CLIENT_ID" --identifier-uris "$APP_ID_URI" 2>/dev/null || true
fi

# Check if an API scope already exists
EXISTING_SCOPES=$(az ad app show --id "$CLIENT_ID" \
  --query "api.oauth2PermissionScopes[?value=='access_as_user'].id" -o tsv 2>/dev/null || echo "")

if [[ -z "$EXISTING_SCOPES" ]]; then
  echo "Adding 'access_as_user' API scope..." >&2
  SCOPE_ID=$(python3 -c "import uuid; print(uuid.uuid4())")
  az ad app update --id "$CLIENT_ID" --set "api={\"oauth2PermissionScopes\":[{\"adminConsentDescription\":\"Access SedanDelivery API\",\"adminConsentDisplayName\":\"Access API\",\"id\":\"${SCOPE_ID}\",\"isEnabled\":true,\"type\":\"User\",\"userConsentDescription\":\"Access SedanDelivery graph API on your behalf\",\"userConsentDisplayName\":\"Access Graph API\",\"value\":\"access_as_user\"}]}" 2>/dev/null || true
  echo "Added API scope: access_as_user (${SCOPE_ID})" >&2
else
  echo "API scope 'access_as_user' already exists" >&2
fi

# Output for CI consumption
echo "GQL_API_ENTRA_TENANT_ID=${TENANT_ID}"
echo "GQL_API_ENTRA_CLIENT_ID=${CLIENT_ID}"
