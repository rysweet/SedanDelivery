#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
IMAGE_TAG="${1:-sedan-bolt-ci:latest}"
BINARY_PATH="$PROJECT_DIR/target/release/sedandelivery-bolt"
DOCKERFILE_PATH="$PROJECT_DIR/Dockerfile.bolt.prebuilt"
TMP_CONTEXT="$(mktemp -d)"

cleanup() {
    rm -rf "$TMP_CONTEXT"
}
trap cleanup EXIT

if [ ! -x "$BINARY_PATH" ]; then
    echo "ERROR: $BINARY_PATH is missing. Build it first with:" >&2
    echo "  cargo build --release -p sedandelivery-bolt" >&2
    exit 1
fi

mkdir -p "$TMP_CONTEXT/target/release"
cp "$BINARY_PATH" "$TMP_CONTEXT/target/release/sedandelivery-bolt"
cp "$DOCKERFILE_PATH" "$TMP_CONTEXT/Dockerfile.bolt.prebuilt"

docker build -f "$TMP_CONTEXT/Dockerfile.bolt.prebuilt" -t "$IMAGE_TAG" "$TMP_CONTEXT"
