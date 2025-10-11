# Stage 1: Base - Install cargo-chef and sccache
FROM rust:1.90-bookworm AS base
RUN cargo install cargo-chef
RUN cargo install sccache
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_DIR=/sccache

# Stage 2: Planner - Generate dependency recipe
FROM base AS planner
WORKDIR /app
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder - Build dependencies and application
FROM base AS builder
WORKDIR /app

# Build dependencies (cached layer)
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json

# Build application
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo build --release --bin algo-trade

# Stage 4: Runtime - Minimal Debian image with ttyd
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies and ttyd
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && curl -L https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.x86_64 -o /usr/local/bin/ttyd \
    && chmod +x /usr/local/bin/ttyd \
    && apt-get remove -y curl \
    && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user (UID 10001 to avoid system UID overlap)
RUN useradd -u 10001 -m -s /bin/bash algotrader

# Copy binary from builder
COPY --from=builder --chown=algotrader:algotrader \
    /app/target/release/algo-trade /usr/local/bin/algo-trade

# Copy SQLite migrations
COPY --chown=algotrader:algotrader \
    crates/bot-orchestrator/migrations /app/migrations

# Create data directory for SQLite (bots.db)
RUN mkdir -p /data && chown algotrader:algotrader /data

# Copy entrypoint script
COPY --chown=algotrader:algotrader docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Switch to non-root user
USER algotrader
WORKDIR /home/algotrader

# Expose ports (Web API, ttyd)
EXPOSE 8080 7681

ENTRYPOINT ["/entrypoint.sh"]
