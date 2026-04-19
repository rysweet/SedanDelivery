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
COPY cypher-core/Cargo.toml              ./cypher-core/
COPY sd2-redis-store/Cargo.toml       ./sd2-redis-store/
COPY sd2-redis-module/Cargo.toml            ./sd2-redis-module/
COPY gql-fabric-store/Cargo.toml      ./gql-fabric-store/
COPY gql-cli/Cargo.toml               ./gql-cli/
COPY sd2-api/Cargo.toml               ./sd2-api/
COPY sedandelivery-bolt/Cargo.toml    ./sedandelivery-bolt/
COPY sedandelivery-fabric-sync/Cargo.toml ./sedandelivery-fabric-sync/
COPY sedandelivery-gremlin/Cargo.toml ./sedandelivery-gremlin/
COPY sedandelivery-gremlin-server/Cargo.toml ./sedandelivery-gremlin-server/
COPY gremlin-core/Cargo.toml          ./gremlin-core/
COPY gql-engine/Cargo.toml            ./gql-engine/
COPY sedan-gql-embedded/Cargo.toml    ./sedan-gql-embedded/
COPY sedan-cypher-embedded/Cargo.toml ./sedan-cypher-embedded/
COPY sedan-gremlin-embedded/Cargo.toml ./sedan-gremlin-embedded/
COPY tests/loadtest/Cargo.toml        ./tests/loadtest/
COPY tests/gremlin-tck/Cargo.toml     ./tests/gremlin-tck/
COPY examples/embedded_app/Cargo.toml ./examples/embedded_app/

# Copy the workspace stub generator and all member Cargo.toml files have already
# been COPYed above. Generate stub source files for every workspace member +
# every [[bench]] target so cargo can resolve the full workspace during the
# dependency pre-caching layer. This replaces a manual mkdir/echo wall that
# had to be edited for every new member or bench (and produced merge conflicts
# on every parallel feature branch — see #1272/#1273/#1274 history).
COPY scripts/generate-cargo-stubs.sh ./scripts/
RUN chmod +x scripts/generate-cargo-stubs.sh && \
    scripts/generate-cargo-stubs.sh && \
    cargo build --release -p sd2-redis-module || true

# Copy actual source code
COPY cypher-core/ ./cypher-core/
COPY sd2-redis-store/ ./sd2-redis-store/
COPY gql-fabric-store/ ./gql-fabric-store/
COPY sd2-redis-module/ ./sd2-redis-module/
COPY gql-cli/ ./gql-cli/
COPY sd2-api/ ./sd2-api/
COPY gql-engine/ ./gql-engine/

# Build the Redis module .so
# --locked ensures the exact dependency versions from Cargo.lock are used,
# making the build reproducible and preventing surprise failures from
# upstream crate changes (root cause of #1000).
RUN touch cypher-core/src/lib.rs sd2-redis-store/src/lib.rs gql-fabric-store/src/lib.rs sd2-redis-module/src/lib.rs && \
    cargo build --release --locked -p sd2-redis-module && \
    ls -lh target/release/libsd2_redis_module.so

# ── Runtime ───────────────────────────────────────────────────────────────────
FROM valkey/valkey:8.1-bookworm AS runtime

# Copy the compiled module
COPY --from=builder /app/target/release/libsd2_redis_module.so /usr/local/lib/libsd2_redis_module.so

# Write static Valkey config (password injected at runtime via VALKEY_PASSWORD)
RUN mkdir -p /etc/valkey /data && \
    echo "loadmodule /usr/local/lib/libsd2_redis_module.so" > /etc/valkey/valkey-gql.conf && \
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
