# Playbook: Persistent Bot Daemon Architecture

**Date**: 2025-10-07
**Agent**: TaskMaster
**Source**: Context Gatherer Report (`.claude/context/2025-10-07_persistent-bot-daemon.md`)

---

## User Request

Transform the trading system so bots run persistently in a daemon process, independent of the TUI. The TUI becomes a management interface connecting to the daemon via the existing REST API.

---

## Scope Boundaries

### MUST DO

- [ ] Create SQLite-based bot persistence layer (`bot_database.rs`)
- [ ] Create database schema migrations for bot configs and runtime state
- [ ] Enhance `BotRegistry` to persist bot configurations on creation/deletion
- [ ] Add auto-restore functionality to reload bots from database on daemon startup
- [ ] Implement graceful shutdown handling (SIGTERM/SIGINT signals)
- [ ] Add sqlx dependency to bot-orchestrator crate
- [ ] Export `BotDatabase` from bot-orchestrator module
- [ ] Update `run_trading_system()` to initialize database and handle signals
- [ ] Maintain backward compatibility with existing web API endpoints

### MUST NOT DO

- ❌ Break existing Web API endpoints or response formats
- ❌ Auto-start bots on restore (safety: bots should be manually started)
- ❌ Store wallet private keys in database (must remain in env vars only)
- ❌ Block main thread with synchronous I/O (use async sqlx)
- ❌ Change `BotConfig` structure (backward compatibility)
- ❌ Implement multi-tenancy (out of scope)
- ❌ Modify TUI in this phase (Phase 3 future work)

---

## Context Summary

**Current Limitation**: Bots are spawned within the TUI event loop and die when TUI exits. BotRegistry is created in-memory without persistence.

**Good News**: The architecture is already well-positioned:
- `Commands::Run` already starts a long-running web API server (90% of daemon functionality!)
- BotRegistry uses `Arc<RwLock<HashMap>>` (thread-safe, shared ownership)
- Bots run as independent Tokio tasks (not tied to TUI threads)
- BotConfig is already `Serialize + Deserialize` ready
- Web API has all necessary control endpoints

**Missing Pieces**:
1. Bot state persistence (SQLite database)
2. Auto-restore on daemon startup
3. Graceful shutdown signal handling

**Architecture Decision**: Use SQLite for simplicity (no external dependency), REST API for control (already implemented), systemd for process supervision.

**Total Effort**: ~370 LOC new code, ~70 LOC modifications

---

## Atomic Tasks

### Phase 1: Database Foundation (Core Persistence Layer)

#### Task 1.1: Add sqlx Dependency

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/Cargo.toml`
**Location**: Dependencies section (after existing dependencies)
**Action**: Add sqlx with SQLite support
**Code**:
```toml
sqlx = { version = "0.7", features = ["runtime-tokio", "sqlite", "migrate"] }
chrono = "0.4"
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Dependency resolves without conflicts
**Estimated LOC**: 2

---

#### Task 1.2: Create Database Schema Migration

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/migrations/001_create_bots.sql` (CREATE NEW)
**Location**: New file in new directory
**Action**: Create SQLite schema for bot persistence
**Code**:
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
**Verification**:
```bash
# Schema will be verified when database module runs migrations
ls crates/bot-orchestrator/migrations/001_create_bots.sql
```
**Acceptance**: File exists and contains valid SQL
**Estimated LOC**: 25

---

#### Task 1.3: Create BotDatabase Module - Struct and Constructor

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_database.rs` (CREATE NEW)
**Location**: New file
**Action**: Create database wrapper with connection pooling
**Code**:
```rust
use crate::BotConfig;
use anyhow::{Context, Result};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

/// SQLite database for bot configuration and runtime state persistence
pub struct BotDatabase {
    pool: SqlitePool,
}

impl BotDatabase {
    /// Create a new database connection and run migrations
    ///
    /// # Arguments
    /// * `path` - Path to SQLite database file (e.g., "data/bots.db")
    ///
    /// # Errors
    /// Returns error if database connection fails or migrations fail
    pub async fn new(path: &str) -> Result<Self> {
        let url = format!("sqlite://{path}");
        let pool = SqlitePool::connect(&url)
            .await
            .context("Failed to connect to SQLite database")?;

        // Run migrations from migrations/ directory
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("Failed to run database migrations")?;

        tracing::info!("Database initialized at: {}", path);

        Ok(Self { pool })
    }
}
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 35

---

#### Task 1.4: Add BotDatabase insert_bot Method

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_database.rs`
**Location**: After the `new()` method in impl BotDatabase block
**Action**: Add method to persist bot configuration
**Code**:
```rust
    /// Insert or update bot configuration in database
    ///
    /// # Arguments
    /// * `config` - Bot configuration to persist
    ///
    /// # Errors
    /// Returns error if database write fails or serialization fails
    pub async fn insert_bot(&self, config: &BotConfig) -> Result<()> {
        let config_json = serde_json::to_string(config)
            .context("Failed to serialize bot config")?;
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

        tracing::debug!("Persisted bot config: {}", config.bot_id);

        Ok(())
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 30

---

#### Task 1.5: Add BotDatabase load_enabled_bots Method

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_database.rs`
**Location**: After the `insert_bot()` method
**Action**: Add method to load bot configurations from database
**Code**:
```rust
    /// Load all enabled bot configurations from database
    ///
    /// # Errors
    /// Returns error if database read fails or deserialization fails
    pub async fn load_enabled_bots(&self) -> Result<Vec<BotConfig>> {
        let rows = sqlx::query(
            "SELECT config_json FROM bot_configs WHERE enabled = 1 ORDER BY created_at ASC"
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to load bot configs from database")?;

        let mut configs = Vec::new();
        for row in rows {
            let json: String = row.get("config_json");
            let config: BotConfig = serde_json::from_str(&json)
                .context("Failed to deserialize bot config")?;
            configs.push(config);
        }

        tracing::info!("Loaded {} enabled bots from database", configs.len());

        Ok(configs)
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 25

---

#### Task 1.6: Add BotDatabase update_bot_state Method

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_database.rs`
**Location**: After the `load_enabled_bots()` method
**Action**: Add method to update bot runtime state
**Code**:
```rust
    /// Update bot runtime state in database
    ///
    /// # Arguments
    /// * `bot_id` - Bot identifier
    /// * `state` - Runtime state (Stopped, Running, Paused, Error)
    ///
    /// # Errors
    /// Returns error if database write fails
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

        tracing::debug!("Updated bot state: {} -> {}", bot_id, state);

        Ok(())
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 28

---

#### Task 1.7: Add BotDatabase delete_bot Method

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_database.rs`
**Location**: After the `update_bot_state()` method
**Action**: Add method to delete bot configuration
**Code**:
```rust
    /// Delete bot configuration from database
    ///
    /// # Arguments
    /// * `bot_id` - Bot identifier to delete
    ///
    /// # Errors
    /// Returns error if database write fails
    pub async fn delete_bot(&self, bot_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM bot_configs WHERE bot_id = ?")
            .bind(bot_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete bot from database")?;

        tracing::info!("Deleted bot from database: {}", bot_id);

        Ok(())
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings, full module compiles
**Estimated LOC**: 18

---

#### Task 1.8: Export BotDatabase from bot-orchestrator Module

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs`
**Location**: After existing module declarations (around line 5)
**Action**: Add bot_database module declaration and export
**Code**:
```rust
pub mod bot_database;
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Module compiles, no warnings
**Estimated LOC**: 1

---

#### Task 1.9: Export BotDatabase Type from bot-orchestrator

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs`
**Location**: After existing pub use statements (around line 13)
**Action**: Export BotDatabase type
**Code**:
```rust
pub use bot_database::BotDatabase;
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Type is publicly accessible
**Estimated LOC**: 1

---

### Phase 2: Registry Persistence Integration

#### Task 2.1: Add Database Field to BotRegistry

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: Line 14-17 (BotRegistry struct definition)
**Action**: Add optional database field for persistence
**Old Code**:
```rust
pub struct BotRegistry {
    bots: Arc<RwLock<HashMap<String, BotHandle>>>,
}
```
**New Code**:
```rust
pub struct BotRegistry {
    bots: Arc<RwLock<HashMap<String, BotHandle>>>,
    db: Option<Arc<crate::BotDatabase>>,
}
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles (will show errors about constructor, fixed in next task)
**Estimated LOC**: 1

---

#### Task 2.2: Update BotRegistry::new Constructor

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: Around line 30 (new() method)
**Action**: Update constructor to initialize db field as None
**Old Code**:
```rust
    pub fn new() -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
        }
    }
```
**New Code**:
```rust
    pub fn new() -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without errors
**Estimated LOC**: 1

---

#### Task 2.3: Add BotRegistry::new_with_persistence Constructor

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: After the `new()` method (around line 36)
**Action**: Add constructor that accepts database for persistence
**Code**:
```rust
    /// Create a new registry with database persistence
    ///
    /// # Arguments
    /// * `db` - Database for bot configuration persistence
    pub fn new_with_persistence(db: Arc<crate::BotDatabase>) -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
            db: Some(db),
        }
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 11

---

#### Task 2.4: Enhance spawn_bot to Persist Configuration

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: Line 40-82 (spawn_bot method, at the start of method body)
**Action**: Add database persistence before spawning bot
**Old Code** (first few lines of method):
```rust
    pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
        let bot_id = config.bot_id.clone();
```
**New Code**:
```rust
    pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
        let bot_id = config.bot_id.clone();

        // Persist to database BEFORE spawning (crash safety)
        if let Some(ref db) = self.db {
            db.insert_bot(&config).await?;
        }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 5

---

#### Task 2.5: Add restore_from_db Method to BotRegistry

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: After spawn_bot method (around line 85)
**Action**: Add method to restore bots from database on startup
**Code**:
```rust
    /// Restore all enabled bots from database
    ///
    /// This is called on daemon startup to reload bots that were
    /// running before shutdown. Bots are restored in 'Stopped' state
    /// and must be manually started for safety.
    ///
    /// # Errors
    /// Returns error if database read fails or bot spawn fails
    pub async fn restore_from_db(&self) -> Result<()> {
        let db = self.db.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database configured for persistence"))?;

        let configs = db.load_enabled_bots().await?;

        tracing::info!("Restoring {} bots from database", configs.len());

        for config in configs {
            let bot_id = config.bot_id.clone();
            match self.spawn_bot(config).await {
                Ok(handle) => {
                    let status = handle.latest_status();
                    tracing::info!("Restored bot: {} ({})", bot_id, status.state);
                }
                Err(e) => {
                    tracing::error!("Failed to restore bot {}: {}", bot_id, e);
                    // Continue restoring other bots even if one fails
                }
            }
        }

        Ok(())
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 30

---

#### Task 2.6: Add shutdown_all Method to BotRegistry

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: After restore_from_db method
**Action**: Add method to gracefully shutdown all bots
**Code**:
```rust
    /// Shutdown all bots gracefully
    ///
    /// This is called on daemon shutdown (SIGTERM/SIGINT) to ensure
    /// all bots stop cleanly and update their state in the database.
    ///
    /// # Errors
    /// Returns error if bot shutdown fails or database update fails
    pub async fn shutdown_all(&self) -> Result<()> {
        let handles: Vec<_> = self.bots.read().await.values().cloned().collect();

        tracing::info!("Shutting down {} bots", handles.len());

        for handle in handles {
            let bot_id = handle.latest_status().bot_id.clone();

            match handle.shutdown().await {
                Ok(()) => {
                    tracing::info!("Bot {} shut down successfully", bot_id);

                    // Update state in database
                    if let Some(ref db) = self.db {
                        if let Err(e) = db.update_bot_state(&bot_id, "Stopped").await {
                            tracing::error!("Failed to update bot state in database: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to shutdown bot {}: {}", bot_id, e);
                }
            }
        }

        tracing::info!("All bots shutdown complete");

        Ok(())
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 36

---

#### Task 2.7: Update remove_bot to Persist Deletion

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: Line 97-105 (remove_bot method body)
**Action**: Add database deletion after removing bot
**Old Code**:
```rust
    pub async fn remove_bot(&self, bot_id: &str) -> Result<()> {
        let value = self.bots.write().await.remove(bot_id);
        if let Some(handle) = value {
            handle.shutdown().await?;
        }
        Ok(())
    }
```
**New Code**:
```rust
    pub async fn remove_bot(&self, bot_id: &str) -> Result<()> {
        let value = self.bots.write().await.remove(bot_id);
        if let Some(handle) = value {
            handle.shutdown().await?;

            // Remove from database
            if let Some(ref db) = self.db {
                db.delete_bot(bot_id).await?;
            }
        }
        Ok(())
    }
```
**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 5

---

### Phase 3: Daemon Signal Handling and Startup

#### Task 3.1: Add Signal Handling Helper Function

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: After the main function (around line 175)
**Action**: Add Unix signal handler for SIGTERM
**Code**:
```rust
/// Wait for SIGTERM signal (Unix only)
///
/// This is used for graceful shutdown when daemon receives
/// systemd stop command or kill -TERM signal.
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate())
        .expect("Failed to register SIGTERM handler");
    term.recv().await;
}

/// Placeholder for non-Unix platforms (Windows)
#[cfg(not(unix))]
async fn shutdown_signal() {
    std::future::pending::<()>().await;
}
```
**Verification**:
```bash
cargo check -p algo-trade-cli
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 18

---

#### Task 3.2: Update run_trading_system - Add Database Initialization

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: Line 154-171 (run_trading_system function body, after config load)
**Action**: Initialize database and create registry with persistence
**Old Code**:
```rust
async fn run_trading_system(config_path: &str) -> anyhow::Result<()> {
    tracing::info!("Starting trading system with config: {}", config_path);

    let config = algo_trade_core::ConfigLoader::load()?;
    let registry = std::sync::Arc::new(algo_trade_bot_orchestrator::BotRegistry::new());
```
**New Code**:
```rust
async fn run_trading_system(config_path: &str) -> anyhow::Result<()> {
    tracing::info!("Starting trading system with config: {}", config_path);

    let config = algo_trade_core::ConfigLoader::load()?;

    // Initialize bot state database
    let db_path = std::env::var("BOT_STATE_DB")
        .unwrap_or_else(|_| "data/bots.db".to_string());

    tracing::info!("Initializing bot database at: {}", db_path);
    let db = std::sync::Arc::new(
        algo_trade_bot_orchestrator::BotDatabase::new(&db_path).await?
    );

    // Create registry with persistence
    let registry = std::sync::Arc::new(
        algo_trade_bot_orchestrator::BotRegistry::new_with_persistence(db)
    );

    // Restore bots from database
    tracing::info!("Restoring bots from database");
    registry.restore_from_db().await?;
```
**Verification**:
```bash
cargo check -p algo-trade-cli
```
**Acceptance**: Compiles without warnings
**Estimated LOC**: 15

---

#### Task 3.3: Update run_trading_system - Spawn Server in Background

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: Line 165-171 (run_trading_system function, after registry creation)
**Action**: Spawn web API server as background task instead of blocking
**Old Code**:
```rust
    let server = algo_trade_web_api::ApiServer::new(registry.clone());
    let addr = format!("{}:{}", config.server.host, config.server.port);
    server.serve(&addr).await?;
    Ok(())
}
```
**New Code**:
```rust
    // Start web API server in background task
    let server = algo_trade_web_api::ApiServer::new(registry.clone());
    let addr = format!("{}:{}", config.server.host, config.server.port);

    tracing::info!("Starting web API server on {}", addr);
    let server_handle = tokio::spawn(async move {
        server.serve(&addr).await
    });

    // Wait for shutdown signal or server error
    tokio::select! {
        result = server_handle => {
            match result {
                Ok(Ok(())) => {
                    tracing::info!("Web API server stopped normally");
                }
                Ok(Err(e)) => {
                    tracing::error!("Web API server error: {}", e);
                    return Err(e);
                }
                Err(e) => {
                    tracing::error!("Server task panicked: {}", e);
                    return Err(anyhow::anyhow!("Server task panicked: {}", e));
                }
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT (Ctrl+C), shutting down gracefully...");
        }
        _ = shutdown_signal() => {
            tracing::info!("Received SIGTERM, shutting down gracefully...");
        }
    }

    // Graceful shutdown sequence
    tracing::info!("Shutting down all bots...");
    registry.shutdown_all().await?;
    tracing::info!("Shutdown complete");

    Ok(())
}
```
**Verification**:
```bash
cargo check -p algo-trade-cli
```
**Acceptance**: Compiles without warnings, function handles all shutdown paths
**Estimated LOC**: 35

---

### Phase 4: Optional - Systemd Service File

#### Task 4.1: Create Systemd Service File

**File**: `/home/a/Work/algo-trade/deploy/algo-trade-daemon.service` (CREATE NEW)
**Location**: New file in new directory
**Action**: Create systemd service configuration
**Code**:
```ini
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
**Verification**:
```bash
ls deploy/algo-trade-daemon.service
systemd-analyze verify deploy/algo-trade-daemon.service 2>/dev/null || echo "Validation skipped (systemd not available)"
```
**Acceptance**: File exists and is valid systemd format
**Estimated LOC**: 38

---

#### Task 4.2: Create Deployment README

**File**: `/home/a/Work/algo-trade/deploy/README.md` (CREATE NEW)
**Location**: New file
**Action**: Document deployment process
**Code**:
```markdown
# Deployment Guide

## Installing as systemd Service

### 1. Build Release Binary

```bash
cargo build --release
```

### 2. Install Binary

```bash
sudo cp target/release/algo-trade-cli /usr/local/bin/algo-trade-daemon
sudo chmod +x /usr/local/bin/algo-trade-daemon
```

### 3. Create User and Directories

```bash
sudo useradd -r -s /bin/false algo-trade
sudo mkdir -p /opt/algo-trade/{config,data}
sudo chown -R algo-trade:algo-trade /opt/algo-trade
```

### 4. Copy Configuration

```bash
sudo cp config/Config.toml /opt/algo-trade/config/
sudo chown algo-trade:algo-trade /opt/algo-trade/config/Config.toml
```

### 5. Create Environment File

```bash
sudo mkdir -p /etc/algo-trade
sudo vi /etc/algo-trade/daemon.env
```

Add environment variables:
```bash
HYPERLIQUID_ACCOUNT_ADDRESS=0x...
HYPERLIQUID_API_WALLET_KEY=0x...
DAEMON_API_KEY=<generate-random-key>
```

```bash
sudo chmod 600 /etc/algo-trade/daemon.env
sudo chown algo-trade:algo-trade /etc/algo-trade/daemon.env
```

### 6. Install systemd Service

```bash
sudo cp deploy/algo-trade-daemon.service /etc/systemd/system/
sudo systemctl daemon-reload
```

### 7. Start and Enable Service

```bash
# Start daemon
sudo systemctl start algo-trade-daemon

# Check status
sudo systemctl status algo-trade-daemon

# Enable auto-start on boot
sudo systemctl enable algo-trade-daemon
```

### 8. View Logs

```bash
sudo journalctl -u algo-trade-daemon -f
```

## Managing Bots

### Via curl

```bash
# List bots
curl http://localhost:8080/api/bots

# Create bot
curl -X POST http://localhost:8080/api/bots \
  -H "Content-Type: application/json" \
  -d '{
    "bot_id": "btc_trader",
    "symbol": "BTC",
    "strategy": "quad_ma",
    "execution_mode": "Paper"
  }'

# Start bot
curl -X PUT http://localhost:8080/api/bots/btc_trader/start

# Stop bot
curl -X PUT http://localhost:8080/api/bots/btc_trader/stop

# Delete bot
curl -X DELETE http://localhost:8080/api/bots/btc_trader
```

## Troubleshooting

### Check if daemon is running

```bash
sudo systemctl status algo-trade-daemon
```

### View recent logs

```bash
sudo journalctl -u algo-trade-daemon -n 50
```

### Restart daemon

```bash
sudo systemctl restart algo-trade-daemon
```

### Check database

```bash
sudo sqlite3 /opt/algo-trade/data/bots.db ".schema"
sudo sqlite3 /opt/algo-trade/data/bots.db "SELECT * FROM bot_configs;"
```
```
**Verification**:
```bash
ls deploy/README.md
```
**Acceptance**: File exists and is readable
**Estimated LOC**: 110

---

## Task Dependencies

### Execution Order

```
Phase 1: Database Foundation (Parallel Execution Possible)
├── Task 1.1 → Task 1.2 → Task 1.3 → Task 1.4 → Task 1.5 → Task 1.6 → Task 1.7 → Task 1.8 → Task 1.9
└── All must complete before Phase 2

Phase 2: Registry Persistence Integration (Sequential Execution)
├── Task 2.1 → Task 2.2 → Task 2.3 (must complete in order)
├── Task 2.4 (depends on Task 2.1-2.3)
├── Task 2.5 (depends on Task 2.1-2.4)
├── Task 2.6 (depends on Task 2.1-2.3)
└── Task 2.7 (depends on Task 2.1-2.3)

Phase 3: Daemon Signal Handling (Sequential Execution)
├── Task 3.1 (independent)
├── Task 3.2 (depends on Phase 2 complete)
└── Task 3.3 (depends on Task 3.1, 3.2)

Phase 4: Optional Deployment Files (Parallel Execution)
├── Task 4.1 (independent, documentation)
└── Task 4.2 (independent, documentation)
```

### Critical Path

1. Phase 1 Database Foundation (Tasks 1.1-1.9)
2. Phase 2 Registry Integration (Tasks 2.1-2.7)
3. Phase 3 Daemon Startup (Tasks 3.1-3.3)
4. Phase 4 Optional (can be done anytime)

**Total Sequential Time**: ~3-4 hours implementation + 1 hour testing
**Parallelizable**: Phase 1 tasks can be batched, Phase 4 can run anytime

---

## Verification Checklist

### Per-Phase Verification

#### Phase 1: Database Foundation
```bash
cargo check -p algo-trade-bot-orchestrator
cargo clippy -p algo-trade-bot-orchestrator -- -D warnings
```

#### Phase 2: Registry Integration
```bash
cargo check -p algo-trade-bot-orchestrator
cargo clippy -p algo-trade-bot-orchestrator -- -D warnings
cargo test -p algo-trade-bot-orchestrator
```

#### Phase 3: Daemon Startup
```bash
cargo check -p algo-trade-cli
cargo clippy -p algo-trade-cli -- -D warnings
cargo build --release
```

### Integration Tests

#### Test 1: Database Creation
```bash
# Start daemon
cargo run -- run --config config/Config.toml

# Verify database exists
ls -lh data/bots.db

# Verify schema
sqlite3 data/bots.db ".schema"
# Expected: bot_configs and bot_runtime_state tables
```

#### Test 2: Bot Persistence
```bash
# Create bot
curl -X POST http://localhost:8080/api/bots \
  -H "Content-Type: application/json" \
  -d '{"bot_id":"test1","symbol":"BTC","strategy":"quad_ma","execution_mode":"Paper"}'

# Verify in database
sqlite3 data/bots.db "SELECT bot_id, symbol FROM bot_configs;"
# Expected: test1|BTC
```

#### Test 3: Auto-Restore
```bash
# Kill daemon (Ctrl+C)
# Restart daemon
cargo run -- run --config config/Config.toml

# Check logs for:
# "Restoring 1 bots from database"
# "Restored bot: test1"

# Verify via API
curl http://localhost:8080/api/bots
# Expected: ["test1"]
```

#### Test 4: Graceful Shutdown
```bash
# Start daemon in background
cargo run -- run &
PID=$!

# Create and start bot
curl -X POST http://localhost:8080/api/bots -d '{"bot_id":"test2","symbol":"ETH","strategy":"quad_ma"}'
curl -X PUT http://localhost:8080/api/bots/test2/start

# Send SIGTERM
kill -TERM $PID

# Check logs for:
# "Received SIGTERM, shutting down gracefully..."
# "Shutting down all bots..."
# "Shutdown complete"
```

### Karen Quality Gate Requirements

**After Each Phase**:
```bash
# Phase 0: Compilation
cargo build --package algo-trade-bot-orchestrator --lib
cargo build --package algo-trade-cli

# Phase 1: Clippy (all levels)
cargo clippy -p algo-trade-bot-orchestrator -- -D warnings
cargo clippy -p algo-trade-bot-orchestrator -- -W clippy::pedantic
cargo clippy -p algo-trade-bot-orchestrator -- -W clippy::nursery
cargo clippy -p algo-trade-cli -- -D warnings

# Phase 2: rust-analyzer diagnostics
# (Run in IDE, verify zero issues)

# Phase 3: Cross-file validation
cargo check --workspace

# Phase 4: Per-file verification
# Each modified file must pass individual check

# Phase 5: Release build
cargo build --release

# Phase 6: Tests compile
cargo test --no-run
```

---

## Estimated Timeline

### Development Time

| Phase | Tasks | LOC | Estimated Time |
|-------|-------|-----|----------------|
| Phase 1: Database Foundation | 1.1-1.9 | ~160 | 2 hours |
| Phase 2: Registry Integration | 2.1-2.7 | ~90 | 1.5 hours |
| Phase 3: Daemon Startup | 3.1-3.3 | ~70 | 1 hour |
| Phase 4: Deployment (Optional) | 4.1-4.2 | ~150 | 0.5 hours |
| **Total** | **21 tasks** | **~470 LOC** | **5 hours** |

### Testing Time

| Test Type | Estimated Time |
|-----------|----------------|
| Unit tests | 0.5 hours |
| Integration tests | 1 hour |
| Manual verification | 0.5 hours |
| Karen review | 1 hour |
| **Total** | **3 hours** |

### Overall Estimate

**Implementation**: 5 hours
**Testing**: 3 hours
**Total**: **8 hours** (1 development day)

---

## Risk Assessment

### Low Risk
- All changes are additive (no breaking changes)
- Backward compatible (existing TUI/API continue working)
- Database is optional (BotRegistry works without it)
- Graceful degradation (if DB fails, bots still work in-memory)

### Mitigation Strategies

1. **Database Corruption**: Use SQLite WAL mode, daily backups
2. **Migration Failures**: Test migrations on sample database first
3. **Signal Handling Issues**: Extensive testing on Unix/Linux platforms
4. **Memory Leaks**: Monitor with `valgrind` or `heaptrack` in testing

### Rollback Plan

If issues occur:
1. Revert changes to `run_trading_system()` (Phase 3)
2. Keep database module (Phase 1) but don't use it
3. Registry works without database (backward compatible)
4. No data loss (bots.db file preserved)

---

## Success Criteria

### Must Have (Blocking)
- ✅ Daemon starts and runs indefinitely
- ✅ Bots persist to SQLite database
- ✅ Bots auto-restore on daemon restart
- ✅ Graceful shutdown on SIGTERM/SIGINT
- ✅ All existing API endpoints work unchanged
- ✅ Zero clippy warnings
- ✅ Zero compilation warnings

### Should Have (Important)
- ✅ Integration tests pass
- ✅ systemd service file works
- ✅ Deployment documentation complete
- ✅ Database schema is correct
- ✅ No wallet keys in database

### Nice to Have (Future)
- ⏭️ TUI connects to daemon (Phase 3 future work)
- ⏭️ Unix socket support (performance optimization)
- ⏭️ API key authentication (security enhancement)
- ⏭️ Auto-start bots on restore (opt-in flag)

---

## Notes

### Architectural Decisions

1. **SQLite over PostgreSQL**: Simpler deployment, no external dependency, sufficient for <1000 bots
2. **REST API over Unix Sockets**: Already implemented, remote access possible, language-agnostic
3. **Manual start over Auto-start**: Safety first - user explicitly starts bots after reviewing state
4. **Unified CLI over Separate Daemon**: Reuses 90% of existing code, single binary to maintain

### Future Enhancements

**Phase 5: TUI Client Mode** (Future)
- Replace TUI's local registry with HTTP client to daemon
- Use WebSocket for real-time bot status updates
- Fallback to local registry if daemon unavailable

**Phase 6: Security Hardening** (Future)
- API key authentication middleware
- Rate limiting on API endpoints
- TLS/HTTPS support for remote access

**Phase 7: High Availability** (Future)
- Migrate from SQLite to PostgreSQL for multi-server support
- Leader election for active daemon
- WebSocket connection pooling per symbol

### Constraints

- **No breaking changes**: All existing code must continue working
- **Backward compatible**: Old configs, API responses unchanged
- **Security**: No wallet keys in database (already enforced by `#[serde(skip)]`)
- **Performance**: Async I/O only (sqlx, no blocking operations)

---

**End of Playbook**

Ready for execution. Proceed with Phase 1, Task 1.1.
