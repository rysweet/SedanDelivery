# Multi-stage Dockerfile for SedanDelivery
# Stage 1: Build the Rust Redis module
# Stage 2: Package in Valkey image with module loaded

# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1.89-bookworm AS builder

WORKDIR /app

# Install libclang required by redis-module bindgen
RUN apt-get update -q && apt-get install -y -q libclang-dev && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests and lockfile first for layer caching.
# Cargo.lock is committed (not gitignored) — copy it to ensure reproducible,
# locked dependency resolution inside Docker. Fixes #1000.
COPY Cargo.toml Cargo.lock ./
COPY gql-core/Cargo.toml              ./gql-core/
COPY gql-redis-store/Cargo.toml       ./gql-redis-store/
COPY gql-module/Cargo.toml            ./gql-module/
COPY gql-fabric-store/Cargo.toml      ./gql-fabric-store/
COPY gql-cli/Cargo.toml               ./gql-cli/
COPY gql-api/Cargo.toml               ./gql-api/
COPY sedandelivery-bolt/Cargo.toml    ./sedandelivery-bolt/
COPY sedandelivery-fabric-sync/Cargo.toml ./sedandelivery-fabric-sync/
COPY sedandelivery-gremlin/Cargo.toml ./sedandelivery-gremlin/
COPY gremlin-core/Cargo.toml          ./gremlin-core/
COPY sedan-gql-embedded/Cargo.toml    ./sedan-gql-embedded/
COPY sedan-cypher-embedded/Cargo.toml ./sedan-cypher-embedded/
COPY sedan-gremlin-embedded/Cargo.toml ./sedan-gremlin-embedded/
COPY tests/loadtest/Cargo.toml        ./tests/loadtest/
COPY tests/gremlin-tck/Cargo.toml     ./tests/gremlin-tck/

# Create stub source files so cargo can resolve the full workspace dependency
# graph during the dependency pre-caching layer. All workspace members listed
# in Cargo.toml must have at least a stub source or cargo cannot load the
# workspace (which caused exit code 101 in #1000).
RUN mkdir -p gql-core/src gql-redis-store/src gql-fabric-store/src gql-module/src gql-cli/src gql-api/src \
             sedandelivery-bolt/src sedandelivery-bolt/benches sedandelivery-fabric-sync/src \
             sedandelivery-gremlin/src gremlin-core/src sedan-gql-embedded/src \
             sedan-cypher-embedded/src sedan-gremlin-embedded/src \
             tests/loadtest/src tests/gremlin-tck/src && \
    echo "pub fn stub() {}" > gql-core/src/lib.rs && \
    echo "pub fn stub() {}" > gql-redis-store/src/lib.rs && \
    echo "pub fn stub() {}" > gql-fabric-store/src/lib.rs && \
    echo "pub fn stub() {}" > gql-module/src/lib.rs && \
    echo "fn main() {}" > gql-cli/src/main.rs && \
    echo "fn main() {}" > gql-api/src/main.rs && \
    echo "pub fn stub() {}" > sedandelivery-bolt/src/lib.rs && \
    echo "fn main() {}" > sedandelivery-bolt/src/main.rs && \
    echo "fn main() {}" > sedandelivery-bolt/benches/bolt_benchmarks.rs && \
    echo "fn main() {}" > sedandelivery-fabric-sync/src/main.rs && \
    echo "pub fn stub() {}" > sedandelivery-gremlin/src/lib.rs && \
    echo "fn main() {}" > sedandelivery-gremlin/src/main.rs && \
    echo "pub fn stub() {}" > gremlin-core/src/lib.rs && \
    echo "pub fn stub() {}" > sedan-gql-embedded/src/lib.rs && \
    echo "pub fn stub() {}" > sedan-cypher-embedded/src/lib.rs && \
    echo "pub fn stub() {}" > sedan-gremlin-embedded/src/lib.rs && \
    echo "fn main() {}" > tests/loadtest/src/main.rs && \
    echo "pub fn stub() {}" > tests/gremlin-tck/src/lib.rs && \
    cargo build --release -p gql-module || true

# Copy actual source code
COPY gql-core/ ./gql-core/
COPY gql-redis-store/ ./gql-redis-store/
COPY gql-fabric-store/ ./gql-fabric-store/
COPY gql-module/ ./gql-module/
COPY gql-cli/ ./gql-cli/
COPY gql-api/ ./gql-api/

# Build the Redis module .so
# --locked ensures the exact dependency versions from Cargo.lock are used,
# making the build reproducible and preventing surprise failures from
# upstream crate changes (root cause of #1000).
RUN touch gql-core/src/lib.rs gql-redis-store/src/lib.rs gql-fabric-store/src/lib.rs gql-module/src/lib.rs && \
    cargo build --release --locked -p gql-module && \
    ls -lh target/release/libgql_module.so

# ── Runtime ───────────────────────────────────────────────────────────────────
FROM valkey/valkey:8.1-bookworm AS runtime

# Copy the compiled module
COPY --from=builder /app/target/release/libgql_module.so /usr/local/lib/libgql_module.so

# Write static Valkey config (password injected at runtime via VALKEY_PASSWORD)
RUN mkdir -p /etc/valkey /data && \
    echo "loadmodule /usr/local/lib/libgql_module.so" > /etc/valkey/valkey-gql.conf && \
    echo "bind 0.0.0.0" >> /etc/valkey/valkey-gql.conf && \
    echo "protected-mode yes" >> /etc/valkey/valkey-gql.conf && \
    echo "loglevel notice" >> /etc/valkey/valkey-gql.conf && \
    echo "dir /data" >> /etc/valkey/valkey-gql.conf && \
    echo "save 60 1" >> /etc/valkey/valkey-gql.conf && \
    echo "appendonly yes" >> /etc/valkey/valkey-gql.conf && \
    echo "appendfsync everysec" >> /etc/valkey/valkey-gql.conf

# Entrypoint injects requirepass at startup, verifies /data is mounted
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 6379

HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD REDISCLI_AUTH="${VALKEY_PASSWORD:-}" valkey-cli ping | grep -q PONG || valkey-cli ping | grep -q PONG

# Run as non-root for security
RUN chown -R valkey:valkey /etc/valkey /data /entrypoint.sh
USER valkey

ENTRYPOINT ["/entrypoint.sh"]
