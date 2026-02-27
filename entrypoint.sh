#!/bin/sh
set -e

# Verify /data is actually a mounted volume (not the image layer)
if ! mountpoint -q /data 2>/dev/null; then
    echo "WARNING: /data is not a mounted volume — data will NOT persist across restarts" >&2
fi

# Inject requirepass from environment variable if set
if [ -n "${VALKEY_PASSWORD}" ]; then
    # Validate password does not contain whitespace (prevents config injection)
    case "$VALKEY_PASSWORD" in
      *[[:space:]]*) echo "ERROR: VALKEY_PASSWORD contains whitespace" >&2; exit 1 ;;
    esac
    printf 'requirepass %s\n' "$VALKEY_PASSWORD" >> /etc/valkey/valkey-gql.conf
    echo "INFO: Valkey authentication enabled" >&2
else
    echo "WARNING: VALKEY_PASSWORD not set — running without authentication" >&2
fi

exec valkey-server /etc/valkey/valkey-gql.conf
