# Context Report: Docker Containerization for Hyperliquid Trading System

**Date**: 2025-10-07
**Author**: Context Gatherer Agent
**Status**: Complete - Ready for TaskMaster Handoff

---

## 1. Request Analysis

### Explicit Requirements
- **Docker Container**: Create self-contained Docker image/container for entire trading system
- **Daemon Auto-Start**: Automatically run trading daemon when container boots
- **Database Setup**: TimescaleDB + PostgreSQL initialization on container startup
- **TUI Remote Access**: Expose or forward TUI for remote connection
- **Self-Contained System**: Single deployable unit managed by Docker

### Implicit Requirements
- **Multi-Container Orchestration**: App container + database container coordination
- **Data Persistence**: Volumes for SQLite (bots.db), PostgreSQL, TimescaleDB data
- **Environment Variables**: Configuration injection (API keys, database URLs, secrets)
- **Network Exposure**: Port mapping for API (8080), TUI access method
- **Build Optimization**: Fast builds (cargo-chef), small image sizes (multi-stage)
- **Security**: Non-root user, secret management, minimal attack surface
- **Graceful Shutdown**: SIGTERM handling for clean bot shutdown

### Success Criteria
1. Single command deployment: `docker compose up -d`
2. TimescaleDB auto-initializes with schema from `/home/a/Work/algo-trade/scripts/setup_timescale.sql`
3. Trading daemon starts automatically in daemon mode (Run command)
4. TUI accessible remotely via chosen method (ttyd/SSH/docker exec)
5. Bot configurations persist across container restarts (SQLite bots.db volume)
6. TimescaleDB data persists across restarts (PostgreSQL volume)
7. Web API accessible on port 8080

---

## 2. Codebase Reconnaissance

### Current Architecture Analysis

#### Entry Points (`/home/a/Work/algo-trade/crates/cli/src/main.rs`)
**Lines 6-88**: CLI subcommands
- `Run`: Daemon mode with web API (primary container mode)
- `Server`: Web API only
- `LiveBotTui`: Interactive TUI for bot management
- `Backtest`, `TuiBacktest`, `ScheduledBacktest`: Analysis modes
- `FetchData`, `TokenSelection`: Utility commands

**Lines 154-235**: Daemon Implementation (`run_trading_system`)
- Loads config from `algo_trade_core::ConfigLoader`
- Initializes SQLite database for bot persistence: `BOT_DATABASE_URL` env var (default: `sqlite://bots.db`)
- Creates `BotRegistry` with database persistence
- Restores bots from database on startup (lines 175-186)
- Starts Axum web API server on `config.server.host:config.server.port` (default 0.0.0.0:8080)
- **Graceful shutdown**: Lines 202-232 handle SIGTERM/SIGINT signals, shutdown all bots, abort server

**Lines 172-186**: LiveBotTui Entry Point
- No database initialization (creates in-memory registry)
- Runs Ratatui TUI application
- Allows bot creation, monitoring, parameter configuration

#### Configuration (`/home/a/Work/algo-trade/config/Config.example.toml`)
**Lines 1-12**: Core config
```toml
[server]
host = "0.0.0.0"
port = 8080

[database]
url = "postgresql://localhost/algo_trade"
max_connections = 10

[hyperliquid]
api_url = "https://api.hyperliquid.xyz"
ws_url = "wss://api.hyperliquid.xyz/ws"
```

#### Database Schema (`/home/a/Work/algo-trade/scripts/setup_timescale.sql`)
**Lines 1-95**: TimescaleDB Schema
- `ohlcv` table (hypertable, lines 2-12): DECIMAL(20,8) precision for OHLCV data
- `trades` table (hypertable, lines 31-42): Trade execution tracking
- `fills` table (lines 46-60): Order fill history
- `backtest_results` table (hypertable, lines 63-94): Token selection metrics

**Required for Docker**: This file must be copied to `/docker-entrypoint-initdb.d/` in TimescaleDB container.

#### Bot Persistence (`/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_database.rs`)
**Lines 10-37**: SQLite Database for Bot Configs
- Connection pool: `SqlitePoolOptions::new().max_connections(5)`
- Migrations: `sqlx::migrate!("./migrations")` (lines 32-34)
- Database URL: Env var `BOT_DATABASE_URL` or default `sqlite://bots.db`

**Lines 67-86**: Bot Configuration Persistence
- Stores entire `BotConfig` as JSON blob in `bot_configs` table
- Enables auto-restore on daemon restart (lines 179-193 in `registry.rs`)

**Migration Schema**: `/home/a/Work/algo-trade/crates/bot-orchestrator/migrations/20251007000001_create_bots.sql`
```sql
CREATE TABLE bot_configs (
    bot_id TEXT PRIMARY KEY,
    config_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE bot_runtime_state (
    bot_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    started_at INTEGER,
    last_heartbeat INTEGER NOT NULL,
    FOREIGN KEY (bot_id) REFERENCES bot_configs(bot_id)
);
```

#### Workspace Structure (`/home/a/Work/algo-trade/Cargo.toml`)
**Lines 1-14**: Workspace members
- 13 crates total
- Primary binary: `crates/cli`
- Dependencies: tokio, serde, anyhow, thiserror, tracing

### Files Requiring Docker Integration
1. **Project Root**: `/home/a/Work/algo-trade/` (workspace root)
2. **TimescaleDB Init**: `/home/a/Work/algo-trade/scripts/setup_timescale.sql`
3. **SQLite Migrations**: `/home/a/Work/algo-trade/crates/bot-orchestrator/migrations/*.sql`
4. **Config Template**: `/home/a/Work/algo-trade/config/Config.example.toml`

### Missing Docker Files (To Create)
- `Dockerfile` (multi-stage Rust build)
- `docker-compose.yml` (app + TimescaleDB orchestration)
- `.dockerignore` (optimize build context)
- `docker/entrypoint.sh` (container initialization script)
- `docker/timescale-init.sql` (copy of setup_timescale.sql)

---

## 3. External Research

### 3.1 Rust Docker Best Practices (2025)

#### Multi-Stage Build with cargo-chef
**Source**: https://depot.dev/blog/rust-dockerfile-best-practices

**3-Stage Pattern** (5x faster builds):
```dockerfile
FROM rust:1.75 AS base
RUN cargo install sccache --version ^0.7
RUN cargo install cargo-chef --version ^0.1
ENV RUSTC_WRAPPER=sccache SCCACHE_DIR=/sccache

FROM base AS planner
WORKDIR /app
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef prepare --recipe-path recipe.json

FROM base AS builder
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo build --release --bin algo-trade-cli

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/algo-trade-cli /usr/local/bin/algo-trade
ENTRYPOINT ["/usr/local/bin/algo-trade"]
```

**Performance Results**:
- Unoptimized: 1m 4s
- With cargo-chef: 15s (75% reduction)
- With cargo-chef + sccache: 7s (89% reduction)

**Key Optimizations**:
- `cargo-chef`: Caches dependency builds (recipe.json remains stable unless Cargo.toml changes)
- `sccache`: Caches individual compilation artifacts (granular caching)
- BuildKit cache mounts: Persist registry/git/sccache across builds
- Same Rust version across all stages (critical for cache hits)

#### Base Image Selection: Alpine vs Debian
**Source**: https://www.docker.com/blog/simplify-your-deployments-using-the-rust-official-image/

**Debian bookworm-slim** (RECOMMENDED):
- Size: ~80MB base + binary (~15MB Rust static binary = ~95MB total)
- Compatibility: Full glibc support, compatible with most dependencies
- Security: Regular updates, well-maintained
- Rust builds: No musl compilation issues

**Alpine** (NOT RECOMMENDED for this project):
- Size: ~40MB base + binary = ~55MB total
- Compatibility: musl libc instead of glibc (potential dependency issues)
- TimescaleDB/PostgreSQL clients: May have linking issues with musl
- Trade-off: 40MB savings NOT worth compatibility risk

**Decision**: Use `rust:1.75-bookworm` for builder, `debian:bookworm-slim` for runtime.

#### Security: Non-Root User
**Source**: https://betterstack.com/community/guides/scaling-docker/docker-security-best-practices/

**Best Practices 2025**:
- Principle of Least Privilege: Never run containers as root
- UID/GID: Use UID > 10000 to avoid conflicts with system users
- Ownership: Change binary ownership to non-root user
- Security Options: `--security-opt=no-new-privileges` prevents privilege escalation

**Implementation**:
```dockerfile
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Create non-root user (UID 10001 to avoid system UID overlap)
RUN useradd -u 10001 -m -s /bin/bash algotrader

# Copy binary and set ownership
COPY --from=builder --chown=algotrader:algotrader /app/target/release/algo-trade-cli /usr/local/bin/algo-trade

# Switch to non-root user
USER algotrader
WORKDIR /home/algotrader

ENTRYPOINT ["/usr/local/bin/algo-trade"]
```

### 3.2 TimescaleDB Docker Integration

#### Official Image: timescale/timescaledb
**Source**: https://github.com/timescale/timescaledb-docker

**Recommended Version**: `timescale/timescaledb:latest-pg17`
- Based on official PostgreSQL 17 image
- TimescaleDB extension pre-installed
- Auto-runs `timescaledb-tune` on initialization

**Environment Variables**:
- `POSTGRES_DB`: Database name (default: `postgres`)
- `POSTGRES_USER`: Database user (default: `postgres`)
- `POSTGRES_PASSWORD`: **REQUIRED** (security: use Docker secrets)
- `TIMESCALEDB_TELEMETRY`: Set to `off` (disable telemetry)
- `TS_TUNE_MEMORY`: Memory allocation (e.g., `4GB`)
- `TS_TUNE_NUM_CPUS`: CPU allocation (e.g., `4`)
- `NO_TS_TUNE`: Set to `true` to disable auto-tuning

**Initialization Scripts**:
- Directory: `/docker-entrypoint-initdb.d/`
- Behavior: Runs all `.sql` and `.sh` files during first container startup
- **CRITICAL WARNING**: Mounting host directory to `/docker-entrypoint-initdb.d/` REMOVES TimescaleDB setup files
- **SOLUTION**: Copy `setup_timescale.sql` into image at build time (not volume mount)

**Docker Compose Example**:
```yaml
services:
  timescaledb:
    image: timescale/timescaledb:latest-pg17
    environment:
      POSTGRES_DB: algo_trade
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD_FILE: /run/secrets/db_password
      TIMESCALEDB_TELEMETRY: 'off'
      TS_TUNE_MEMORY: 4GB
      TS_TUNE_NUM_CPUS: 4
    volumes:
      - timescale-data:/var/lib/postgresql/data
      - ./scripts/setup_timescale.sql:/docker-entrypoint-initdb.d/01-init.sql:ro
    secrets:
      - db_password
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres -d algo_trade"]
      interval: 5s
      timeout: 5s
      retries: 5
      start_period: 10s
    ports:
      - "5432:5432"

volumes:
  timescale-data:

secrets:
  db_password:
    file: ./secrets/db_password.txt
```

**Healthcheck Pattern**:
- Command: `pg_isready -U postgres -d algo_trade`
- Purpose: Verify PostgreSQL accepting connections before app starts
- Timing: 5s interval, 5s timeout, 5 retries, 10s start period

### 3.3 TUI Remote Access Options

#### Option A: ttyd (Web-based Terminal) - RECOMMENDED
**Source**: https://github.com/tsl0922/ttyd

**Advantages**:
- No SSH setup required
- Web browser access (modern UX)
- Single port exposure (HTTP/HTTPS)
- Full terminal emulation (Xterm.js)
- Easy authentication (basic auth)

**Implementation**:
```dockerfile
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    ttyd \
    && rm -rf /var/lib/apt/lists/*

# Entrypoint script runs daemon + ttyd
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
```

**Entrypoint Script** (`docker/entrypoint.sh`):
```bash
#!/bin/bash
set -e

# Start trading daemon in background
algo-trade run --config /config/Config.toml &
DAEMON_PID=$!

# Start ttyd exposing TUI on port 7681
ttyd -p 7681 -W algo-trade live-bot-tui

# Wait for daemon (in case ttyd exits)
wait $DAEMON_PID
```

**Port Mapping**:
- 8080: Web API
- 7681: ttyd web terminal

**Access Method**: `http://localhost:7681` in web browser

**Security**: Add basic authentication: `ttyd -p 7681 -c admin:password -W algo-trade live-bot-tui`

#### Option B: SSH Access
**Advantages**: Traditional, secure, supports key-based auth

**Disadvantages**:
- Requires SSH daemon in container (increased image size)
- Extra port exposure (22)
- Key management complexity

**Implementation** (NOT RECOMMENDED - adds ~50MB to image):
```dockerfile
RUN apt-get install -y openssh-server
RUN mkdir /var/run/sshd
RUN echo 'algotrader:yourpassword' | chpasswd
RUN sed -i 's/#PermitRootLogin prohibit-password/PermitRootLogin no/' /etc/ssh/sshd_config
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D"]
```

#### Option C: docker exec (Simplest)
**Advantages**: No extra software, built into Docker

**Disadvantages**:
- Requires Docker daemon access
- Not suitable for remote deployment
- Manual connection each time

**Usage**: `docker exec -it algo-trade-app algo-trade live-bot-tui`

**Recommended for**: Local development only

### 3.4 Docker Compose Multi-Container Networking

#### Automatic Service Discovery
**Source**: https://docs.docker.com/compose/how-tos/networking/

**Default Behavior**:
- Compose creates single network automatically
- Containers discoverable by service name (e.g., `timescaledb:5432`)
- No manual network configuration needed

**Example** (app connects to database):
```yaml
services:
  timescaledb:
    image: timescale/timescaledb:latest-pg17
    # Accessible at: postgresql://timescaledb:5432/algo_trade

  app:
    build: .
    environment:
      DATABASE_URL: postgresql://postgres:password@timescaledb:5432/algo_trade
    depends_on:
      timescaledb:
        condition: service_healthy
```

**Key Point**: Use service name `timescaledb` (NOT `localhost`) in connection string.

#### Healthcheck Dependencies
**Pattern**: `depends_on` with `condition: service_healthy`
- App container waits for database healthcheck to pass
- Prevents connection errors on startup
- Requires healthcheck defined on database service

**Implementation**:
```yaml
app:
  depends_on:
    timescaledb:
      condition: service_healthy
```

#### Volume Persistence
**Named Volumes** (RECOMMENDED):
```yaml
volumes:
  timescale-data:        # TimescaleDB PostgreSQL data
  sqlite-data:           # Bot configurations (bots.db)

services:
  timescaledb:
    volumes:
      - timescale-data:/var/lib/postgresql/data

  app:
    volumes:
      - sqlite-data:/data  # App writes bots.db to /data/bots.db
```

**Bind Mounts** (for config):
```yaml
app:
  volumes:
    - ./config/Config.toml:/config/Config.toml:ro  # Read-only config
```

### 3.5 Graceful Shutdown (Docker SIGTERM)

#### Current Implementation Analysis
**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs` (lines 202-232)

**Existing Signal Handling** (EXCELLENT - already Docker-ready):
```rust
let shutdown_signal = async {
    let mut sigterm = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::terminate()
    ).expect("Failed to create SIGTERM handler");

    let mut sigint = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::interrupt()
    ).expect("Failed to create SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, initiating graceful shutdown");
        }
        _ = sigint.recv() => {
            tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
        }
    }
};

shutdown_signal.await;

// Graceful shutdown
tracing::info!("Shutting down all bots...");
if let Err(e) = registry.shutdown_all().await {
    tracing::error!("Error during bot shutdown: {}", e);
}

server_handle.abort();
```

**Docker Shutdown Flow**:
1. `docker stop` sends SIGTERM to PID 1 (main process)
2. Tokio signal handler catches SIGTERM
3. Registry shuts down all bots gracefully (`registry.shutdown_all()`)
4. Server task aborted
5. Container exits with code 0

**Default Timeout**: Docker waits 10 seconds after SIGTERM before sending SIGKILL.

**No Changes Required**: Existing implementation perfect for Docker.

**Optional Optimization** (if shutdown takes >10s):
```yaml
services:
  app:
    stop_grace_period: 30s  # Allow 30s for graceful shutdown
```

---

## 4. Analysis & Synthesis

### Recommended Architecture: Option C (Hybrid - Docker Compose Multi-Container)

#### Design Rationale
**Option A: Single Container with supervisord** ❌
- **Rejected**: Violates Docker best practices (one process per container)
- PostgreSQL + daemon in one container creates lifecycle coupling
- Harder to scale, debug, and maintain

**Option B: Separate Services (TimescaleDB + Daemon + TUI)** ❌
- **Rejected**: TUI as separate service adds complexity
- TUI should be accessed on-demand via ttyd in daemon container
- No benefit to separate TUI container

**Option C: Hybrid (TimescaleDB + App with ttyd)** ✅ RECOMMENDED
- **TimescaleDB Service**: Official `timescale/timescaledb:latest-pg17` image
- **App Service**: Multi-stage Rust build with daemon + ttyd
- **Single Entry Point**: App container runs daemon, exposes ttyd for TUI access
- **Clear Separation**: Database (stateful) vs app (stateless after bot configs persist)

#### Service Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Docker Compose                           │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌────────────────────────┐      ┌──────────────────────┐  │
│  │  timescaledb:latest-pg17│      │  algo-trade-app      │  │
│  │                         │      │                       │  │
│  │  - PostgreSQL 17        │◄─────┤  - Rust daemon       │  │
│  │  - TimescaleDB ext.     │ 5432 │  - Web API (8080)    │  │
│  │  - Auto-tuned           │      │  - ttyd TUI (7681)   │  │
│  │                         │      │                       │  │
│  │  Volume:                │      │  Volume:              │  │
│  │  timescale-data         │      │  sqlite-data         │  │
│  │  /var/lib/postgresql/   │      │  /data/bots.db       │  │
│  └────────────────────────┘      └──────────────────────┘  │
│                                                               │
│  Network: algo-trade-network (automatic)                     │
│  Volumes: timescale-data, sqlite-data (named, persistent)    │
│  Secrets: db_password.txt (file-based secret)                │
└─────────────────────────────────────────────────────────────┘

External Access:
- http://localhost:8080      → Web API (REST endpoints)
- http://localhost:7681      → TUI via ttyd (web terminal)
- postgresql://localhost:5432 → TimescaleDB (development only)
```

#### Dockerfile Structure (Multi-Stage)

**Stage 1: Base** (cargo-chef + sccache installation)
```dockerfile
FROM rust:1.75-bookworm AS base
RUN cargo install cargo-chef --locked
RUN cargo install sccache --locked
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_DIR=/sccache
```

**Stage 2: Planner** (generate dependency recipe)
```dockerfile
FROM base AS planner
WORKDIR /app
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef prepare --recipe-path recipe.json
```

**Stage 3: Builder** (build dependencies + app)
```dockerfile
FROM base AS builder
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo build --release --bin algo-trade-cli
```

**Stage 4: Runtime** (minimal Debian with ttyd)
```dockerfile
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    ttyd \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -u 10001 -m -s /bin/bash algotrader

# Copy binary
COPY --from=builder --chown=algotrader:algotrader \
    /app/target/release/algo-trade-cli /usr/local/bin/algo-trade

# Copy SQLite migrations
COPY --chown=algotrader:algotrader \
    crates/bot-orchestrator/migrations /app/migrations

# Create data directory for SQLite
RUN mkdir -p /data && chown algotrader:algotrader /data

# Copy entrypoint script
COPY --chown=algotrader:algotrader docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER algotrader
WORKDIR /home/algotrader

EXPOSE 8080 7681

ENTRYPOINT ["/entrypoint.sh"]
```

**Final Image Size Estimate**:
- Base (bookworm-slim): ~80MB
- ca-certificates: ~5MB
- ttyd: ~2MB
- Rust binary (static): ~15MB
- **Total**: ~102MB (vs 1.2GB with full Rust image)

#### Entrypoint Script (`docker/entrypoint.sh`)

**Purpose**: Start daemon + ttyd in parallel, handle shutdown gracefully

```bash
#!/bin/bash
set -e

# Function to handle shutdown
shutdown() {
    echo "Shutting down gracefully..."
    kill -TERM "$daemon_pid" 2>/dev/null || true
    kill -TERM "$ttyd_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
    wait "$ttyd_pid" 2>/dev/null || true
    exit 0
}

# Trap SIGTERM and SIGINT
trap shutdown TERM INT

# Start trading daemon in background
echo "Starting trading daemon..."
algo-trade run --config "${CONFIG_PATH:-/config/Config.toml}" &
daemon_pid=$!

# Wait for daemon to initialize (2 seconds)
sleep 2

# Start ttyd for TUI access
echo "Starting ttyd on port 7681..."
ttyd -p 7681 -W algo-trade live-bot-tui &
ttyd_pid=$!

# Wait for either process to exit
wait -n "$daemon_pid" "$ttyd_pid"

# If we reach here, one process exited - trigger shutdown
shutdown
```

**Key Features**:
- Parallel execution: Daemon and ttyd run simultaneously
- Signal propagation: SIGTERM forwarded to both processes
- Graceful shutdown: Waits for both processes to exit cleanly
- Exit code: Clean exit (0) after shutdown

#### docker-compose.yml

```yaml
version: '3.8'

services:
  timescaledb:
    image: timescale/timescaledb:latest-pg17
    container_name: algo-trade-db
    environment:
      POSTGRES_DB: algo_trade
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD_FILE: /run/secrets/db_password
      TIMESCALEDB_TELEMETRY: 'off'
      TS_TUNE_MEMORY: 4GB
      TS_TUNE_NUM_CPUS: 4
    volumes:
      - timescale-data:/var/lib/postgresql/data
      - ./scripts/setup_timescale.sql:/docker-entrypoint-initdb.d/01-init.sql:ro
    secrets:
      - db_password
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres -d algo_trade"]
      interval: 5s
      timeout: 5s
      retries: 5
      start_period: 10s
    ports:
      - "5432:5432"  # Expose for development (remove in production)
    restart: unless-stopped
    networks:
      - algo-trade-network

  app:
    build:
      context: .
      dockerfile: Dockerfile
    container_name: algo-trade-app
    environment:
      # Database connection
      DATABASE_URL: postgresql://postgres:${DB_PASSWORD}@timescaledb:5432/algo_trade

      # Bot persistence (SQLite)
      BOT_DATABASE_URL: sqlite:///data/bots.db

      # Hyperliquid API (from .env)
      HYPERLIQUID_API_URL: ${HYPERLIQUID_API_URL:-https://api.hyperliquid.xyz}
      HYPERLIQUID_WS_URL: ${HYPERLIQUID_WS_URL:-wss://api.hyperliquid.xyz/ws}

      # Logging
      RUST_LOG: ${RUST_LOG:-info}

      # Config path
      CONFIG_PATH: /config/Config.toml
    volumes:
      - sqlite-data:/data                          # Bot configurations persist
      - ./config/Config.toml:/config/Config.toml:ro # Read-only config
    secrets:
      - db_password
    depends_on:
      timescaledb:
        condition: service_healthy
    ports:
      - "8080:8080"  # Web API
      - "7681:7681"  # ttyd TUI
    restart: unless-stopped
    stop_grace_period: 30s  # Allow 30s for graceful shutdown
    networks:
      - algo-trade-network

volumes:
  timescale-data:
    driver: local
  sqlite-data:
    driver: local

secrets:
  db_password:
    file: ./secrets/db_password.txt

networks:
  algo-trade-network:
    driver: bridge
```

#### .dockerignore

**Purpose**: Reduce build context size, exclude unnecessary files

```
# Git
.git/
.gitignore

# Build artifacts
target/
**/*.rs.bk
*.pdb

# Logs
*.log
quad_ma_debug.log

# Cache
cache/

# Test data
tests/data/

# IDE
.vscode/
.idea/
*.swp
*.swo

# Documentation (not needed at runtime)
*.md
.claude/

# CI/CD
.github/

# Environment
.env
.env.*

# Database files (should be in volumes, not image)
*.db
*.db-shm
*.db-wal

# Temporary
tmp/
temp/
```

**Size Impact**: Reduces build context from ~500MB to ~50MB (excludes target/, cache/, .git/)

#### Secret Management

**File**: `./secrets/db_password.txt`
```
your_secure_password_here
```

**Security**:
- Add to `.gitignore` (NEVER commit secrets)
- Use Docker secrets (not environment variables in docker-compose.yml)
- Production: Use Docker Swarm secrets or Kubernetes secrets

**.gitignore Entry**:
```
secrets/
```

---

## 5. Edge Cases & Constraints

### 5.1 Data Persistence Failures

**Issue**: Volume data corruption or loss
**Scenario**: Docker volume deleted, disk failure, accidental `docker volume prune`
**Impact**: Loss of all bot configurations + historical trade data
**Mitigation**:
- Backup strategy: `docker run --rm -v timescale-data:/data -v $(pwd):/backup alpine tar czf /backup/timescale-backup.tar.gz /data`
- Health monitoring: Check volume mounts on startup
- Recovery: Document restore procedure in README

### 5.2 TimescaleDB Initialization Race Condition

**Issue**: App starts before TimescaleDB schema fully initialized
**Scenario**: Healthcheck passes but hypertables not yet created (init script still running)
**Impact**: App queries fail with "table does not exist" errors
**Mitigation**:
- Use `depends_on.condition: service_healthy` (already in design)
- Add `start_period: 10s` to healthcheck (allows init script time)
- App-level retry: Add retry logic to database client initialization

**Enhanced Healthcheck**:
```yaml
healthcheck:
  test: ["CMD-SHELL", "pg_isready -U postgres -d algo_trade && psql -U postgres -d algo_trade -c 'SELECT 1 FROM ohlcv LIMIT 1' > /dev/null 2>&1"]
  interval: 5s
  timeout: 5s
  retries: 5
  start_period: 15s  # Allow time for hypertable creation
```

### 5.3 Environment Variable Injection Failures

**Issue**: Missing or malformed environment variables
**Scenario**: `.env` file not created, typo in variable name, secret file missing
**Impact**: App crashes on startup with connection errors
**Mitigation**:
- Provide `.env.example` with all required variables
- Validation script: Check required env vars before container start
- Sensible defaults: Use `${VAR:-default}` syntax in docker-compose.yml

**Example `.env.example`**:
```bash
# Database
DB_PASSWORD=your_secure_password_here

# Hyperliquid API
HYPERLIQUID_API_URL=https://api.hyperliquid.xyz
HYPERLIQUID_WS_URL=wss://api.hyperliquid.xyz/ws

# Logging
RUST_LOG=info
```

### 5.4 Port Conflicts

**Issue**: Ports 8080, 7681, or 5432 already in use on host
**Scenario**: User runs multiple instances, other services using same ports
**Impact**: `docker compose up` fails with "port already allocated" error
**Mitigation**:
- Document port mappings clearly in README
- Allow customization via environment variables:
  ```yaml
  ports:
    - "${API_PORT:-8080}:8080"
    - "${TUI_PORT:-7681}:7681"
    - "${DB_PORT:-5432}:5432"
  ```
- Production: Remove DB port exposure (internal only)

### 5.5 Build Time Optimization Failures

**Issue**: Docker build cache invalidates frequently
**Scenario**: Changes to source code invalidate cargo-chef recipe cache
**Impact**: Full rebuild takes 5+ minutes instead of <1 minute
**Mitigation**:
- Ensure cargo-chef stage ONLY copies Cargo.toml/Cargo.lock (not source)
- Use BuildKit cache mounts (requires `DOCKER_BUILDKIT=1`)
- CI/CD: Use persistent cache volumes between builds

**Cache-Friendly Build Command**:
```bash
DOCKER_BUILDKIT=1 docker compose build --build-arg BUILDKIT_INLINE_CACHE=1
```

### 5.6 TUI Terminal Compatibility

**Issue**: TUI colors/formatting broken in ttyd web terminal
**Scenario**: Ratatui rendering issues in browser vs native terminal
**Impact**: Garbled display, missing UI elements
**Mitigation**:
- ttyd uses Xterm.js (full terminal emulation - should work)
- Test TUI rendering in ttyd during development
- Fallback: Provide docker exec option in README

**Test Command**:
```bash
docker exec -it algo-trade-app algo-trade live-bot-tui
```

### 5.7 Graceful Shutdown Timeout

**Issue**: Bot shutdown takes longer than Docker's default 10s timeout
**Scenario**: Many open positions, slow network, exchange API delays
**Impact**: SIGKILL forces immediate termination (potential data loss)
**Mitigation**:
- Set `stop_grace_period: 30s` (already in design)
- Monitor shutdown logs: `docker compose logs app` after stop
- Increase timeout if needed: `stop_grace_period: 60s`

### 5.8 SQLite Migration Path Errors

**Issue**: Bot orchestrator migrations not found at runtime
**Scenario**: Migrations copied to wrong path, volume mount issues
**Impact**: `sqlx::migrate!` fails, bots.db not initialized
**Mitigation**:
- COPY migrations to `/app/migrations` in Dockerfile (line 36 of runtime stage)
- Set `SQLX_MIGRATIONS_DIR=/app/migrations` environment variable
- Verify path in entrypoint script

**Alternative**: Embed migrations in binary
```rust
sqlx::migrate!("./migrations").run(&pool).await?;
```

### 5.9 Secret Exposure via Logs

**Issue**: Database password logged in plaintext
**Scenario**: Connection string logged during error, debug logs enabled
**Impact**: Credentials leak in container logs
**Mitigation**:
- Never log full connection strings
- Use `RUST_LOG=info` (not `debug`) in production
- Filter sensitive fields in tracing subscriber

**Example** (safe logging):
```rust
tracing::info!("Connecting to database at {}", db_url.split('@').last().unwrap_or("unknown"));
```

### 5.10 Image Size Bloat

**Issue**: Final image larger than expected
**Scenario**: Debug symbols included, unnecessary dependencies installed
**Impact**: Slower pulls, higher storage costs
**Mitigation**:
- Use `--release` build (strips debug symbols)
- Multi-stage build (exclude build tools from runtime)
- Clean apt cache: `rm -rf /var/lib/apt/lists/*`
- Alpine alternative: `rust:1.75-alpine` + `alpine:latest` (55MB total, but musl compatibility risk)

**Current Design**: ~102MB (acceptable for trading system)

---

## 6. TaskMaster Handoff Package

### MUST DO

#### File Creation Tasks

1. **Create `/home/a/Work/algo-trade/Dockerfile`**
   - Multi-stage build (base → planner → builder → runtime)
   - Base: `rust:1.75-bookworm` with cargo-chef + sccache
   - Runtime: `debian:bookworm-slim` with ttyd
   - Non-root user: `algotrader` (UID 10001)
   - Copy binary from builder: `/usr/local/bin/algo-trade`
   - Copy migrations: `/app/migrations`
   - Create data directory: `/data` (for bots.db)
   - EXPOSE ports: 8080 (API), 7681 (ttyd)
   - ENTRYPOINT: `/entrypoint.sh`

2. **Create `/home/a/Work/algo-trade/docker-compose.yml`**
   - Service: `timescaledb` (image: `timescale/timescaledb:latest-pg17`)
     - Environment: `POSTGRES_DB=algo_trade`, `POSTGRES_USER=postgres`, password from secret
     - Volume: `timescale-data:/var/lib/postgresql/data`
     - Init script: `./scripts/setup_timescale.sql:/docker-entrypoint-initdb.d/01-init.sql:ro`
     - Healthcheck: `pg_isready -U postgres -d algo_trade`
     - Ports: `5432:5432`
   - Service: `app` (build: `.`)
     - Environment: `DATABASE_URL`, `BOT_DATABASE_URL=sqlite:///data/bots.db`, Hyperliquid URLs
     - Volume: `sqlite-data:/data`, `./config/Config.toml:/config/Config.toml:ro`
     - Depends on: `timescaledb` (condition: `service_healthy`)
     - Ports: `8080:8080`, `7681:7681`
     - Stop grace period: `30s`
   - Volumes: `timescale-data`, `sqlite-data`
   - Secrets: `db_password` (file: `./secrets/db_password.txt`)
   - Network: `algo-trade-network` (bridge)

3. **Create `/home/a/Work/algo-trade/.dockerignore`**
   - Exclude: `.git/`, `target/`, `*.log`, `cache/`, `tests/data/`, `.vscode/`, `.idea/`
   - Exclude: `*.md`, `.claude/`, `.github/`, `.env*`, `*.db*`, `tmp/`, `temp/`

4. **Create `/home/a/Work/algo-trade/docker/entrypoint.sh`**
   - Shebang: `#!/bin/bash`
   - Set: `set -e`
   - Define shutdown function: Kill daemon + ttyd PIDs, wait for clean exit
   - Trap SIGTERM and SIGINT: `trap shutdown TERM INT`
   - Start daemon: `algo-trade run --config "${CONFIG_PATH:-/config/Config.toml}" &`
   - Capture daemon PID: `daemon_pid=$!`
   - Sleep 2 seconds (allow daemon initialization)
   - Start ttyd: `ttyd -p 7681 -W algo-trade live-bot-tui &`
   - Capture ttyd PID: `ttyd_pid=$!`
   - Wait for either process: `wait -n "$daemon_pid" "$ttyd_pid"`
   - Call shutdown function on exit

5. **Create `/home/a/Work/algo-trade/secrets/db_password.txt`**
   - Single line: Database password (e.g., `secure_password_here`)
   - **MUST** add to `.gitignore` (security critical)

6. **Create `/home/a/Work/algo-trade/.env.example`**
   - Document all environment variables with examples
   - Variables: `DB_PASSWORD`, `HYPERLIQUID_API_URL`, `HYPERLIQUID_WS_URL`, `RUST_LOG`, `API_PORT`, `TUI_PORT`, `DB_PORT`

7. **Update `/home/a/Work/algo-trade/.gitignore`**
   - Add: `secrets/`, `.env`, `*.db`, `*.db-shm`, `*.db-wal`

8. **Update `/home/a/Work/algo-trade/README.md`**
   - Section: "Docker Deployment"
   - Commands: `docker compose build`, `docker compose up -d`, `docker compose logs -f app`
   - Access methods: Web API (http://localhost:8080), TUI (http://localhost:7681)
   - Management: `docker compose stop`, `docker compose down`, `docker compose down -v` (delete volumes)
   - Backup: Volume backup/restore commands

#### Configuration Tasks

9. **Verify `/home/a/Work/algo-trade/config/Config.toml`**
   - Ensure `[server]` section: `host = "0.0.0.0"`, `port = 8080`
   - Ensure `[database]` section: `url = "postgresql://postgres:password@timescaledb:5432/algo_trade"`
   - Note: Connection string uses service name `timescaledb` (NOT `localhost`)

10. **Create `/home/a/Work/algo-trade/config/Config.docker.toml`**
    - Override for Docker environment
    - Database URL: Use environment variable `${DATABASE_URL}`
    - Server host: `0.0.0.0` (listen on all interfaces)

#### Testing & Verification

11. **Build Test**: `DOCKER_BUILDKIT=1 docker compose build`
    - Expected: Build completes in <2 minutes (first build), <30s (cached)
    - Verify: Final image size ~100MB

12. **Start Test**: `docker compose up -d`
    - Expected: TimescaleDB healthcheck passes in ~15s
    - Expected: App container starts after DB healthy
    - Expected: No errors in `docker compose logs app`

13. **API Test**: `curl http://localhost:8080/health` (if health endpoint exists)
    - Expected: 200 OK response

14. **TUI Test**: Open `http://localhost:7681` in browser
    - Expected: Terminal interface renders correctly
    - Expected: Can run `algo-trade live-bot-tui` interactively

15. **Persistence Test**:
    - Create bot via TUI
    - Stop containers: `docker compose down`
    - Start containers: `docker compose up -d`
    - Verify: Bot configuration restored from bots.db

16. **Graceful Shutdown Test**: `docker compose stop`
    - Expected: Logs show "Shutting down all bots..."
    - Expected: Container exits with code 0 (not 137/SIGKILL)

### MUST NOT DO

1. **DO NOT hardcode secrets in docker-compose.yml or Dockerfile**
   - Use Docker secrets, environment variables, or .env files
   - Never commit secrets to git

2. **DO NOT run containers as root**
   - Always create non-root user (UID > 10000)
   - Always use `USER algotrader` directive in Dockerfile

3. **DO NOT expose database port 5432 in production**
   - Only expose for development/debugging
   - Remove port mapping in production docker-compose.yml

4. **DO NOT mount host directory to `/docker-entrypoint-initdb.d/`**
   - This removes TimescaleDB setup files
   - Always COPY init scripts into image at build time

5. **DO NOT skip healthchecks**
   - Always define healthcheck on database service
   - Always use `depends_on.condition: service_healthy`

6. **DO NOT use `FROM scratch` for runtime image**
   - Rust binary requires glibc (not available in scratch)
   - Use `debian:bookworm-slim` for compatibility

7. **DO NOT copy entire workspace in planner stage**
   - Only copy `Cargo.toml`, `Cargo.lock`, and workspace member manifests
   - Avoids cache invalidation on source code changes

8. **DO NOT forget .dockerignore**
   - Without it, entire `target/` directory copied (500MB+)
   - Slows builds, increases build context transfer time

9. **DO NOT use Alpine unless tested thoroughly**
   - musl libc compatibility issues with PostgreSQL client libraries
   - 40MB savings NOT worth potential runtime failures

10. **DO NOT skip BuildKit cache mounts**
    - Without cache mounts, dependencies rebuilt on every change
    - Use `--mount=type=cache` for registry and sccache

### RECOMMENDED DESIGN DECISIONS

#### Architecture
- **Pattern**: Multi-container with Docker Compose
- **Database**: Separate TimescaleDB service (official image)
- **App**: Single container running daemon + ttyd
- **Networking**: Automatic service discovery (default Compose network)
- **Volumes**: Named volumes for persistence (timescale-data, sqlite-data)

#### TUI Access Method
- **Primary**: ttyd on port 7681 (web-based terminal)
- **Fallback**: `docker exec -it algo-trade-app algo-trade live-bot-tui`
- **NOT RECOMMENDED**: SSH (adds 50MB to image, complex setup)

#### Base Images
- **Builder**: `rust:1.75-bookworm` (full Rust toolchain)
- **Runtime**: `debian:bookworm-slim` (minimal, glibc compatible)
- **Database**: `timescale/timescaledb:latest-pg17` (official, auto-tuned)

#### Build Optimization
- **Tool**: cargo-chef (dependency caching)
- **Tool**: sccache (compilation artifact caching)
- **Technique**: Multi-stage build (exclude build tools from runtime)
- **Technique**: BuildKit cache mounts (persist caches across builds)

#### Security
- **User**: Non-root user `algotrader` (UID 10001)
- **Secrets**: File-based Docker secrets (NOT environment variables)
- **Capabilities**: No elevated privileges required
- **Network**: Internal bridge network (database not exposed to internet)

#### Deployment Commands

**Initial Setup**:
```bash
# 1. Create secrets directory
mkdir -p secrets
echo "your_secure_password" > secrets/db_password.txt
chmod 600 secrets/db_password.txt

# 2. Copy and customize config
cp config/Config.example.toml config/Config.toml
# Edit Config.toml to use timescaledb:5432 (NOT localhost)

# 3. Build images
DOCKER_BUILDKIT=1 docker compose build

# 4. Start services
docker compose up -d

# 5. View logs
docker compose logs -f app
```

**Daily Operations**:
```bash
# Start
docker compose up -d

# Stop (graceful)
docker compose stop

# Restart
docker compose restart app

# View logs
docker compose logs -f app

# Access TUI (browser)
open http://localhost:7681

# Access TUI (terminal)
docker exec -it algo-trade-app algo-trade live-bot-tui

# Shell access
docker exec -it algo-trade-app /bin/bash
```

**Data Management**:
```bash
# Backup TimescaleDB
docker run --rm \
  -v algo-trade_timescale-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar czf /backup/timescale-$(date +%Y%m%d).tar.gz /data

# Backup SQLite bots.db
docker run --rm \
  -v algo-trade_sqlite-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar czf /backup/sqlite-$(date +%Y%m%d).tar.gz /data

# Restore TimescaleDB
docker run --rm \
  -v algo-trade_timescale-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar xzf /backup/timescale-YYYYMMDD.tar.gz -C /

# Clean up (DELETE ALL DATA)
docker compose down -v
```

**Troubleshooting**:
```bash
# Check container status
docker compose ps

# Check healthchecks
docker compose ps timescaledb

# View full logs
docker compose logs

# Rebuild from scratch
docker compose down
docker compose build --no-cache
docker compose up -d

# Inspect volumes
docker volume ls
docker volume inspect algo-trade_timescale-data
docker volume inspect algo-trade_sqlite-data
```

#### Port Mappings

| Port | Service | Purpose | External Access |
|------|---------|---------|-----------------|
| 8080 | App | Web API (REST) | http://localhost:8080 |
| 7681 | App | TUI (ttyd) | http://localhost:7681 |
| 5432 | TimescaleDB | PostgreSQL | localhost:5432 (dev only) |

#### Environment Variables Reference

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes | - | PostgreSQL connection string |
| `BOT_DATABASE_URL` | No | `sqlite:///data/bots.db` | SQLite path for bot configs |
| `HYPERLIQUID_API_URL` | No | `https://api.hyperliquid.xyz` | Hyperliquid REST API |
| `HYPERLIQUID_WS_URL` | No | `wss://api.hyperliquid.xyz/ws` | Hyperliquid WebSocket |
| `RUST_LOG` | No | `info` | Log level (trace/debug/info/warn/error) |
| `CONFIG_PATH` | No | `/config/Config.toml` | Path to config file in container |
| `DB_PASSWORD` | Yes | - | Database password (from secret) |

#### File Structure After Implementation

```
/home/a/Work/algo-trade/
├── Dockerfile                          # NEW: Multi-stage build
├── docker-compose.yml                  # NEW: Service orchestration
├── .dockerignore                       # NEW: Build context optimization
├── .env.example                        # NEW: Environment variable template
├── .gitignore                          # UPDATED: Add secrets/, .env
├── README.md                           # UPDATED: Add Docker deployment section
├── docker/
│   └── entrypoint.sh                   # NEW: Container initialization
├── secrets/
│   └── db_password.txt                 # NEW: Database password (git-ignored)
├── config/
│   ├── Config.toml                     # EXISTING: Modified for Docker
│   └── Config.docker.toml              # NEW: Docker-specific overrides
├── scripts/
│   └── setup_timescale.sql             # EXISTING: Mounted to TimescaleDB
├── crates/
│   └── bot-orchestrator/
│       └── migrations/
│           └── *.sql                   # EXISTING: Copied to app container
└── (all other files unchanged)
```

---

## 7. Report Generation

### Summary

This context report provides comprehensive research and architectural design for Dockerizing the Hyperliquid algorithmic trading system. The recommended solution uses Docker Compose to orchestrate two services:

1. **TimescaleDB Service**: Official `timescale/timescaledb:latest-pg17` image with auto-initialization
2. **Trading App Service**: Multi-stage Rust build with daemon + ttyd for TUI access

**Key Features**:
- ✅ Self-contained deployment: `docker compose up -d`
- ✅ TimescaleDB auto-initializes with hypertables
- ✅ Daemon auto-starts on container boot
- ✅ TUI accessible via web browser (http://localhost:7681)
- ✅ Data persists across restarts (named volumes)
- ✅ Graceful shutdown (SIGTERM handling already implemented)
- ✅ Security hardened (non-root user, Docker secrets)
- ✅ Build optimized (cargo-chef: 89% faster builds)
- ✅ Image size optimized (~102MB runtime image)

**Ready for TaskMaster**: Section 6 contains complete MUST DO/MUST NOT DO lists with exact file paths, line-by-line specifications, and verification steps.

### Research Sources

1. **Rust Docker Best Practices**: https://depot.dev/blog/rust-dockerfile-best-practices
2. **TimescaleDB Docker**: https://github.com/timescale/timescaledb-docker
3. **ttyd (Web Terminal)**: https://github.com/tsl0922/ttyd
4. **Docker Compose Networking**: https://docs.docker.com/compose/how-tos/networking/
5. **Docker Security**: https://betterstack.com/community/guides/scaling-docker/docker-security-best-practices/
6. **Tokio Graceful Shutdown**: https://tokio.rs/tokio/topics/shutdown

### Next Steps

1. **Pass this report to TaskMaster** for atomic task generation
2. **TaskMaster reads Section 6** (TaskMaster Handoff Package)
3. **TaskMaster generates playbook** with 16 atomic tasks (file creation, configuration, testing)
4. **Execute tasks sequentially** following playbook
5. **Verify with Karen** after implementation (cargo build, clippy, runtime tests)

---

**Report Status**: ✅ Complete - Ready for handoff to TaskMaster

**File Path**: `/home/a/Work/algo-trade/.claude/context/2025-10-07_docker-containerization.md`
