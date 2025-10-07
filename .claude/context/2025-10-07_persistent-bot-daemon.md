# Context Report: Persistent Bot Daemon Architecture

**Date**: 2025-10-07
**Agent**: Context Gatherer
**Request**: Enable persistent, async bot operation independent of TUI

---

## Section 1: Request Analysis

### Explicit Requirements
1. **Bot Persistence**: Bots should run independently of TUI lifecycle
2. **TUI as Control Interface**: TUI becomes visual management/control, not runtime
3. **Async Operation**: Bots run asynchronously in background
4. **Modular Architecture**: Maintain existing modular design principles
5. **Future Process Spawning**: Support external processes spawning bots

### Implicit Requirements
1. **State Persistence**: Bot state should survive daemon restarts
2. **Graceful Shutdown**: Handle SIGTERM/SIGINT properly
3. **Auto-Restore**: Reload bots on daemon startup
4. **Multiple Clients**: Support multiple TUI/API clients connecting to same daemon
5. **Security**: Protect daemon API from unauthorized access
6. **Resource Management**: Prevent resource leaks, limit max bots

### Current Limitation Analysis
**Root Cause**: Bots are spawned within TUI event loop lifecycle (crates/cli/src/tui_live_bot.rs:172-201)
- BotRegistry created in TUI main function (line 181)
- When TUI exits, registry and all spawned bots are dropped
- No persistence mechanism for bot state or configuration

**Good News**: Architecture is already well-positioned for this change!
- BotRegistry uses Arc<RwLock<HashMap>> (thread-safe, shared ownership)
- Web API server already runs independently (crates/web-api/src/server.rs)
- Bots are tokio tasks, not tied to TUI threads
- Actor pattern with message passing already decouples bot lifecycle

---

## Section 2: Codebase Context

### Current Architecture Overview

#### Bot Lifecycle Management

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
```rust
// Lines 14-35: BotRegistry - in-memory registry
pub struct BotRegistry {
    bots: Arc<RwLock<HashMap<String, BotHandle>>>,
}

// Lines 40-82: Bot spawning
pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
    let (tx, rx) = mpsc::channel(32);
    let (event_tx, _event_rx) = broadcast::channel(1000);
    let (status_tx, status_rx) = watch::channel(initial_status);

    let handle = BotHandle::new(tx, event_tx.clone(), status_rx);
    let actor = BotActor::new(config, rx, event_tx, status_tx);

    tokio::spawn(async move {  // ← Bot runs in independent task
        if let Err(e) = actor.run().await {
            tracing::error!("Bot {} error: {}", bot_id_for_task, e);
        }
    });

    self.bots.write().await.insert(bot_id, handle.clone());
    Ok(handle)
}
```

**Analysis**: Registry already supports independent bot lifecycle. Only issue: no persistence.

#### Current CLI Structure

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
```rust
// Lines 90-171: Main function with subcommands
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match cli.command {
        Commands::Run { config } => run_trading_system(&config).await?,  // ← Starts web API
        Commands::Server { addr } => run_server(&addr).await?,            // ← Starts web API
        Commands::LiveBotTui { .. } => tui_live_bot::run().await?,       // ← Starts TUI
        // ... other commands
    }
}

// Lines 154-171: run_trading_system - launches web API with registry
async fn run_trading_system(config_path: &str) -> anyhow::Result<()> {
    let config = algo_trade_core::ConfigLoader::load()?;
    let registry = std::sync::Arc::new(algo_trade_bot_orchestrator::BotRegistry::new());
    let server = algo_trade_web_api::ApiServer::new(registry.clone());
    let addr = format!("{}:{}", config.server.host, config.server.port);
    server.serve(&addr).await?;  // ← Blocks forever, running daemon
    Ok(())
}
```

**Analysis**: `Commands::Run` already implements daemon pattern! It starts web API server that runs forever. Only missing:
1. Bot auto-restore on startup
2. Bot state persistence
3. Graceful shutdown handling

#### Web API Server

**File**: `/home/a/Work/algo-trade/crates/web-api/src/server.rs`
```rust
// Lines 11-52: ApiServer with REST + WebSocket support
pub struct ApiServer {
    registry: Arc<BotRegistry>,
}

impl ApiServer {
    pub async fn serve(self, addr: &str) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, self.router()).await?;  // ← Runs forever
        Ok(())
    }
}
```

**Endpoints**:
- `GET /api/bots` - List all bots
- `POST /api/bots` - Create bot
- `GET /api/bots/:bot_id` - Get bot status
- `PUT /api/bots/:bot_id/start` - Start bot
- `PUT /api/bots/:bot_id/stop` - Stop bot
- `DELETE /api/bots/:bot_id` - Delete bot
- `GET /ws` - WebSocket for real-time updates

**Analysis**: Web API is already production-ready! Can serve as daemon control plane.

#### TUI Implementation

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
```rust
// Lines 172-201: TUI creates its own registry (NOT shared with daemon)
pub async fn run() -> Result<()> {
    let registry = Arc::new(BotRegistry::new());  // ← Local, not persistent
    let mut app = App::new(registry.clone());
    let res = run_app(&mut terminal, &mut app).await;
    // ... cleanup
}
```

**Analysis**: TUI currently operates in isolation. To connect to daemon:
1. Replace local registry with HTTP client to daemon API
2. Use WebSocket for real-time updates
3. TUI becomes thin client

#### Bot Configuration

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
```rust
// Lines 34-71: BotConfig structure (already Serialize + Deserialize!)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
    pub enabled: bool,
    pub interval: String,
    pub ws_url: String,
    pub api_url: String,
    pub warmup_periods: usize,
    pub strategy_config: Option<String>,
    pub initial_capital: Decimal,
    pub risk_per_trade_pct: f64,
    pub max_position_pct: f64,
    pub leverage: u8,
    pub margin_mode: MarginMode,
    pub execution_mode: ExecutionMode,
    pub paper_slippage_bps: f64,
    pub paper_commission_rate: f64,
    #[serde(skip)]
    pub wallet: Option<WalletConfig>,  // ← Loaded from env, not persisted
}
```

**Analysis**: BotConfig is already serializable! Can be stored directly in database/file.

#### Existing Database Infrastructure

**File**: `/home/a/Work/algo-trade/crates/data/src/lib.rs`
```rust
pub use database::{BacktestResultRecord, DatabaseClient, OhlcvRecord};
```

**Analysis**: Project already uses PostgreSQL/TimescaleDB via `DatabaseClient`. Could reuse for bot state persistence, or use separate SQLite file for lightweight bot registry.

---

## Section 3: External Research

### 3.1 Daemon Service Patterns (Rust + Tokio)

#### Key Findings from Web Search

**Recommendation**: Use systemd for process management, NOT in-app daemonization
- Tokio doesn't mix well with fork-based daemonization (daemonize crate issues)
- Modern best practice: Write foreground Rust process, let systemd manage it
- systemd provides: auto-restart, logging, resource limits, startup ordering

**Graceful Shutdown Pattern** (from Tokio docs + Stack Overflow):
```rust
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Spawn daemon tasks
    let registry = Arc::new(BotRegistry::new());
    let server_handle = tokio::spawn(run_server(registry.clone()));

    // Wait for shutdown signal
    tokio::select! {
        _ = signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down...");
        }
        _ = signal_unix(SignalKind::terminate()) => {
            tracing::info!("Received SIGTERM, shutting down...");
        }
    }

    // Graceful shutdown
    registry.shutdown_all().await?;
    server_handle.abort();

    Ok(())
}

async fn signal_unix(kind: SignalKind) {
    signal::unix::signal(kind).unwrap().recv().await;
}
```

**Crate**: `tokio-graceful-shutdown` - Provides utility for managing shutdown across tasks

#### Systemd Service File Example
```ini
[Unit]
Description=Algo Trade Bot Daemon
After=network.target postgresql.service

[Service]
Type=simple
User=algo-trade
WorkingDirectory=/opt/algo-trade
ExecStart=/opt/algo-trade/algo-trade-daemon
Restart=always
RestartSec=10
Environment="RUST_LOG=info"
Environment="HYPERLIQUID_API_URL=https://api.hyperliquid.xyz"

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/algo-trade/data

[Install]
WantedBy=multi-user.target
```

### 3.2 State Persistence Options

#### Option A: SQLite for Bot Registry (Recommended)

**Crates**:
- `rusqlite` - Ergonomic SQLite bindings, most popular (GitHub: 3k stars)
- `sqlx` - Async SQL with compile-time query checking (already used in project for PostgreSQL)
- `turbosql` - High-level ORM for SQLite (simpler API, less control)

**Schema Design**:
```sql
CREATE TABLE bot_configs (
    bot_id TEXT PRIMARY KEY,
    symbol TEXT NOT NULL,
    strategy TEXT NOT NULL,
    config_json TEXT NOT NULL,  -- Full BotConfig serialized
    enabled BOOLEAN NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE bot_runtime_state (
    bot_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,  -- "Running", "Stopped", "Paused", "Error"
    started_at INTEGER,
    last_heartbeat INTEGER NOT NULL,
    error_message TEXT,
    FOREIGN KEY (bot_id) REFERENCES bot_configs(bot_id) ON DELETE CASCADE
);
```

**Pros**:
- Serverless, no external dependency
- Single file: `/opt/algo-trade/data/bots.db`
- Fast for small datasets (<10k bots)
- ACID guarantees
- Already familiar (team knows SQL)

**Cons**:
- Single-writer bottleneck (not an issue for <100 bots)
- No built-in replication (fine for single-server deployment)

#### Option B: PostgreSQL/TimescaleDB (Already Available)

**Reuse existing DatabaseClient** from `crates/data/src/database.rs`

**Pros**:
- Already in infrastructure
- Better for multi-server deployment
- JSONB columns for flexible config storage
- Powerful querying

**Cons**:
- External dependency (must be running)
- Overkill for simple bot registry
- More operational complexity

**Recommendation**: Start with SQLite (Option A), migrate to PostgreSQL if multi-server needed.

#### Persistence Strategy

**On Bot Creation**:
```rust
impl BotRegistry {
    pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
        // 1. Persist config to database FIRST
        self.db.insert_bot_config(&config).await?;

        // 2. Spawn bot actor (existing logic)
        let handle = /* ... spawn logic ... */;

        // 3. Add to in-memory registry
        self.bots.write().await.insert(bot_id, handle.clone());

        Ok(handle)
    }
}
```

**On Daemon Startup**:
```rust
impl BotRegistry {
    pub async fn new_with_persistence(db: BotDatabase) -> Result<Self> {
        let registry = Self::new();

        // Load all enabled bots from database
        let configs = db.load_enabled_bots().await?;

        for config in configs {
            // Auto-spawn each bot
            registry.spawn_bot(config).await?;
        }

        Ok(registry)
    }
}
```

**On Graceful Shutdown**:
```rust
impl BotRegistry {
    pub async fn shutdown_all(&self) -> Result<()> {
        let handles: Vec<_> = self.bots.read().await.values().cloned().collect();

        for handle in handles {
            // 1. Stop bot gracefully
            handle.shutdown().await?;

            // 2. Update state in database
            self.db.update_bot_state(&handle.bot_id, "Stopped").await?;
        }

        Ok(())
    }
}
```

### 3.3 IPC / Control Plane Options

#### Option A: REST API (Already Implemented!) ✅

**Current Implementation**: `/home/a/Work/algo-trade/crates/web-api/src/handlers.rs`

**Pros**:
- Already working
- Language-agnostic (curl, Python, any HTTP client)
- TUI can connect via `reqwest` crate
- Browser-based dashboard possible
- Well-understood, debuggable

**Cons**:
- Higher latency than Unix sockets (~1ms vs ~10µs)
- Network exposure (mitigate with firewall or localhost-only)

**Security**: Add API key authentication
```rust
// middleware/auth.rs
pub async fn auth_middleware(
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let api_key = req.headers()
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok());

    if api_key != Some(std::env::var("DAEMON_API_KEY").unwrap().as_str()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
```

#### Option B: Unix Domain Sockets (Future Enhancement)

**Crate**: `tokio::net::UnixListener` (built-in)

**Use Case**: When TUI and daemon are on same machine, want <100µs latency

**Example**:
```rust
// In daemon
let listener = UnixListener::bind("/tmp/algo-trade-daemon.sock")?;
axum::serve(listener, router).await?;

// In TUI
let stream = UnixStream::connect("/tmp/algo-trade-daemon.sock").await?;
// Use HTTP client over Unix socket
```

**Pros**:
- 10x faster than TCP
- No network exposure (filesystem permissions)

**Cons**:
- Same-machine only (no remote TUI)
- More complex client code

**Recommendation**: Stick with REST API (Option A), add Unix sockets later if needed.

#### Option C: gRPC (Overkill for this use case)

**Not Recommended**:
- Adds complexity (protobuf definitions, code generation)
- ~100µs overhead vs Unix sockets (acceptable but unnecessary)
- Better for microservices, not single-daemon control

---

## Section 4: Analysis & Synthesis

### 4.1 Architectural Recommendations

#### Recommended Architecture: "Web API Daemon with SQLite Persistence"

```
┌─────────────────────────────────────────────────────────────┐
│                     algo-trade-daemon                       │
│  (Single binary, runs as systemd service)                   │
│                                                              │
│  ┌──────────────┐        ┌─────────────────┐               │
│  │   BotRegistry │◄──────┤  BotDatabase    │               │
│  │   (in-memory)│        │  (SQLite)       │               │
│  │              │        │  bots.db        │               │
│  └──────┬───────┘        └─────────────────┘               │
│         │                                                   │
│         │ Manages                                           │
│         ▼                                                   │
│  ┌─────────────────────────────────────────┐               │
│  │  Bot Actors (tokio::spawn tasks)        │               │
│  │  ┌──────┐  ┌──────┐  ┌──────┐           │               │
│  │  │ BTC  │  │ ETH  │  │ SOL  │  ...      │               │
│  │  └──────┘  └──────┘  └──────┘           │               │
│  └─────────────────────────────────────────┘               │
│         ▲                                                   │
│         │ REST API / WebSocket                              │
│  ┌──────┴─────────────────────────────────┐                │
│  │   Axum Web Server (port 8080)          │                │
│  │   - POST /api/bots (create)            │                │
│  │   - PUT /api/bots/:id/start            │                │
│  │   - GET /ws (real-time updates)        │                │
│  └────────────────────────────────────────┘                │
└──────────────────┬───────────────────┬─────────────────────┘
                   │                   │
       ┌───────────▼──────┐   ┌────────▼─────────┐
       │   TUI Client     │   │  External Process │
       │   (ratatui)      │   │  (Python, curl)   │
       │                  │   │                   │
       │  Uses HTTP API   │   │  Uses HTTP API    │
       └──────────────────┘   └───────────────────┘
```

#### Component Breakdown

**1. Daemon Binary** (`crates/cli/src/daemon.rs` - NEW)
- Runs `Commands::Run` (already exists!)
- Enhanced with:
  - Bot auto-restore on startup
  - Graceful shutdown (SIGTERM/SIGINT)
  - SQLite persistence layer

**2. Bot Database** (`crates/bot-orchestrator/src/bot_database.rs` - NEW)
- SQLite wrapper for bot configs
- Methods: `insert_bot()`, `load_enabled_bots()`, `update_bot_state()`
- Async via `sqlx` or `tokio::task::spawn_blocking` with `rusqlite`

**3. BotRegistry Enhancement** (`crates/bot-orchestrator/src/registry.rs` - MODIFY)
- Add `db: Option<BotDatabase>` field
- Persist on `spawn_bot()`, `remove_bot()`
- Load from DB on `new_with_persistence()`

**4. TUI Client Mode** (`crates/cli/src/tui_live_bot.rs` - MODIFY)
- Replace local registry with HTTP client
- Use `reqwest` for REST API calls
- WebSocket for real-time status updates
- Fallback to local registry if daemon not running

**5. CLI Commands** (`crates/cli/src/main.rs` - MODIFY)
```rust
enum Commands {
    // New: Daemon mode (runs forever)
    Daemon {
        #[arg(short, long, default_value = "config/Config.toml")]
        config: String,

        #[arg(long)]
        state_db: Option<String>,  // Default: ./data/bots.db
    },

    // Modified: TUI connects to daemon
    LiveBotTui {
        #[arg(long, default_value = "http://localhost:8080")]
        daemon_url: String,

        #[arg(long)]
        log_file: Option<String>,
    },

    // Existing commands remain unchanged
    Run { config: String },
    Server { addr: String },
    // ...
}
```

### 4.2 Implementation Strategy

#### Phase 1: Persistence Layer (Minimal Changes)
**Goal**: Bots survive daemon restarts

1. Create `crates/bot-orchestrator/src/bot_database.rs`
   - SQLite wrapper using `sqlx` (async) or `rusqlite` (sync)
   - Schema: `bot_configs`, `bot_runtime_state`

2. Modify `BotRegistry::spawn_bot()` to persist config
3. Add `BotRegistry::restore_from_db()` for startup
4. No CLI changes yet (use existing `Commands::Run`)

**Verification**:
```bash
# Start daemon
cargo run -- run --config config/Config.toml

# Create bot via web API
curl -X POST http://localhost:8080/api/bots -d '{"bot_id":"test1","symbol":"BTC","strategy":"quad_ma"}'

# Kill daemon (Ctrl+C)
# Restart daemon
cargo run -- run --config config/Config.toml

# Verify bot auto-restored
curl http://localhost:8080/api/bots
# Should show: ["test1"]
```

#### Phase 2: Graceful Shutdown (Safety)
**Goal**: Clean shutdown on SIGTERM/SIGINT

1. Add signal handling to `run_trading_system()`
2. Call `registry.shutdown_all()` before exit
3. Update bot states in database

**Verification**:
```bash
# Start daemon
cargo run -- run

# Send SIGTERM
kill -TERM <pid>

# Check logs for:
# "Received SIGTERM, shutting down..."
# "Bot test1 shutting down"
# "All bots stopped gracefully"
```

#### Phase 3: TUI Client Mode (Optional but Valuable)
**Goal**: TUI connects to remote daemon

1. Add `--daemon-url` flag to `LiveBotTui` command
2. Replace `BotRegistry::new()` with `DaemonClient::new(url)`
3. Implement `DaemonClient` using `reqwest` + `tungstenite` (WebSocket)

**Verification**:
```bash
# Terminal 1: Start daemon
cargo run -- run

# Terminal 2: Start TUI (connects to daemon)
cargo run -- live-bot-tui --daemon-url http://localhost:8080

# Terminal 3: Create bot via curl
curl -X POST http://localhost:8080/api/bots -d '...'

# Verify: TUI shows new bot in real-time
```

#### Phase 4: Systemd Integration (Production Deployment)
**Goal**: Auto-start on boot, auto-restart on crash

1. Create `deploy/algo-trade-daemon.service`
2. Install script: `make install-service`
3. Documentation: `docs/deployment.md`

**Verification**:
```bash
sudo systemctl start algo-trade-daemon
sudo systemctl status algo-trade-daemon
sudo systemctl enable algo-trade-daemon  # Auto-start on boot

# Crash test
sudo kill -9 $(pidof algo-trade-daemon)
sleep 15
sudo systemctl status algo-trade-daemon  # Should be running again
```

### 4.3 Design Decisions

#### Decision 1: SQLite vs PostgreSQL for Bot Registry

**Choice**: SQLite
**Rationale**:
- Simpler deployment (no external dependency)
- Sufficient for <1000 bots (well beyond current scale)
- Can migrate to PostgreSQL later if multi-server needed
- Faster for single-server workload

#### Decision 2: REST API vs Unix Sockets vs gRPC

**Choice**: REST API (already implemented)
**Rationale**:
- Already working (web-api crate)
- TUI can be remote (SSH, tmux, different machine)
- Language-agnostic (Python scripts, shell scripts)
- 1ms latency acceptable for management operations
- Can add Unix sockets later for same-machine optimization

#### Decision 3: Daemon Binary vs Separate Service

**Choice**: Unified CLI with `daemon` subcommand
**Rationale**:
- Reuses existing `Commands::Run` logic (90% done!)
- Single binary to maintain
- Can still run `LiveBotTui` standalone (fallback mode)
- Familiar to users (same binary, different command)

#### Decision 4: Auto-Start Bots vs Manual Start

**Choice**: Manual start (daemon loads configs but doesn't auto-start)
**Rationale**:
- Safety: User explicitly starts bot after reviewing state
- Allows config changes before starting
- Avoids unintended trading on restart
- Can add `--auto-start` flag later

**Counter-Argument**: Could store `last_state` in DB and restore to Running if was Running before shutdown. Implement as opt-in flag.

---

## Section 5: Edge Cases & Constraints

### 5.1 Failure Scenarios

#### Edge Case 1: Daemon Crashes Mid-Trade
**Scenario**: Bot has open position, daemon crashes
**Impact**: Position left unmanaged until restart
**Mitigation**:
1. **Immediate**: systemd auto-restart (RestartSec=10s)
2. **Future**: Store open positions in database, resume monitoring on restart
3. **Safety**: Paper trading mode by default (no real money at risk during testing)

**Detection**:
```sql
-- Query to find orphaned positions
SELECT * FROM bot_runtime_state
WHERE state = 'Running'
AND last_heartbeat < datetime('now', '-5 minutes');
```

#### Edge Case 2: Database Corruption
**Scenario**: SQLite file corrupted (disk failure, power loss during write)
**Impact**: Unable to restore bots on restart
**Mitigation**:
1. **WAL mode**: Enable Write-Ahead Logging (SQLite feature for durability)
2. **Backups**: Daily cron job to copy `bots.db` to `bots.db.backup`
3. **Validation**: Health check endpoint that verifies DB integrity

**Recovery**:
```bash
# Restore from backup
cp data/bots.db.backup data/bots.db

# Or rebuild from TOML config
cargo run -- restore-from-config config/bots.toml
```

#### Edge Case 3: Port Already in Use
**Scenario**: Port 8080 occupied by another process
**Impact**: Daemon fails to start
**Mitigation**:
1. **Early check**: Bind to port before spawning bots
2. **Error message**: "Port 8080 in use, try --port 8081"
3. **systemd**: Configure port via environment variable

```rust
let listener = TcpListener::bind(addr).await
    .context(format!("Failed to bind to {addr}. Is another instance running?"))?;
```

#### Edge Case 4: Multiple Daemon Instances
**Scenario**: User accidentally starts two daemons
**Impact**: Conflicting bot management, database contention
**Mitigation**:
1. **PID file**: Create `/var/run/algo-trade-daemon.pid` on startup
2. **Exclusive lock**: SQLite `PRAGMA locking_mode=EXCLUSIVE`
3. **Health check**: New daemon detects existing instance, exits gracefully

```rust
// In daemon startup
let pid_file = "/var/run/algo-trade-daemon.pid";
if Path::new(pid_file).exists() {
    let existing_pid = fs::read_to_string(pid_file)?;
    if process_running(existing_pid) {
        anyhow::bail!("Daemon already running with PID {existing_pid}");
    }
}
fs::write(pid_file, std::process::id().to_string())?;
```

### 5.2 Security Constraints

#### Constraint 1: API Authentication
**Risk**: Unauthenticated API allows anyone to create/control bots
**Solution**: API key authentication

```rust
// In server.rs
.layer(middleware::from_fn(auth_middleware))

// In auth middleware
async fn auth_middleware(req: Request, next: Next) -> Result<Response, StatusCode> {
    let expected_key = std::env::var("DAEMON_API_KEY")
        .expect("DAEMON_API_KEY not set");

    let provided_key = req.headers()
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok());

    if provided_key != Some(&expected_key) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
```

**Deployment**:
```bash
# In systemd service
Environment="DAEMON_API_KEY=<random-64-char-string>"

# In TUI
export DAEMON_API_KEY=<same-key>
cargo run -- live-bot-tui
```

#### Constraint 2: Wallet Private Key Protection
**Risk**: Private keys exposed in database or logs
**Current**: Wallet loaded from env vars, not stored in BotConfig serialization (serde skip)
**Validation**: Already correct! ✅

```rust
// In commands.rs line 69
#[serde(skip)]
pub wallet: Option<WalletConfig>,
```

**Additional**: Ensure logs never print full config
```rust
impl fmt::Debug for BotConfig {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BotConfig")
            .field("bot_id", &self.bot_id)
            // ... other fields
            .field("wallet", &self.wallet.as_ref().map(|_| "***REDACTED***"))
            .finish()
    }
}
```

### 5.3 Resource Constraints

#### Constraint 1: Maximum Bots Per Daemon
**Limit**: 1000 concurrent bots (conservative estimate)
**Bottlenecks**:
- Tokio task limit: ~10k tasks (comfortable headroom)
- WebSocket connections: ~1k per bot (market data)
- Memory: ~10MB per bot (TradingSystem state) = 10GB for 1000 bots

**Enforcement**:
```rust
const MAX_BOTS: usize = 1000;

impl BotRegistry {
    pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
        let current_count = self.bots.read().await.len();
        if current_count >= MAX_BOTS {
            anyhow::bail!("Max bots reached ({MAX_BOTS})");
        }
        // ... spawn logic
    }
}
```

#### Constraint 2: Database Size Growth
**Growth Rate**: ~1KB per bot config + ~100 bytes per state update
**Projected**: 1MB per 1000 bots, minimal growth

**Maintenance**:
```sql
-- Cleanup old bot records (deleted bots)
DELETE FROM bot_configs WHERE updated_at < datetime('now', '-30 days');

-- Vacuum to reclaim space
VACUUM;
```

#### Constraint 3: WebSocket Connection Limits
**Per Bot**: 1 WebSocket to Hyperliquid (market data)
**Total**: 1000 bots = 1000 connections

**Hyperliquid Limit**: Unknown, likely >10k
**Mitigation**: Connection pooling (share WebSocket for same symbol)

**Future Optimization**:
```rust
// In data provider
struct SharedWebSocketPool {
    connections: HashMap<String, Arc<WebSocket>>,  // symbol -> shared connection
}
```

### 5.4 Operational Constraints

#### Constraint 1: No Hot Reload of Bot Code
**Limitation**: Updating strategy code requires daemon restart
**Workaround**:
1. Stop all bots
2. Restart daemon (picks up new binary)
3. Start bots again

**Future**: Plugin system with dynamic loading (significant effort)

#### Constraint 2: Config Changes Require Restart
**Current**: TOML config loaded once at startup
**Impact**: Changing server port, database URL requires restart

**Future Enhancement**: Config hot-reload (already designed in codebase!)
```rust
// crates/core/src/config_watcher.rs exists but unused
// Could watch config file and broadcast updates
```

#### Constraint 3: No Multi-Tenancy
**Limitation**: Single daemon serves one user/organization
**Security**: All bots share same environment (HYPERLIQUID_API_KEY)

**Future**: Add `user_id` field to BotConfig, separate credentials per user

---

## Section 6: TaskMaster Handoff Package

### MUST DO

#### 1. Create Bot Database Module
**File**: `crates/bot-orchestrator/src/bot_database.rs` (NEW)
**Purpose**: SQLite persistence for bot configs and state
**Key Functions**:
```rust
pub struct BotDatabase {
    pool: sqlx::SqlitePool,
}

impl BotDatabase {
    pub async fn new(path: &str) -> Result<Self>;
    pub async fn insert_bot(&self, config: &BotConfig) -> Result<()>;
    pub async fn load_enabled_bots(&self) -> Result<Vec<BotConfig>>;
    pub async fn update_bot_state(&self, bot_id: &str, state: &str) -> Result<()>;
    pub async fn delete_bot(&self, bot_id: &str) -> Result<()>;
}
```

**Schema** (in `migrations/001_create_bots.sql`):
```sql
CREATE TABLE IF NOT EXISTS bot_configs (
    bot_id TEXT PRIMARY KEY,
    symbol TEXT NOT NULL,
    strategy TEXT NOT NULL,
    config_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS bot_runtime_state (
    bot_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    started_at INTEGER,
    last_heartbeat INTEGER NOT NULL,
    error_message TEXT,
    FOREIGN KEY (bot_id) REFERENCES bot_configs(bot_id) ON DELETE CASCADE
);
```

**Lines**: ~200 LOC
**Dependencies**: Add `sqlx = { version = "0.7", features = ["runtime-tokio", "sqlite"] }`

#### 2. Enhance BotRegistry with Persistence
**File**: `crates/bot-orchestrator/src/registry.rs` (MODIFY)
**Line 14**: Add `db` field to `BotRegistry`
```rust
pub struct BotRegistry {
    bots: Arc<RwLock<HashMap<String, BotHandle>>>,
    db: Option<Arc<BotDatabase>>,  // NEW: Optional for backward compatibility
}
```

**Line 30**: Update `new()` to accept optional database
```rust
pub fn new() -> Self {
    Self { bots: Arc::new(RwLock::new(HashMap::new())), db: None }
}

pub fn new_with_persistence(db: Arc<BotDatabase>) -> Self {
    Self { bots: Arc::new(RwLock::new(HashMap::new())), db: Some(db) }
}
```

**Line 40**: Modify `spawn_bot()` to persist
```rust
pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
    // NEW: Persist to database FIRST
    if let Some(ref db) = self.db {
        db.insert_bot(&config).await?;
    }

    // Existing spawn logic...
    let (tx, rx) = mpsc::channel(32);
    // ... rest unchanged
}
```

**NEW METHOD** (after line 82): Add `restore_from_db()`
```rust
pub async fn restore_from_db(&self) -> Result<()> {
    let db = self.db.as_ref()
        .ok_or_else(|| anyhow::anyhow!("No database configured"))?;

    let configs = db.load_enabled_bots().await?;

    tracing::info!("Restoring {} bots from database", configs.len());

    for config in configs {
        match self.spawn_bot(config).await {
            Ok(handle) => {
                tracing::info!("Restored bot: {}", handle.latest_status().bot_id);
            }
            Err(e) => {
                tracing::error!("Failed to restore bot: {}", e);
            }
        }
    }

    Ok(())
}
```

**Lines Modified**: ~50 LOC
**New Lines**: ~30 LOC

#### 3. Add Graceful Shutdown Signal Handling
**File**: `crates/cli/src/main.rs` (MODIFY)
**Line 154**: Enhance `run_trading_system()` with signal handling
```rust
async fn run_trading_system(config_path: &str) -> anyhow::Result<()> {
    tracing::info!("Starting trading system with config: {}", config_path);

    let config = algo_trade_core::ConfigLoader::load()?;

    // NEW: Initialize database
    let db_path = std::env::var("BOT_STATE_DB")
        .unwrap_or_else(|_| "data/bots.db".to_string());
    let db = Arc::new(algo_trade_bot_orchestrator::BotDatabase::new(&db_path).await?);

    // NEW: Registry with persistence
    let registry = std::sync::Arc::new(
        algo_trade_bot_orchestrator::BotRegistry::new_with_persistence(db)
    );

    // NEW: Restore bots from database
    registry.restore_from_db().await?;

    // Start web API (in separate task)
    let server = algo_trade_web_api::ApiServer::new(registry.clone());
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let server_handle = tokio::spawn(async move {
        server.serve(&addr).await
    });

    // NEW: Wait for shutdown signal
    tokio::select! {
        result = server_handle => {
            result??;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down gracefully...");
        }
        _ = shutdown_signal() => {
            tracing::info!("Received SIGTERM, shutting down gracefully...");
        }
    }

    // NEW: Graceful shutdown
    tracing::info!("Shutting down all bots...");
    registry.shutdown_all().await?;
    tracing::info!("Shutdown complete");

    Ok(())
}

// NEW: Unix signal handler for SIGTERM
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).unwrap();
    term.recv().await;
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    std::future::pending::<()>().await;
}
```

**Lines Modified**: 154-171 (replace entirely)
**New Lines**: ~60 LOC

#### 4. Add Database to bot-orchestrator Dependencies
**File**: `crates/bot-orchestrator/Cargo.toml` (MODIFY)
**Line 10** (after existing dependencies): Add sqlx
```toml
sqlx = { version = "0.7", features = ["runtime-tokio", "sqlite", "migrate"] }
```

**Lines Modified**: 1 line added

#### 5. Export BotDatabase from bot-orchestrator
**File**: `crates/bot-orchestrator/src/lib.rs` (MODIFY)
**Line 5**: Add bot_database module
```rust
pub mod bot_database;  // NEW
```

**Line 13**: Export BotDatabase
```rust
pub use bot_database::BotDatabase;  // NEW
```

**Lines Modified**: 2 lines added

#### 6. Create SQLite Migrations Directory
**Path**: `crates/bot-orchestrator/migrations/` (NEW DIRECTORY)
**File**: `crates/bot-orchestrator/migrations/001_create_bots.sql` (NEW)
```sql
-- Bot configurations table
CREATE TABLE IF NOT EXISTS bot_configs (
    bot_id TEXT PRIMARY KEY,
    symbol TEXT NOT NULL,
    strategy TEXT NOT NULL,
    config_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

-- Bot runtime state table
CREATE TABLE IF NOT EXISTS bot_runtime_state (
    bot_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK(state IN ('Stopped', 'Running', 'Paused', 'Error')),
    started_at INTEGER,
    last_heartbeat INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    error_message TEXT,
    FOREIGN KEY (bot_id) REFERENCES bot_configs(bot_id) ON DELETE CASCADE
);

-- Index for querying enabled bots
CREATE INDEX IF NOT EXISTS idx_enabled_bots ON bot_configs(enabled, updated_at);
```

**Lines**: ~30 LOC SQL

#### 7. Update remove_bot to Persist Deletion
**File**: `crates/bot-orchestrator/src/registry.rs` (MODIFY)
**Line 97**: Add database deletion
```rust
pub async fn remove_bot(&self, bot_id: &str) -> Result<()> {
    let value = self.bots.write().await.remove(bot_id);
    if let Some(handle) = value {
        handle.shutdown().await?;

        // NEW: Remove from database
        if let Some(ref db) = self.db {
            db.delete_bot(bot_id).await?;
        }
    }
    Ok(())
}
```

**Lines Modified**: ~5 lines added

### MUST NOT DO

#### 1. Do NOT Break Existing Web API
**Reason**: TUI and external scripts depend on current endpoints
**Enforcement**: Keep all existing routes unchanged
- `POST /api/bots` signature must not change
- Response formats must not change
- Error codes must not change

**Test**: Run integration tests after changes
```bash
cargo test -p algo-trade-web-api
```

#### 2. Do NOT Auto-Start Bots on Restore
**Reason**: Safety - user should explicitly start bots after reviewing state
**Implementation**: Bots restored in `Stopped` state, user calls `/api/bots/:id/start`

**Exception**: Can add `--auto-start` flag later as opt-in feature

#### 3. Do NOT Store Wallet Private Keys in Database
**Reason**: Security - keys must come from environment variables only
**Validation**: `BotConfig::wallet` has `#[serde(skip)]` attribute (already correct)

**Audit**: Grep for any serialization of WalletConfig
```bash
grep -r "serialize.*WalletConfig" crates/
# Should return 0 results
```

#### 4. Do NOT Block Main Thread with Synchronous I/O
**Reason**: Tokio runtime requires all I/O to be async
**Enforcement**: Use `sqlx` (async) OR `tokio::task::spawn_blocking` with `rusqlite`

**Example** (if using rusqlite):
```rust
// WRONG
let result = db_connection.execute("INSERT ...");

// CORRECT
let result = tokio::task::spawn_blocking(move || {
    db_connection.execute("INSERT ...")
}).await??;
```

#### 5. Do NOT Change BotConfig Structure
**Reason**: Backward compatibility with existing TUI/API code
**Enforcement**: Only ADD optional fields, never remove or change types

**Example**:
```rust
// ALLOWED
pub struct BotConfig {
    // ... existing fields
    #[serde(default)]  // NEW optional field
    pub auto_restart: bool,
}

// NOT ALLOWED
pub struct BotConfig {
    // pub bot_id: String,  ← NEVER delete
    pub bot_id: i64,  ← NEVER change type
}
```

#### 6. Do NOT Implement Multi-Tenancy Yet
**Reason**: Out of scope for this feature, adds complexity
**Defer**: Single daemon, single user for MVP

**Future**: Add `owner_id` field to BotConfig in separate feature

#### 7. Do NOT Modify TUI in Phase 1
**Reason**: TUI can continue using local registry, daemon integration is Phase 3
**Current**: TUI creates own `BotRegistry::new()` (no database)
**Later**: TUI can connect to daemon via HTTP client

**This Keeps**: Modular architecture - TUI still works standalone

### Integration Points

#### Entry Point 1: Web API Bot Creation
**File**: `crates/web-api/src/handlers.rs`
**Line 38**: Already calls `registry.spawn_bot(config)`
**Change**: No modification needed! Persistence happens inside `spawn_bot()` ✅

#### Entry Point 2: Daemon Startup
**File**: `crates/cli/src/main.rs`
**Line 154**: `run_trading_system()` function
**Change**: Add database initialization and `restore_from_db()` call

#### Entry Point 3: Daemon Shutdown
**File**: `crates/cli/src/main.rs`
**Line 154**: `run_trading_system()` function
**Change**: Add signal handling and `shutdown_all()` call

### Dependencies to Add

**Crate**: `bot-orchestrator/Cargo.toml`
```toml
[dependencies]
sqlx = { version = "0.7", features = ["runtime-tokio", "sqlite", "migrate"] }
```

**Why sqlx over rusqlite**:
- Async-first (native Tokio integration)
- Compile-time query checking (catches SQL errors at build time)
- Migration support built-in
- Already used in project for PostgreSQL (team familiarity)

**Alternative** (if prefer sync):
```toml
rusqlite = { version = "0.31", features = ["bundled"] }
# Must wrap in spawn_blocking
```

### File Summary

**New Files** (3):
1. `crates/bot-orchestrator/src/bot_database.rs` (~200 LOC)
2. `crates/bot-orchestrator/migrations/001_create_bots.sql` (~30 LOC)
3. Optional: `deploy/algo-trade-daemon.service` (~20 LOC systemd config)

**Modified Files** (3):
1. `crates/bot-orchestrator/src/registry.rs` (+80 LOC, modify ~10 lines)
2. `crates/bot-orchestrator/src/lib.rs` (+2 LOC)
3. `crates/cli/src/main.rs` (+60 LOC, modify run_trading_system)
4. `crates/bot-orchestrator/Cargo.toml` (+1 dependency)

**Total New Code**: ~370 LOC
**Total Modified Code**: ~70 LOC

### Verification Steps

#### Step 1: Database Creation
```bash
# Start daemon
cargo run -- run --config config/Config.toml

# Verify database created
ls -lh data/bots.db
# Should exist with ~20KB initial size

# Verify schema
sqlite3 data/bots.db ".schema"
# Should show bot_configs and bot_runtime_state tables
```

#### Step 2: Bot Persistence
```bash
# Create bot via API
curl -X POST http://localhost:8080/api/bots \
  -H "Content-Type: application/json" \
  -d '{"bot_id":"test_btc","symbol":"BTC","strategy":"quad_ma"}'

# Verify persisted to database
sqlite3 data/bots.db "SELECT bot_id, symbol, strategy FROM bot_configs;"
# Should output: test_btc|BTC|quad_ma
```

#### Step 3: Auto-Restore
```bash
# Kill daemon (Ctrl+C)
# Restart daemon
cargo run -- run --config config/Config.toml

# Check logs
# Should see: "Restoring 1 bots from database"
# Should see: "Restored bot: test_btc"

# Verify via API
curl http://localhost:8080/api/bots
# Should output: ["test_btc"]
```

#### Step 4: Graceful Shutdown
```bash
# Start daemon in background
cargo run -- run &
PID=$!

# Create running bot
curl -X POST http://localhost:8080/api/bots -d '{"bot_id":"test1","symbol":"ETH","strategy":"quad_ma"}'
curl -X PUT http://localhost:8080/api/bots/test1/start

# Send SIGTERM
kill -TERM $PID

# Check logs
# Should see: "Received SIGTERM, shutting down gracefully..."
# Should see: "Shutting down all bots..."
# Should see: "Bot test1 shutting down"
# Should see: "Shutdown complete"

# Verify clean exit (no errors)
echo $?
# Should be 0
```

#### Step 5: Clippy and Build
```bash
cargo clippy -p algo-trade-bot-orchestrator -- -D warnings
cargo build --release
cargo test -p algo-trade-bot-orchestrator
```

### Migration Path for Existing Users

**Current State**: Users run TUI or `cargo run -- run`
**After Changes**: Existing workflows unchanged

**Migration**:
1. Users currently using `cargo run -- run`: Works as before, now with persistence! ✅
2. Users currently using TUI standalone: Continue working, no database needed ✅
3. Users wanting daemon + TUI: `cargo run -- run` in terminal 1, `cargo run -- live-bot-tui --daemon-url http://localhost:8080` in terminal 2 (future Phase 3)

**Backward Compatibility**: 100% - no breaking changes

---

## Section 7: Architectural Alternatives (Not Recommended)

### Alternative 1: In-App Daemonization (Using `daemonize` Crate)
**Approach**: Fork process, detach from terminal
**Why Not**:
- Tokio doesn't mix well with fork
- systemd provides better supervision
- Modern best practice: foreground process + systemd

### Alternative 2: Separate Daemon Binary
**Approach**: Create `algo-trade-daemon` separate from `algo-trade-cli`
**Why Not**:
- Duplicates code (both need BotRegistry, WebAPI)
- Confusing to users (which binary to use?)
- Existing `Commands::Run` already 90% there

### Alternative 3: Store Bot State in PostgreSQL
**Approach**: Use existing DatabaseClient
**Why Not**:
- Requires PostgreSQL running (external dependency)
- Overkill for simple key-value storage
- SQLite sufficient for single-server deployment
- Can migrate later if multi-server needed

### Alternative 4: File-Based Config (TOML) Instead of Database
**Approach**: Store bot configs in `bots/bot1.toml`, `bots/bot2.toml`
**Why Not**:
- No transactional updates (corruption risk)
- No querying (must load all files)
- No concurrent access control
- No migration path

### Alternative 5: Embedded Event Store (Event Sourcing)
**Approach**: Store all bot events, rebuild state from events
**Why Not**:
- Over-engineering for MVP
- Complex to implement correctly
- High storage costs (events grow unbounded)
- SQLite snapshot model simpler

---

## Appendix A: Example Workflows

### Workflow 1: Production Deployment with systemd

```bash
# 1. Build release binary
cargo build --release

# 2. Install binary
sudo cp target/release/algo-trade-cli /usr/local/bin/algo-trade-daemon

# 3. Create user and directories
sudo useradd -r -s /bin/false algo-trade
sudo mkdir -p /opt/algo-trade/data
sudo chown algo-trade:algo-trade /opt/algo-trade/data

# 4. Install systemd service
sudo cp deploy/algo-trade-daemon.service /etc/systemd/system/
sudo systemctl daemon-reload

# 5. Configure environment
sudo vi /etc/systemd/system/algo-trade-daemon.service.d/override.conf
# Add:
# [Service]
# Environment="HYPERLIQUID_ACCOUNT_ADDRESS=0x..."
# Environment="HYPERLIQUID_API_WALLET_KEY=0x..."
# Environment="DAEMON_API_KEY=<random-key>"

# 6. Start daemon
sudo systemctl start algo-trade-daemon

# 7. Verify
sudo systemctl status algo-trade-daemon
curl http://localhost:8080/api/bots

# 8. Enable auto-start
sudo systemctl enable algo-trade-daemon
```

### Workflow 2: Development with TUI

```bash
# Terminal 1: Start daemon
RUST_LOG=debug cargo run -- run --config config/Config.toml

# Terminal 2: Create bots via TUI (future Phase 3)
cargo run -- live-bot-tui --daemon-url http://localhost:8080

# Or use curl for now
curl -X POST http://localhost:8080/api/bots \
  -H "Content-Type: application/json" \
  -d '{
    "bot_id": "dev_btc",
    "symbol": "BTC",
    "strategy": "quad_ma",
    "execution_mode": "Paper"
  }'

curl -X PUT http://localhost:8080/api/bots/dev_btc/start
```

### Workflow 3: Remote Bot Management

```bash
# On server: Start daemon
cargo run -- run --config config/Config.toml

# On laptop: Control remotely via SSH tunnel
ssh -L 8080:localhost:8080 user@server

# Now access daemon from laptop
curl http://localhost:8080/api/bots

# Or run TUI locally, connecting to remote daemon
cargo run -- live-bot-tui --daemon-url http://localhost:8080
```

---

## Appendix B: Code Examples

### Example: BotDatabase Implementation (Full)

```rust
// crates/bot-orchestrator/src/bot_database.rs
use crate::BotConfig;
use anyhow::{Context, Result};
use sqlx::{sqlite::SqlitePool, Row};

pub struct BotDatabase {
    pool: SqlitePool,
}

impl BotDatabase {
    pub async fn new(path: &str) -> Result<Self> {
        let url = format!("sqlite://{path}");
        let pool = SqlitePool::connect(&url).await
            .context("Failed to connect to SQLite database")?;

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("Failed to run migrations")?;

        Ok(Self { pool })
    }

    pub async fn insert_bot(&self, config: &BotConfig) -> Result<()> {
        let config_json = serde_json::to_string(config)?;
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO bot_configs (bot_id, symbol, strategy, config_json, enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, 1, ?, ?)
             ON CONFLICT(bot_id) DO UPDATE SET
                config_json = excluded.config_json,
                updated_at = excluded.updated_at"
        )
        .bind(&config.bot_id)
        .bind(&config.symbol)
        .bind(&config.strategy)
        .bind(&config_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("Failed to insert bot config")?;

        Ok(())
    }

    pub async fn load_enabled_bots(&self) -> Result<Vec<BotConfig>> {
        let rows = sqlx::query(
            "SELECT config_json FROM bot_configs WHERE enabled = 1"
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to load bot configs")?;

        let mut configs = Vec::new();
        for row in rows {
            let json: String = row.get("config_json");
            let config: BotConfig = serde_json::from_str(&json)
                .context("Failed to deserialize bot config")?;
            configs.push(config);
        }

        Ok(configs)
    }

    pub async fn update_bot_state(&self, bot_id: &str, state: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO bot_runtime_state (bot_id, state, last_heartbeat)
             VALUES (?, ?, ?)
             ON CONFLICT(bot_id) DO UPDATE SET
                state = excluded.state,
                last_heartbeat = excluded.last_heartbeat"
        )
        .bind(bot_id)
        .bind(state)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("Failed to update bot state")?;

        Ok(())
    }

    pub async fn delete_bot(&self, bot_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM bot_configs WHERE bot_id = ?")
            .bind(bot_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete bot")?;

        Ok(())
    }
}
```

### Example: Systemd Service File (Full)

```ini
# deploy/algo-trade-daemon.service
[Unit]
Description=Algo Trade Bot Daemon
Documentation=https://github.com/yourorg/algo-trade
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=algo-trade
Group=algo-trade
WorkingDirectory=/opt/algo-trade
ExecStart=/usr/local/bin/algo-trade-daemon run --config /opt/algo-trade/config/Config.toml

# Restart policy
Restart=always
RestartSec=10
StartLimitInterval=200
StartLimitBurst=5

# Environment variables
Environment="RUST_LOG=info"
Environment="BOT_STATE_DB=/opt/algo-trade/data/bots.db"
EnvironmentFile=-/etc/algo-trade/daemon.env

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/algo-trade/data
ReadOnlyPaths=/opt/algo-trade/config

# Resource limits
LimitNOFILE=10000
MemoryLimit=4G

[Install]
WantedBy=multi-user.target
```

---

## Summary

**Current State**: Bots tied to TUI lifecycle, no persistence
**Desired State**: Bots run independently in daemon, TUI is control interface

**Key Insight**: Web API server already implements 90% of daemon functionality! Only missing:
1. Bot state persistence (SQLite)
2. Auto-restore on startup
3. Graceful shutdown handling

**Recommended Architecture**: "Web API Daemon with SQLite Persistence"
- Unified CLI with `daemon` subcommand (reuses existing `Commands::Run`)
- SQLite for bot config/state persistence
- REST API for control (already implemented)
- systemd for process supervision
- TUI connects as HTTP client (future Phase 3)

**Effort Estimate**: ~370 LOC new code, ~70 LOC modifications, ~3-4 days implementation

**Risk Level**: Low - changes are additive, no breaking changes to existing functionality

**Next Steps**: TaskMaster to break down into atomic tasks following Section 6 specifications.

---

**End of Context Report**
