#!/usr/bin/env bash
# Add a deployed URL to the SedanDelivery Entra ID app registration as a SPA redirect URI.
#
# Usage:
#   ./scripts/setup-entra-redirect.sh https://my-app.azurewebsites.net
#
# Prerequisites:
#   - Azure CLI (az) installed and logged in
#   - Permissions to modify the app registration

set -euo pipefail

APP_ID="${ENTRA_CLIENT_ID:-97926b31-e1a3-4a76-b09d-eed5aeeabc9f}"
REDIRECT_URI="$1"

if [ -z "${REDIRECT_URI:-}" ]; then
  echo "Usage: $0 <redirect-uri>"
  exit 1
fi

OBJ_ID=$(az ad app show --id "$APP_ID" --query id -o tsv)
EXISTING=$(az ad app show --id "$APP_ID" --query "spa.redirectUris" -o json)

# Add new URI if not already present
RESULT=$(echo "$EXISTING" | REDIRECT_URI="$REDIRECT_URI" python3 -c "
import json, sys, os
uris = json.load(sys.stdin)
new_uri = os.environ['REDIRECT_URI']
if new_uri not in uris:
    uris.append(new_uri)
    print(json.dumps(uris))
else:
    print('ALREADY_EXISTS')
")

if [ "$RESULT" = "ALREADY_EXISTS" ]; then
  echo "Redirect URI already registered: $REDIRECT_URI"
else
  az rest --method PATCH \
    --uri "https://graph.microsoft.com/v1.0/applications/$OBJ_ID" \
    --headers "Content-Type=application/json" \
    --body "{\"spa\": {\"redirectUris\": $RESULT}}"
  echo "Added redirect URI: $REDIRECT_URI"
fi
