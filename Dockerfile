# Multi-stage Dockerfile for SedanDelivery
# Stage 1: Build the Rust Redis module
# Stage 2: Package in Valkey image with module loaded

# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1.89-bookworm AS builder

WORKDIR /app

# Install libclang required by redis-module bindgen
RUN apt-get update -q && apt-get install -y -q libclang-dev && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests first for layer caching
# Note: Cargo.lock is in .gitignore so we only copy Cargo.toml
COPY Cargo.toml ./
COPY gql-core/Cargo.toml ./gql-core/
COPY gql-redis-store/Cargo.toml ./gql-redis-store/
COPY gql-module/Cargo.toml ./gql-module/
COPY gql-cli/Cargo.toml ./gql-cli/

# Create stub source files so cargo can resolve the dependency graph
RUN mkdir -p gql-core/src gql-redis-store/src gql-module/src gql-cli/src && \
    echo "pub fn stub() {}" > gql-core/src/lib.rs && \
    echo "pub fn stub() {}" > gql-redis-store/src/lib.rs && \
    echo "pub fn stub() {}" > gql-module/src/lib.rs && \
    echo "fn main() {}" > gql-cli/src/main.rs && \
    cargo build --release -p gql-module 2>/dev/null || true

# Copy actual source code
COPY gql-core/ ./gql-core/
COPY gql-redis-store/ ./gql-redis-store/
COPY gql-module/ ./gql-module/
COPY gql-cli/ ./gql-cli/

# Build the Redis module .so
RUN touch gql-core/src/lib.rs gql-redis-store/src/lib.rs gql-module/src/lib.rs && \
    cargo build --release -p gql-module && \
    ls -lh target/release/libgql_module.so

# ── Runtime ───────────────────────────────────────────────────────────────────
FROM valkey/valkey:8.1-bookworm AS runtime

# Copy the compiled module
COPY --from=builder /app/target/release/libgql_module.so /usr/local/lib/libgql_module.so

# Write a minimal valkey.conf that loads the GQL module and persists data to /data
RUN mkdir -p /etc/valkey /data && \
    echo "loadmodule /usr/local/lib/libgql_module.so" > /etc/valkey/valkey-gql.conf && \
    echo "bind 0.0.0.0" >> /etc/valkey/valkey-gql.conf && \
    echo "protected-mode no" >> /etc/valkey/valkey-gql.conf && \
    echo "loglevel notice" >> /etc/valkey/valkey-gql.conf && \
    echo "dir /data" >> /etc/valkey/valkey-gql.conf && \
    echo "save 60 1" >> /etc/valkey/valkey-gql.conf && \
    echo "save 300 1" >> /etc/valkey/valkey-gql.conf && \
    echo "appendonly yes" >> /etc/valkey/valkey-gql.conf && \
    echo "appendfsync everysec" >> /etc/valkey/valkey-gql.conf

EXPOSE 6379

HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD valkey-cli ping | grep -q PONG

CMD ["valkey-server", "/etc/valkey/valkey-gql.conf"]
