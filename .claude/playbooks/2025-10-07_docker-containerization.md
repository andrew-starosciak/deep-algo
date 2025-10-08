# Playbook: Docker Containerization for Hyperliquid Trading System

**Date**: 2025-10-07
**Author**: TaskMaster Agent
**Status**: Ready for Execution
**Context Report**: `/home/a/Work/algo-trade/.claude/context/2025-10-07_docker-containerization.md`

---

## User Request

> Create a docker container for the entire system that automatically runs the trading daemon when it boots, sets up TimescaleDB/PostgreSQL, and exposes the TUI for remote access.

---

## Scope Boundaries

### MUST DO

1. **Create Docker Infrastructure Files**
   - ✅ Multi-stage Dockerfile with cargo-chef optimization
   - ✅ docker-compose.yml for multi-container orchestration
   - ✅ .dockerignore for build optimization
   - ✅ Entrypoint script for daemon + ttyd initialization
   - ✅ .env.example for environment variable documentation

2. **Database Setup**
   - ✅ TimescaleDB service with official image (timescale/timescaledb:latest-pg17)
   - ✅ Auto-initialize schema from `/home/a/Work/algo-trade/scripts/setup_timescale.sql`
   - ✅ Healthcheck for database availability
   - ✅ Volume persistence for TimescaleDB and SQLite (bots.db)

3. **TUI Remote Access**
   - ✅ ttyd web-based terminal on port 7681
   - ✅ Daemon auto-start with web API on port 8080
   - ✅ Parallel execution (daemon + ttyd)

4. **Security & Best Practices**
   - ✅ Non-root user (algotrader, UID 10001)
   - ✅ Docker secrets for database password
   - ✅ Graceful shutdown handling (SIGTERM propagation)
   - ✅ Secret files excluded from git

5. **Documentation**
   - ✅ Update README.md with Docker deployment section
   - ✅ Document commands: build, start, stop, backup, restore
   - ✅ Troubleshooting guide

### MUST NOT DO

1. ❌ Hardcode secrets in docker-compose.yml or Dockerfile
2. ❌ Run containers as root user
3. ❌ Expose database port 5432 in production (dev only)
4. ❌ Mount host directory to `/docker-entrypoint-initdb.d/` (removes TimescaleDB setup)
5. ❌ Skip healthchecks or depends_on conditions
6. ❌ Use Alpine base image (musl compatibility risk)
7. ❌ Copy entire workspace in planner stage (cache invalidation)
8. ❌ Omit .dockerignore (500MB+ build context bloat)
9. ❌ Skip BuildKit cache mounts (slow builds)
10. ❌ Forget to make entrypoint.sh executable

### RECOMMENDED DESIGN

**Architecture**: Multi-container with Docker Compose
- **Service 1**: TimescaleDB (official image)
- **Service 2**: App (multi-stage Rust build with daemon + ttyd)
- **Networking**: Automatic service discovery via Compose network
- **Volumes**: Named volumes for persistence (timescale-data, sqlite-data)
- **TUI Access**: ttyd on port 7681 (web browser access)
- **Build Optimization**: cargo-chef + sccache (89% faster builds)
- **Security**: Non-root user, file-based Docker secrets

---

## Atomic Tasks

### Phase 1: Docker Foundation (6 tasks)

#### Task 1.1: Create .dockerignore

**File**: `/home/a/Work/algo-trade/.dockerignore`
**Location**: New file (entire file)
**Action**: Create .dockerignore to optimize build context (exclude target/, .git/, logs, test data)
**Verification**: `ls -lh .dockerignore` (verify file exists)
**Estimated LOC**: 35

**Content**:
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

---

#### Task 1.2: Create Dockerfile - Stage 1 & 2 (Base + Planner)

**File**: `/home/a/Work/algo-trade/Dockerfile`
**Location**: Lines 1-20 (Stages: base, planner)
**Action**: Create base stage with cargo-chef + sccache, and planner stage for dependency analysis
**Verification**: `docker build --target planner -t algo-trade-planner .`
**Estimated LOC**: 20

**Content**:
```dockerfile
# Stage 1: Base - Install cargo-chef and sccache
FROM rust:1.75-bookworm AS base
RUN cargo install cargo-chef --locked
RUN cargo install sccache --locked
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_DIR=/sccache

# Stage 2: Planner - Generate dependency recipe
FROM base AS planner
WORKDIR /app
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef prepare --recipe-path recipe.json
```

---

#### Task 1.3: Create Dockerfile - Stage 3 (Builder)

**File**: `/home/a/Work/algo-trade/Dockerfile`
**Location**: Lines 21-35 (Stage: builder)
**Action**: Add builder stage that compiles dependencies (cached) and then builds the binary
**Verification**: `docker build --target builder -t algo-trade-builder .`
**Estimated LOC**: 15

**Content** (append to Dockerfile):
```dockerfile
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
    cargo build --release --bin algo-trade-cli
```

---

#### Task 1.4: Create Dockerfile - Stage 4 (Runtime)

**File**: `/home/a/Work/algo-trade/Dockerfile`
**Location**: Lines 36-65 (Stage: runtime)
**Action**: Create minimal runtime image with Debian slim, ttyd, non-root user, and binary
**Verification**: `docker build -t algo-trade:latest .`
**Estimated LOC**: 30

**Content** (append to Dockerfile):
```dockerfile
# Stage 4: Runtime - Minimal Debian image with ttyd
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    ttyd \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user (UID 10001 to avoid system UID overlap)
RUN useradd -u 10001 -m -s /bin/bash algotrader

# Copy binary from builder
COPY --from=builder --chown=algotrader:algotrader \
    /app/target/release/algo-trade-cli /usr/local/bin/algo-trade

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
```

---

#### Task 1.5: Create docker/ directory and entrypoint.sh

**File**: `/home/a/Work/algo-trade/docker/entrypoint.sh`
**Location**: New file (entire file)
**Action**: Create entrypoint script that starts daemon + ttyd in parallel with graceful shutdown
**Verification**: `cat docker/entrypoint.sh | head -5` (verify shebang)
**Estimated LOC**: 35

**Content**:
```bash
#!/bin/bash
set -e

# Function to handle graceful shutdown
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

# Wait for daemon to initialize
sleep 2

# Start ttyd for TUI access
echo "Starting ttyd on port 7681..."
ttyd -p 7681 -W algo-trade live-bot-tui &
ttyd_pid=$!

echo "Services started:"
echo "  - Trading daemon (PID: $daemon_pid)"
echo "  - ttyd web terminal (PID: $ttyd_pid)"
echo "Access TUI at http://localhost:7681"

# Wait for either process to exit
wait -n "$daemon_pid" "$ttyd_pid"

# If we reach here, one process exited - trigger shutdown
shutdown
```

---

#### Task 1.6: Make entrypoint.sh executable

**File**: `/home/a/Work/algo-trade/docker/entrypoint.sh`
**Location**: File permissions
**Action**: Set executable permission on entrypoint script
**Verification**: `ls -l docker/entrypoint.sh | grep -q rwx` (verify executable)
**Estimated LOC**: 0 (permission change only)

**Command** (will be done by Dockerfile COPY, but verify locally):
```bash
chmod +x docker/entrypoint.sh
```

---

### Phase 2: Docker Compose Orchestration (5 tasks)

#### Task 2.1: Create docker-compose.yml - Header and TimescaleDB service

**File**: `/home/a/Work/algo-trade/docker-compose.yml`
**Location**: Lines 1-35 (version, timescaledb service)
**Action**: Create docker-compose.yml with TimescaleDB service configuration
**Verification**: `docker compose config` (validate YAML syntax)
**Estimated LOC**: 35

**Content**:
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
      start_period: 15s
    ports:
      - "${DB_PORT:-5432}:5432"
    restart: unless-stopped
    networks:
      - algo-trade-network
```

---

#### Task 2.2: Add app service to docker-compose.yml

**File**: `/home/a/Work/algo-trade/docker-compose.yml`
**Location**: Lines 36-70 (app service)
**Action**: Add app service with build config, environment variables, and dependencies
**Verification**: `docker compose config | grep -A 20 "app:"` (verify app service)
**Estimated LOC**: 35

**Content** (append to docker-compose.yml):
```yaml
  app:
    build:
      context: .
      dockerfile: Dockerfile
    container_name: algo-trade-app
    environment:
      # Database connection (uses service name 'timescaledb')
      DATABASE_URL: postgresql://postgres:${DB_PASSWORD}@timescaledb:5432/algo_trade

      # Bot persistence (SQLite)
      BOT_DATABASE_URL: sqlite:///data/bots.db

      # Hyperliquid API
      HYPERLIQUID_API_URL: ${HYPERLIQUID_API_URL:-https://api.hyperliquid.xyz}
      HYPERLIQUID_WS_URL: ${HYPERLIQUID_WS_URL:-wss://api.hyperliquid.xyz/ws}

      # Logging
      RUST_LOG: ${RUST_LOG:-info}

      # Config path
      CONFIG_PATH: /config/Config.toml
    volumes:
      - sqlite-data:/data
      - ./config/Config.toml:/config/Config.toml:ro
    secrets:
      - db_password
    depends_on:
      timescaledb:
        condition: service_healthy
    ports:
      - "${API_PORT:-8080}:8080"
      - "${TUI_PORT:-7681}:7681"
    restart: unless-stopped
    stop_grace_period: 30s
    networks:
      - algo-trade-network
```

---

#### Task 2.3: Add volumes section to docker-compose.yml

**File**: `/home/a/Work/algo-trade/docker-compose.yml`
**Location**: Lines 71-78 (volumes)
**Action**: Define named volumes for TimescaleDB and SQLite persistence
**Verification**: `docker compose config | grep -A 5 "^volumes:"` (verify volumes)
**Estimated LOC**: 8

**Content** (append to docker-compose.yml):
```yaml
volumes:
  timescale-data:
    driver: local
  sqlite-data:
    driver: local
```

---

#### Task 2.4: Add secrets section to docker-compose.yml

**File**: `/home/a/Work/algo-trade/docker-compose.yml`
**Location**: Lines 79-83 (secrets)
**Action**: Define file-based secret for database password
**Verification**: `docker compose config | grep -A 3 "^secrets:"` (verify secrets)
**Estimated LOC**: 5

**Content** (append to docker-compose.yml):
```yaml
secrets:
  db_password:
    file: ./secrets/db_password.txt
```

---

#### Task 2.5: Add networks section to docker-compose.yml

**File**: `/home/a/Work/algo-trade/docker-compose.yml`
**Location**: Lines 84-87 (networks)
**Action**: Define bridge network for service communication
**Verification**: `docker compose config | grep -A 2 "^networks:"` (verify networks)
**Estimated LOC**: 4

**Content** (append to docker-compose.yml):
```yaml
networks:
  algo-trade-network:
    driver: bridge
```

---

### Phase 3: Secrets & Environment (3 tasks)

#### Task 3.1: Create secrets/ directory and .gitkeep

**File**: `/home/a/Work/algo-trade/secrets/.gitkeep`
**Location**: New file
**Action**: Create secrets directory structure (gitkeep placeholder)
**Verification**: `ls -ld secrets/` (verify directory exists)
**Estimated LOC**: 0

**Command**:
```bash
mkdir -p secrets
touch secrets/.gitkeep
```

---

#### Task 3.2: Create secrets/db_password.txt placeholder

**File**: `/home/a/Work/algo-trade/secrets/db_password.txt`
**Location**: New file (single line)
**Action**: Create placeholder password file with example password
**Verification**: `cat secrets/db_password.txt` (verify content)
**Estimated LOC**: 1

**Content**:
```
changeme_secure_password_here
```

**Note**: Users must change this before deployment.

---

#### Task 3.3: Create .env.example

**File**: `/home/a/Work/algo-trade/.env.example`
**Location**: New file (entire file)
**Action**: Document all environment variables with examples and descriptions
**Verification**: `cat .env.example | grep -c "="` (verify variable count)
**Estimated LOC**: 25

**Content**:
```bash
# Database Password (REQUIRED)
# This password is read from secrets/db_password.txt by Docker Compose
# Change the password in secrets/db_password.txt, then set it here for reference
DB_PASSWORD=changeme_secure_password_here

# Hyperliquid API Configuration
HYPERLIQUID_API_URL=https://api.hyperliquid.xyz
HYPERLIQUID_WS_URL=wss://api.hyperliquid.xyz/ws

# Logging Level
# Options: trace, debug, info, warn, error
RUST_LOG=info

# Port Mappings (optional overrides)
# Change these if default ports conflict with other services
API_PORT=8080
TUI_PORT=7681
DB_PORT=5432

# TimescaleDB Tuning (optional)
TS_TUNE_MEMORY=4GB
TS_TUNE_NUM_CPUS=4

# NOTE: Copy this file to .env and customize values
# Do NOT commit .env to git (it's in .gitignore)
```

---

### Phase 4: Git Configuration (2 tasks)

#### Task 4.1: Update .gitignore - Add Docker-related exclusions

**File**: `/home/a/Work/algo-trade/.gitignore`
**Location**: Append to end of file
**Action**: Add exclusions for secrets/, .env, and Docker-generated files
**Verification**: `grep -q "^secrets/" .gitignore` (verify secrets excluded)
**Estimated LOC**: 12

**Content** (append to .gitignore):
```
# Docker secrets (NEVER commit)
secrets/
!secrets/.gitkeep

# Docker environment files
.env
.env.local

# Docker volumes (local development)
*.db
*.db-shm
*.db-wal
```

---

#### Task 4.2: Verify .gitignore excludes sensitive files

**File**: `/home/a/Work/algo-trade/.gitignore`
**Location**: Verification only
**Action**: Verify that .gitignore properly excludes secrets/ and .env
**Verification**: `git status --ignored | grep -E "(secrets|\.env)"` (should show ignored)
**Estimated LOC**: 0

**Command**:
```bash
git check-ignore -v secrets/db_password.txt .env
```

**Expected Output**: Both files should be ignored.

---

### Phase 5: Documentation (4 tasks)

#### Task 5.1: Update README.md - Add Docker Deployment section (Part 1: Quick Start)

**File**: `/home/a/Work/algo-trade/README.md`
**Location**: New section after existing "Running" section
**Action**: Add Docker Quick Start section with initial setup commands
**Verification**: `grep -q "## Docker Deployment" README.md` (verify section exists)
**Estimated LOC**: 35

**Content** (insert new section):
```markdown
## Docker Deployment

### Quick Start

The entire trading system can be deployed using Docker Compose for a self-contained, production-ready setup.

#### Prerequisites

- Docker Engine 20.10+ with BuildKit enabled
- Docker Compose 2.0+
- 4GB RAM minimum (8GB recommended for TimescaleDB)
- 10GB disk space for Docker images and volumes

#### Initial Setup

1. **Create secrets directory and set database password**:
   ```bash
   mkdir -p secrets
   echo "your_secure_password_here" > secrets/db_password.txt
   chmod 600 secrets/db_password.txt
   ```

2. **Create environment file**:
   ```bash
   cp .env.example .env
   # Edit .env and set DB_PASSWORD to match secrets/db_password.txt
   nano .env
   ```

3. **Build Docker images**:
   ```bash
   DOCKER_BUILDKIT=1 docker compose build
   ```
   First build: ~5-10 minutes. Subsequent builds: <1 minute (with cache).

4. **Start services**:
   ```bash
   docker compose up -d
   ```

5. **Verify services are running**:
   ```bash
   docker compose ps
   docker compose logs -f app
   ```

#### Access Points

- **Web API**: http://localhost:8080
- **TUI (Web Terminal)**: http://localhost:7681
- **TimescaleDB**: postgresql://localhost:5432/algo_trade (development only)
```

---

#### Task 5.2: Update README.md - Add Docker Deployment section (Part 2: Management)

**File**: `/home/a/Work/algo-trade/README.md`
**Location**: Append to Docker Deployment section
**Action**: Add service management commands
**Verification**: `grep -q "### Managing Services" README.md` (verify subsection)
**Estimated LOC**: 40

**Content** (append to Docker Deployment section):
```markdown
### Managing Services

#### Daily Operations

```bash
# Start all services
docker compose up -d

# Stop all services (graceful shutdown)
docker compose stop

# Restart services
docker compose restart app

# View logs (all services)
docker compose logs -f

# View logs (app only)
docker compose logs -f app

# Check service status
docker compose ps

# Shell access to app container
docker exec -it algo-trade-app /bin/bash

# Access TUI via terminal (alternative to web)
docker exec -it algo-trade-app algo-trade live-bot-tui
```

#### Updating the Application

```bash
# Pull latest code
git pull

# Rebuild and restart
docker compose down
DOCKER_BUILDKIT=1 docker compose build
docker compose up -d
```

#### Complete Teardown

```bash
# Stop and remove containers (keeps volumes/data)
docker compose down

# Stop and remove everything including volumes (DELETES ALL DATA)
docker compose down -v
```
```

---

#### Task 5.3: Update README.md - Add Docker Deployment section (Part 3: Data Management)

**File**: `/home/a/Work/algo-trade/README.md`
**Location**: Append to Docker Deployment section
**Action**: Add backup and restore procedures
**Verification**: `grep -q "### Data Backup and Restore" README.md` (verify subsection)
**Estimated LOC**: 45

**Content** (append to Docker Deployment section):
```markdown
### Data Backup and Restore

#### Backup TimescaleDB

```bash
# Create backups directory
mkdir -p backups

# Backup TimescaleDB volume
docker run --rm \
  -v algo-trade_timescale-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar czf /backup/timescale-$(date +%Y%m%d-%H%M%S).tar.gz /data
```

#### Backup SQLite (Bot Configurations)

```bash
# Backup SQLite volume (bots.db)
docker run --rm \
  -v algo-trade_sqlite-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar czf /backup/sqlite-$(date +%Y%m%d-%H%M%S).tar.gz /data
```

#### Restore TimescaleDB

```bash
# Stop services first
docker compose down

# Restore from backup (replace YYYYMMDD-HHMMSS with your backup timestamp)
docker run --rm \
  -v algo-trade_timescale-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar xzf /backup/timescale-YYYYMMDD-HHMMSS.tar.gz -C /

# Restart services
docker compose up -d
```

#### Restore SQLite

```bash
# Stop services first
docker compose down

# Restore from backup
docker run --rm \
  -v algo-trade_sqlite-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar xzf /backup/sqlite-YYYYMMDD-HHMMSS.tar.gz -C /

# Restart services
docker compose up -d
```
```

---

#### Task 5.4: Update README.md - Add Docker Deployment section (Part 4: Troubleshooting)

**File**: `/home/a/Work/algo-trade/README.md`
**Location**: Append to Docker Deployment section
**Action**: Add troubleshooting guide for common Docker issues
**Verification**: `grep -q "### Troubleshooting" README.md` (verify subsection)
**Estimated LOC**: 50

**Content** (append to Docker Deployment section):
```markdown
### Troubleshooting

#### Services Won't Start

**Issue**: `docker compose up -d` fails or services keep restarting.

**Solutions**:

1. Check container logs:
   ```bash
   docker compose logs
   docker compose logs timescaledb
   docker compose logs app
   ```

2. Check service health:
   ```bash
   docker compose ps
   ```
   Ensure `timescaledb` shows `healthy` status.

3. Verify secrets file exists:
   ```bash
   ls -l secrets/db_password.txt
   ```

4. Check port conflicts:
   ```bash
   # On Linux/Mac
   lsof -i :8080
   lsof -i :7681
   lsof -i :5432

   # Change ports in .env if needed
   ```

#### Database Connection Errors

**Issue**: App logs show "connection refused" or "database does not exist".

**Solutions**:

1. Verify TimescaleDB healthcheck passed:
   ```bash
   docker compose ps timescaledb
   ```
   Status should be "healthy".

2. Check database initialization logs:
   ```bash
   docker compose logs timescaledb | grep "init"
   ```

3. Verify connection string (check service name, not localhost):
   ```bash
   docker compose config | grep DATABASE_URL
   ```
   Should contain `timescaledb:5432`, NOT `localhost:5432`.

#### TUI Not Accessible

**Issue**: http://localhost:7681 shows "connection refused".

**Solutions**:

1. Check if ttyd is running in app container:
   ```bash
   docker exec algo-trade-app ps aux | grep ttyd
   ```

2. Check app container logs:
   ```bash
   docker compose logs app | grep ttyd
   ```

3. Use alternative access method:
   ```bash
   docker exec -it algo-trade-app algo-trade live-bot-tui
   ```

#### Build Failures

**Issue**: `docker compose build` fails or takes extremely long.

**Solutions**:

1. Enable BuildKit (faster builds):
   ```bash
   export DOCKER_BUILDKIT=1
   docker compose build
   ```

2. Clear build cache and rebuild:
   ```bash
   docker compose build --no-cache
   ```

3. Check disk space:
   ```bash
   df -h
   docker system df
   ```

4. Prune unused Docker resources:
   ```bash
   docker system prune -a --volumes
   ```

#### Data Loss After Restart

**Issue**: Bot configurations or trade history missing after `docker compose down`.

**Solutions**:

1. Check if volumes still exist:
   ```bash
   docker volume ls | grep algo-trade
   ```

2. Avoid using `docker compose down -v` (deletes volumes):
   ```bash
   # SAFE: Stops containers, keeps data
   docker compose down

   # DESTRUCTIVE: Deletes all data
   docker compose down -v
   ```

3. Restore from backup (see Data Backup section above).

#### Performance Issues

**Issue**: Trading system slow or TimescaleDB consuming too much memory.

**Solutions**:

1. Adjust TimescaleDB tuning parameters in `.env`:
   ```bash
   TS_TUNE_MEMORY=2GB  # Reduce if low RAM
   TS_TUNE_NUM_CPUS=2  # Adjust to available CPUs
   ```

2. Restart services after changing .env:
   ```bash
   docker compose down
   docker compose up -d
   ```

3. Monitor resource usage:
   ```bash
   docker stats
   ```

### Architecture Details

**Services**:
- `timescaledb`: PostgreSQL 17 with TimescaleDB extension for time-series data
- `app`: Trading daemon + Web API + ttyd (web terminal)

**Volumes**:
- `timescale-data`: Persistent PostgreSQL data (/var/lib/postgresql/data)
- `sqlite-data`: Bot configurations (bots.db)

**Networks**:
- `algo-trade-network`: Internal bridge network for service communication

**Ports**:
- `8080`: Web API (REST endpoints)
- `7681`: ttyd web terminal (TUI access)
- `5432`: TimescaleDB (exposed for development only)

**Image Size**: ~102MB (runtime image)

**Build Time**:
- First build: 5-10 minutes
- Cached builds: <1 minute (with cargo-chef)
```

---

### Phase 6: Verification (6 tasks)

#### Task 6.1: Test Docker build

**File**: N/A (verification command)
**Location**: N/A
**Action**: Build Docker image and verify success
**Verification**: `DOCKER_BUILDKIT=1 docker compose build` (should exit 0)
**Estimated LOC**: 0

**Command**:
```bash
DOCKER_BUILDKIT=1 docker compose build
```

**Expected Output**:
- Build completes without errors
- Final image tagged as `algo-trade_app:latest`
- Image size approximately 100-150MB

---

#### Task 6.2: Test container startup and healthchecks

**File**: N/A (verification command)
**Location**: N/A
**Action**: Start all services and verify healthchecks pass
**Verification**: `docker compose up -d && sleep 20 && docker compose ps` (both services healthy)
**Estimated LOC**: 0

**Command**:
```bash
docker compose up -d
sleep 20  # Wait for healthchecks
docker compose ps
```

**Expected Output**:
- `timescaledb` status: `healthy`
- `app` status: `running`
- No restart loops

---

#### Task 6.3: Test Web API accessibility

**File**: N/A (verification command)
**Location**: N/A
**Action**: Verify Web API is accessible on port 8080
**Verification**: `curl -s http://localhost:8080/health || curl -s http://localhost:8080` (should connect)
**Estimated LOC**: 0

**Command**:
```bash
# Test if port is listening
curl -v http://localhost:8080 2>&1 | grep -i "connected"

# Check app logs
docker compose logs app | tail -20
```

**Expected Output**:
- Connection succeeds (even if 404, connection is established)
- Logs show "Web API server started" or similar

---

#### Task 6.4: Test TUI accessibility via ttyd

**File**: N/A (verification command)
**Location**: N/A
**Action**: Verify ttyd web terminal is accessible on port 7681
**Verification**: `curl -s http://localhost:7681 | grep -q "ttyd"` (should find ttyd HTML)
**Estimated LOC**: 0

**Command**:
```bash
# Test ttyd HTTP endpoint
curl -s http://localhost:7681 | head -20

# Check if ttyd process running
docker exec algo-trade-app ps aux | grep ttyd
```

**Expected Output**:
- HTTP response contains HTML (ttyd web interface)
- ttyd process visible in container

**Manual Verification**: Open http://localhost:7681 in browser, verify terminal renders.

---

#### Task 6.5: Test data persistence

**File**: N/A (verification command)
**Location**: N/A
**Action**: Verify volumes persist data across container restarts
**Verification**: Stop and restart containers, check volumes
**Estimated LOC**: 0

**Commands**:
```bash
# Check volumes exist
docker volume ls | grep algo-trade

# Stop containers
docker compose down

# Verify volumes still exist
docker volume ls | grep algo-trade

# Restart containers
docker compose up -d

# Verify volumes mounted
docker compose exec app ls -la /data
docker compose exec timescaledb ls -la /var/lib/postgresql/data
```

**Expected Output**:
- Volumes persist after `docker compose down`
- Data directories populated in containers after restart

---

#### Task 6.6: Test graceful shutdown

**File**: N/A (verification command)
**Location**: N/A
**Action**: Verify services shut down gracefully on SIGTERM
**Verification**: `docker compose stop && docker compose logs app | grep -i "shutdown"` (should show graceful shutdown)
**Estimated LOC**: 0

**Commands**:
```bash
# Stop services
docker compose stop

# Check shutdown logs
docker compose logs app | grep -i "shutdown"
docker compose logs app | tail -50
```

**Expected Output**:
- Logs show "Shutting down gracefully..."
- Logs show "Shutting down all bots..."
- Container exit code 0 (not 137/SIGKILL)

---

## Verification Checklist

### Build Verification
- [ ] `docker compose build` completes successfully
- [ ] Final image size ~100-150MB (check: `docker images | grep algo-trade`)
- [ ] No build warnings or errors

### Runtime Verification
- [ ] `docker compose up -d` starts both services
- [ ] TimescaleDB healthcheck passes (status: healthy)
- [ ] App container status: running (not restarting)
- [ ] No error messages in logs (`docker compose logs`)

### Accessibility Verification
- [ ] Web API accessible: `curl http://localhost:8080` connects
- [ ] TUI accessible: http://localhost:7681 opens in browser
- [ ] TUI terminal renders correctly (manual check)
- [ ] Alternative TUI access: `docker exec -it algo-trade-app algo-trade live-bot-tui` works

### Persistence Verification
- [ ] Volumes created: `docker volume ls | grep algo-trade` shows 2 volumes
- [ ] Data persists after `docker compose down && docker compose up -d`
- [ ] TimescaleDB data retained (tables exist after restart)
- [ ] SQLite bots.db retained (if bots created)

### Security Verification
- [ ] Container runs as non-root user: `docker exec algo-trade-app whoami` returns `algotrader`
- [ ] Secrets not in logs: `docker compose logs | grep -i password` shows no plaintext passwords
- [ ] `.gitignore` excludes `secrets/` and `.env`

### Shutdown Verification
- [ ] Graceful shutdown: `docker compose stop` shows "Shutting down gracefully" in logs
- [ ] Exit code 0: `docker compose ps -a` shows exited containers with code 0

### Karen Quality Review
- [ ] All Rust code compiles: `cargo build --release`
- [ ] Clippy passes: `cargo clippy -- -D warnings`
- [ ] No unused imports or dead code
- [ ] Documentation complete (README.md Docker section)

---

## Summary

### Deliverables

**New Files** (8):
1. `/home/a/Work/algo-trade/Dockerfile` (65 lines)
2. `/home/a/Work/algo-trade/docker-compose.yml` (87 lines)
3. `/home/a/Work/algo-trade/.dockerignore` (35 lines)
4. `/home/a/Work/algo-trade/docker/entrypoint.sh` (35 lines)
5. `/home/a/Work/algo-trade/secrets/db_password.txt` (1 line)
6. `/home/a/Work/algo-trade/.env.example` (25 lines)
7. `/home/a/Work/algo-trade/secrets/.gitkeep` (0 lines)

**Updated Files** (2):
1. `/home/a/Work/algo-trade/.gitignore` (+12 lines)
2. `/home/a/Work/algo-trade/README.md` (+170 lines)

**Total LOC**: ~430 lines

### Phases Summary

- **Phase 1**: Docker Foundation (6 tasks) - Dockerfile, entrypoint script
- **Phase 2**: Docker Compose Orchestration (5 tasks) - Multi-service configuration
- **Phase 3**: Secrets & Environment (3 tasks) - Security and configuration
- **Phase 4**: Git Configuration (2 tasks) - .gitignore updates
- **Phase 5**: Documentation (4 tasks) - Comprehensive README
- **Phase 6**: Verification (6 tasks) - End-to-end testing

**Total Tasks**: 26 atomic tasks

### Key Milestones

1. ✅ **Milestone 1** (After Phase 1): Docker image builds successfully
2. ✅ **Milestone 2** (After Phase 2): Services start and communicate
3. ✅ **Milestone 3** (After Phase 3): Secrets and environment configured
4. ✅ **Milestone 4** (After Phase 4): Git security enforced
5. ✅ **Milestone 5** (After Phase 5): Complete documentation
6. ✅ **Milestone 6** (After Phase 6): Full system verified

### Estimated Completion Time

- **Phase 1-2**: 2-3 hours (Docker infrastructure)
- **Phase 3-4**: 30 minutes (Secrets and git)
- **Phase 5**: 2 hours (Documentation)
- **Phase 6**: 1 hour (Verification and testing)

**Total**: 5-7 hours (including testing and troubleshooting)

### Post-Implementation: Karen Review

After completing all tasks, **MANDATORY** Karen review must verify:

1. **Phase 0**: Compilation - `cargo build --release --bin algo-trade-cli` succeeds
2. **Phase 1**: Clippy - `cargo clippy -- -D warnings` passes (zero warnings)
3. **Phase 2**: rust-analyzer - No diagnostics errors
4. **Phase 3**: Docker build - `docker compose build` succeeds
5. **Phase 4**: Docker run - `docker compose up -d` starts successfully
6. **Phase 5**: Integration - All verification tasks pass
7. **Phase 6**: Documentation - README complete and accurate

---

**Playbook Status**: ✅ Ready for Execution

**Next Steps**:
1. User approves playbook
2. Execute tasks sequentially (Phases 1-6)
3. Run Karen review after Phase 6
4. Fix any issues found by Karen
5. Mark playbook complete

---

**End of Playbook**
