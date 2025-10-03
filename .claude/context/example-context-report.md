# Context Report: Multi-Account Support

**Date**: 2025-10-02
**Agent**: Context Gatherer
**Status**: ✅ Complete (Example Template)
**TaskMaster Handoff**: ✅ Ready

---

## Section 1: Request Analysis

### User Request (Verbatim)
"Add support for multiple Hyperliquid accounts so I can run different strategies on different wallets"

### Explicit Requirements
1. Multiple Hyperliquid accounts supported simultaneously
2. Different strategies can run on different accounts
3. Each account maps to different wallet address

### Implicit Requirements
1. **Account Credentials Storage**: Secure storage of multiple API key/secret pairs
2. **Bot-to-Account Mapping**: Configuration to specify which bot uses which account
3. **Concurrent WebSocket Connections**: Each account needs independent WebSocket connection
4. **Isolated Position Tracking**: Positions tracked separately per account (no cross-account netting)
5. **Configuration Schema Changes**: Config.toml must support array of accounts
6. **CLI Account Management**: Commands to list/add/remove accounts
7. **Rate Limit Coordination**: Hyperliquid limits per-IP, not per-account (must share limiter)
8. **Backward Compatibility**: Existing single-account configs should migrate seamlessly

### Open Questions
1. **Scalability**: How many concurrent accounts? (2? 10? 100?)
2. **Data Sharing**: Should strategies share market data across accounts, or isolated feeds?
3. **Failover**: If one account's credentials fail, should bot switch to backup account?
4. **Rate Limit Fairness**: Should accounts have equal quota, or priority-based?

### Success Criteria
- [ ] User can define N accounts in `config/Config.toml` using TOML array syntax
- [ ] Each bot specifies `account_id` in configuration
- [ ] Position tracking isolated per account (verified in database queries)
- [ ] WebSocket connections managed independently per account
- [ ] Single-account configs migrate automatically without user intervention
- [ ] Rate limiting prevents IP bans when multiple accounts active

---

## Section 2: Codebase Context

### Existing Architecture

**Configuration System** (`crates/core/src/config.rs`):
- **Lines 15-30**: `HyperliquidConfig` struct currently has single account fields:
  ```rust
  pub struct HyperliquidConfig {
      pub api_key: String,
      pub api_secret: String,
      pub testnet: bool,
  }
  ```
- **Line 45**: Uses `Figment` for multi-source loading (TOML → env vars → CLI args)
- **Pattern**: Nested structs with `#[serde(default)]` for optional fields

**WebSocket Management** (`crates/exchange-hyperliquid/src/websocket.rs`):
- **Lines 20-35**: `HyperliquidWebSocket` struct owns single connection:
  ```rust
  pub struct HyperliquidWebSocket {
      url: String,
      tx: mpsc::Sender<Message>,
      rx: broadcast::Receiver<MarketEvent>,
  }
  ```
- **Line 50**: Auto-reconnect logic with exponential backoff (tokio::time::sleep)
- **Pattern**: Spawns Tokio task in constructor, returns handle

**Bot Orchestrator** (`crates/bot-orchestrator/src/bot_actor.rs`):
- **Line 25**: `BotActor` owns single `LiveDataProvider` (assumes one account):
  ```rust
  pub struct BotActor {
      bot_id: String,
      data_provider: LiveDataProvider,  // Single account!
      execution_handler: LiveExecutionHandler,
      strategies: Vec<Arc<Mutex<dyn Strategy>>>,
  }
  ```
- **Line 40**: No concept of account isolation

**Rate Limiting** (`crates/exchange-hyperliquid/src/client.rs`):
- **Line 30**: Each `HyperliquidClient` creates its own `Governor` rate limiter:
  ```rust
  let limiter = RateLimiter::direct(Quota::per_minute(nonzero!(1200u32)));
  ```
- **Problem**: Multiple accounts would create multiple limiters → IP ban risk

### Current Patterns

1. **Single Account Assumption**: All code assumes one Hyperliquid account globally
2. **Configuration Pattern**: Nested structs, `#[serde(default)]` for backward compat
3. **Error Handling**: `anyhow::Result` with `.context()` for error chains
4. **Async Pattern**: Tokio tasks with mpsc for commands, broadcast for events
5. **Financial Precision**: `rust_decimal::Decimal` for all prices/quantities

### Integration Points

Files requiring modification (with exact line numbers):

1. **`crates/core/src/config.rs:15-30`**
   - Change `HyperliquidConfig` from single account to `accounts: Vec<AccountConfig>`
   - Add optional `default_account: Option<String>` for backward compat

2. **`crates/exchange-hyperliquid/src/client.rs:20`**
   - Constructor must accept `account_id: &str` parameter
   - Accept shared `Arc<RateLimiter>` instead of creating new one

3. **`crates/bot-orchestrator/src/bot_actor.rs:25`**
   - Add `account_id: String` field to struct
   - Pass to data provider/execution handler constructors

4. **`config/Config.toml:5-10`**
   - Migrate from single account to TOML array: `[[hyperliquid.accounts]]`

5. **`crates/web-api/src/handlers.rs:45`**
   - `create_bot` handler must accept `account_id` in request body
   - Validate account_id exists before creating bot

### Constraints

**MUST Preserve**:
- ✅ Public API of `DataProvider` trait (crates/core/src/traits.rs:15)
- ✅ Public API of `ExecutionHandler` trait (crates/core/src/traits.rs:25)
- ✅ Backward compatibility with existing single-account configs
- ✅ `rust_decimal::Decimal` for all financial values

**CANNOT Break**:
- ❌ Existing bots running in production (migration must be seamless)
- ❌ Database schema for `positions` table (already has `account_id` column from Phase 5)
- ❌ WebSocket message format (Hyperliquid API contract)

---

## Section 3: External Research

### Crate Evaluation

| Crate | Version | Purpose | Pros | Cons | Decision |
|-------|---------|---------|------|------|----------|
| `secrecy` | 0.8.0 | Prevent accidental secret logging | Zeroizes memory on drop, serde support | Extra wrapper type | ✅ **Use** |
| `keyring` | 2.0.0 | OS-level keychain storage | Secure credential storage | Platform-specific, adds complexity | ❌ **Skip** (overkill) |
| `config` | 0.13.0 | Alternative to Figment | More features | We already use Figment | ❌ **Skip** (no migration) |

**Decision Rationale**:
- `secrecy`: Critical for preventing API secrets from appearing in logs (Rust's `Debug` trait auto-impl would expose secrets)
- `keyring`: Rejected due to platform-specific complexity; TOML with file permissions sufficient for initial implementation

### API Documentation Analysis

**Source**: [Hyperliquid API Docs](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api)

**Key Findings**:

1. **Rate Limiting** (per-IP, not per-account):
   - REST API: 1200 requests/minute per IP address
   - WebSocket: No documented connection limit, but recommend max 10 concurrent
   - **Implication**: MUST share single `RateLimiter` across all accounts

2. **Authentication**:
   - Each account requires separate `api_key` and `api_secret`
   - Signature computed per-request using account's secret
   - No concept of "sub-accounts" (each is fully independent)

3. **WebSocket Subscriptions**:
   - Can subscribe to multiple symbols per connection
   - No cross-account data (each connection sees only its account's data)
   - **Implication**: Each account needs dedicated WebSocket connection

4. **Position Tracking**:
   - Positions isolated per account (no cross-margining)
   - Each account has independent margin balance
   - **Implication**: Position tracking must be account-scoped

### Design Pattern Research

**Source 1**: [Alice Ryhl - Actors with Tokio](https://ryhl.io/blog/actors-with-tokio/)

**Pattern**: DIY Actor Pattern
```rust
pub struct ActorHandle {
    tx: mpsc::Sender<Command>,
}

async fn actor_task(mut rx: mpsc::Receiver<Command>) {
    while let Some(cmd) = rx.recv().await {
        // Handle command
    }
}
```

**Application**: Use for per-account actors with shared resources via `Arc`

**Source 2**: Existing `crates/bot-orchestrator/src/bot_actor.rs`

**Current Pattern**: Already using actor pattern for bots
- Command enum with mpsc sender
- Tokio task spawned for message loop
- `Arc<Mutex<HashMap<String, BotHandle>>>` for registry

**Reuse Strategy**: Extend existing pattern to include account_id in BotActor

### Reference Implementations

**Example from `barter-rs`** (Rust trading framework):
```rust
pub struct ExchangeClient {
    exchange: ExchangeId,
    account_id: String,
    rate_limiter: Arc<Governor>,  // SHARED across clients!
    // ...
}
```

**Key Insight**: Share rate limiter via `Arc<Governor>` to prevent per-IP rate limit violations.

**Example from `openlimits`** (Multi-exchange Rust library):
```rust
pub struct Credentials {
    pub api_key: SecretString,
    pub api_secret: SecretString,
}
```

**Key Insight**: Use `secrecy` crate's `Secret<String>` to prevent logging API secrets.

---

## Section 4: Architectural Recommendations

### Proposed Design

**Hybrid Account Pool Pattern with Shared Rate Limiter**:

```
┌─────────────────────────────────────────────────────────────┐
│                      BotRegistry                            │
│              Arc<RwLock<HashMap<String, BotHandle>>>        │
└──────────────┬──────────────────┬───────────────────────────┘
               │                  │
       ┌───────▼────────┐  ┌─────▼──────────┐
       │  BotActor #1   │  │  BotActor #2   │
       │  account: "A"  │  │  account: "A"  │  (Multiple bots
       └───────┬────────┘  └─────┬──────────┘   same account)
               │                  │
       ┌───────▼─────────────────▼──────┐
       │  HyperliquidClient (acct A)    │
       │  - WebSocket connection        │
       │  - REST client                 │
       └───────┬────────────────────────┘
               │
       ┌───────▼──────────────────────────┐
       │  AccountRegistry                 │
       │  - accounts: HashMap<String, AccountConfig>
       │  - rate_limiter: Arc<RateLimiter> ← SHARED!
       └──────────────────────────────────┘
```

**Rationale**:
1. **Isolation**: Each account has dedicated `HyperliquidClient` (WebSocket + REST)
2. **Safety**: Shared `Arc<RateLimiter>` prevents IP bans from multiple accounts
3. **Flexibility**: Multiple bots can use same account (useful for different strategies)
4. **Minimal Change**: Extends existing `BotActor` pattern, no architectural rewrite

### Module Changes

#### **1. Create `crates/core/src/account.rs`** (NEW FILE, ~90 lines)

```rust
use secrecy::{Secret, ExposeSecret};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use governor::{RateLimiter, Quota, clock::DefaultClock, state::direct::NotKeyed};
use anyhow::{Result, Context, bail};

/// Configuration for a single Hyperliquid account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Unique identifier for this account (e.g., "main", "test", "strategy-a")
    pub account_id: String,

    /// Hyperliquid API key (kept secret)
    pub api_key: Secret<String>,

    /// Hyperliquid API secret (kept secret)
    pub api_secret: Secret<String>,

    /// Whether to use testnet
    #[serde(default)]
    pub testnet: bool,
}

/// Registry managing multiple Hyperliquid accounts with shared rate limiting
pub struct AccountRegistry {
    accounts: HashMap<String, AccountConfig>,
    rate_limiter: Arc<RateLimiter<NotKeyed, DefaultClock>>,
}

impl AccountRegistry {
    /// Create new registry from account configurations
    ///
    /// # Errors
    /// Returns error if duplicate account_id found
    pub fn new(accounts: Vec<AccountConfig>) -> Result<Self> {
        let mut map = HashMap::new();
        for account in accounts {
            if map.contains_key(&account.account_id) {
                bail!("Duplicate account_id: {}", account.account_id);
            }
            map.insert(account.account_id.clone(), account);
        }

        // Hyperliquid: 1200 req/min per IP (shared across ALL accounts)
        let quota = Quota::per_minute(nonzero::nonzero!(1200u32));
        let limiter = Arc::new(RateLimiter::direct(quota));

        Ok(Self {
            accounts: map,
            rate_limiter: limiter,
        })
    }

    /// Get account configuration by ID
    pub fn get(&self, account_id: &str) -> Option<&AccountConfig> {
        self.accounts.get(account_id)
    }

    /// Get shared rate limiter (used by all accounts to prevent IP ban)
    pub fn rate_limiter(&self) -> Arc<RateLimiter<NotKeyed, DefaultClock>> {
        Arc::clone(&self.rate_limiter)
    }

    /// List all account IDs (useful for validation errors)
    pub fn list_ids(&self) -> Vec<&str> {
        self.accounts.keys().map(String::as_str).collect()
    }
}
```

**Integration**: Add `pub mod account;` to `crates/core/src/lib.rs:5`

#### **2. Update `crates/core/Cargo.toml`** (Line 15, add dependency)

```toml
[dependencies]
# ... existing dependencies ...
secrecy = { version = "0.8.0", features = ["serde"] }
```

#### **3. Modify `crates/core/src/config.rs:15-30`**

```rust
// BEFORE (single account):
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperliquidConfig {
    pub api_key: String,
    pub api_secret: String,
    #[serde(default)]
    pub testnet: bool,
}

// AFTER (multiple accounts):
use crate::account::AccountConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperliquidConfig {
    /// Multiple accounts (new format)
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,

    /// Optional default account ID for backward compatibility
    #[serde(default)]
    pub default_account: Option<String>,

    // Legacy fields (for migration from old configs)
    #[serde(skip_serializing)]
    api_key: Option<String>,
    #[serde(skip_serializing)]
    api_secret: Option<String>,
    #[serde(skip_serializing)]
    testnet: Option<bool>,
}
```

#### **4. Add Migration Logic in `crates/core/src/config_loader.rs:50`**

```rust
impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let mut config: Config = Figment::new()
            .merge(Toml::file(path))
            .merge(Env::prefixed("ALGO_TRADE_"))
            .extract()
            .context("Failed to load configuration")?;

        // MIGRATION: Convert old single-account format to new format
        if config.hyperliquid.accounts.is_empty() {
            if let (Some(key), Some(secret)) = (
                config.hyperliquid.api_key.take(),
                config.hyperliquid.api_secret.take(),
            ) {
                let testnet = config.hyperliquid.testnet.unwrap_or(false);
                let account = AccountConfig {
                    account_id: "default".to_string(),
                    api_key: Secret::new(key),
                    api_secret: Secret::new(secret),
                    testnet,
                };
                config.hyperliquid.accounts.push(account);
                config.hyperliquid.default_account = Some("default".to_string());

                tracing::warn!("Migrated legacy single-account config to new format. Consider updating Config.toml to use [[hyperliquid.accounts]] array.");
            }
        }

        Ok(config)
    }
}
```

#### **5. Update `crates/bot-orchestrator/src/bot_actor.rs:25`**

```rust
// Add field to struct:
pub struct BotActor {
    bot_id: String,
    account_id: String,  // NEW: Which Hyperliquid account this bot uses
    data_provider: LiveDataProvider,
    execution_handler: LiveExecutionHandler,
    strategies: Vec<Arc<Mutex<dyn Strategy>>>,
    risk_manager: Arc<dyn RiskManager>,
}

// Update constructor (line 50):
impl BotActor {
    pub fn new(
        bot_id: String,
        account_id: String,  // NEW parameter
        data_provider: LiveDataProvider,
        execution_handler: LiveExecutionHandler,
        strategies: Vec<Arc<Mutex<dyn Strategy>>>,
        risk_manager: Arc<dyn RiskManager>,
    ) -> Self {
        Self {
            bot_id,
            account_id,  // Store account_id
            data_provider,
            execution_handler,
            strategies,
            risk_manager,
        }
    }
}
```

#### **6. Update `crates/exchange-hyperliquid/src/client.rs:20`**

```rust
pub struct HyperliquidClient {
    account_id: String,  // NEW: Track which account this client represents
    api_key: String,
    api_secret: String,
    http_client: reqwest::Client,
    rate_limiter: Arc<RateLimiter<NotKeyed, DefaultClock>>,  // Changed: Accept shared limiter
}

impl HyperliquidClient {
    pub fn new(
        account_id: String,  // NEW parameter
        api_key: String,
        api_secret: String,
        rate_limiter: Arc<RateLimiter<NotKeyed, DefaultClock>>,  // NEW: Shared limiter
    ) -> Self {
        Self {
            account_id,
            api_key,
            api_secret,
            http_client: reqwest::Client::new(),
            rate_limiter,  // Use provided limiter instead of creating new one
        }
    }
}
```

#### **7. Update `config/Config.toml:5-10`**

```toml
# OLD FORMAT (single account):
[hyperliquid]
api_key = "your-api-key"
api_secret = "your-api-secret"
testnet = false

# NEW FORMAT (multiple accounts):
[[hyperliquid.accounts]]
account_id = "main"
api_key = "your-main-api-key"
api_secret = "your-main-api-secret"
testnet = false

[[hyperliquid.accounts]]
account_id = "test"
api_key = "your-test-api-key"
api_secret = "your-test-api-secret"
testnet = true
```

#### **8. Update `crates/web-api/src/handlers.rs:45`**

```rust
// Add account_id to CreateBotRequest (line 20):
#[derive(Debug, Deserialize)]
pub struct CreateBotRequest {
    pub strategy: String,
    pub account_id: String,  // NEW: Which account to use
    // ... existing fields
}

// Validate account_id in create_bot handler (line 45):
pub async fn create_bot(
    State(registry): State<Arc<BotRegistry>>,
    State(account_registry): State<Arc<AccountRegistry>>,  // NEW: Inject AccountRegistry
    Json(request): Json<CreateBotRequest>,
) -> Result<Json<CreateBotResponse>, AppError> {
    // Validate account exists
    let account = account_registry.get(&request.account_id)
        .ok_or_else(|| {
            anyhow!(
                "Account '{}' not found. Available accounts: {:?}",
                request.account_id,
                account_registry.list_ids()
            )
        })?;

    // Create bot with specified account_id
    let bot_id = registry.create_bot(
        request.strategy,
        request.account_id,  // Pass account_id
        // ... other params
    ).await?;

    Ok(Json(CreateBotResponse { bot_id }))
}
```

### Critical Decisions

**Decision 1: Use `secrecy` crate for API credentials**
- **Rationale**: Rust's auto-derived `Debug` trait would print secrets to logs/errors
- **Alternative Considered**: Plain `String` (rejected - security risk in production)
- **Trade-off**: Requires `.expose_secret()` when using credentials, but worth it for safety
- **Impact**: Prevents accidental credential leakage in logs

**Decision 2: Shared `Arc<RateLimiter>` across all accounts**
- **Rationale**: Hyperliquid rate limit is per-IP, not per-account (1200 req/min total)
- **Alternative Considered**: Per-account limiters (rejected - would cause IP bans)
- **Trade-off**: Accounts share quota, but necessary for correctness
- **Impact**: With 3 accounts, each gets ~400 req/min on average (still sufficient)

**Decision 3: Backward compatibility via migration logic**
- **Rationale**: Existing users shouldn't need to manually update configs
- **Alternative Considered**: Breaking change (rejected - unnecessary user pain)
- **Trade-off**: Extra migration code (40 lines), but seamless upgrade path
- **Impact**: Old configs auto-convert to `accounts = [{ account_id = "default", ... }]`

**Decision 4: TOML array syntax `[[hyperliquid.accounts]]`**
- **Rationale**: Standard TOML pattern for array of tables
- **Alternative Considered**: JSON array in TOML string (rejected - unnatural)
- **Trade-off**: None - this is idiomatic TOML
- **Impact**: Users can easily add accounts by copy-pasting block

### Risk Assessment

**Breaking Changes**:
- ❌ NONE - Backward compatible via migration path

**Performance Implications**:
- ✅ HashMap account lookup: O(1), negligible overhead
- ⚠️  Shared rate limiter contention: Potential bottleneck with >20 accounts (unlikely in practice)
- ✅ WebSocket connections: ~1MB memory per account (acceptable for <100 accounts)

**Security Implications**:
- ✅ **IMPROVED**: Secrets no longer logged via Debug trait
- ✅ **IMPROVED**: API keys stored in TOML with file permissions (chmod 600)
- ⚠️  **TODO**: Consider keyring integration for production (Phase 11+)

---

## Section 5: Edge Cases & Constraints

### Edge Cases

**EC1: Invalid Account Credentials**
- **Scenario**: User provides wrong API key for account "main"
- **Current Behavior**: Bot would fail at first API call with 401 Unauthorized
- **Expected Behavior**: Fail fast during bot startup with clear error message
- **TaskMaster TODO**: Add credential validation in `AccountRegistry::new()` (optional pre-flight check)
- **Test**: Create account with invalid key, verify error message lists account_id

**EC2: Account Deleted While Bot Running**
- **Scenario**: Config reloaded (hot-reload), account "test" removed, but bot still references it
- **Current Behavior**: Undefined (config watcher would panic or stale config)
- **Expected Behavior**: Bot shuts down gracefully with warning log
- **TaskMaster TODO**: Add account existence check in config watcher (crates/core/src/config_watcher.rs:45)
- **Test**: Remove account from config, trigger reload, verify bot shutdown

**EC3: Non-Existent Account ID in Bot Creation**
- **Scenario**: User sends POST /api/bots with `account_id = "nonexistent"`
- **Current Behavior**: Would panic when accessing account
- **Expected Behavior**: 400 Bad Request with helpful error: "Account 'nonexistent' not found. Available: ['main', 'test']"
- **TaskMaster TODO**: Validation in create_bot handler (crates/web-api/src/handlers.rs:45)
- **Test**: Send invalid account_id, verify 400 response with account list

**EC4: Rate Limit Exhaustion with Many Accounts**
- **Scenario**: 10 bots across 5 accounts all making requests simultaneously
- **Current Behavior**: Shared limiter queues requests
- **Expected Behavior**: Requests wait (Governor's `until_ready()`), eventual timeout if overloaded
- **TaskMaster TODO**: Add timeout to rate limiter wait (e.g., 10s max wait)
- **Test**: Simulate 100 concurrent requests, verify no IP ban and graceful degradation

**EC5: Migration from Single-Account Config**
- **Scenario**: User has old `[hyperliquid]` config with `api_key = "..."`
- **Current Behavior**: New code expects `accounts = []` array
- **Expected Behavior**: Auto-migrate to `accounts = [{ account_id = "default", ... }]`
- **TaskMaster TODO**: Migration logic in config_loader.rs:50 (covered in Section 4)
- **Test**: Load old config, verify accounts[0].account_id == "default"

**EC6: Duplicate Account IDs**
- **Scenario**: Config has two `[[hyperliquid.accounts]]` with same `account_id = "main"`
- **Current Behavior**: HashMap would silently overwrite first with second
- **Expected Behavior**: Config load fails with error: "Duplicate account_id: main"
- **TaskMaster TODO**: Validation in `AccountRegistry::new()` (already in Section 4 code)
- **Test**: Create config with duplicates, verify error on load

**EC7: No Accounts Configured**
- **Scenario**: User has `[hyperliquid]` section but empty `accounts = []`
- **Current Behavior**: Bot creation would fail when looking up account
- **Expected Behavior**: Startup validation fails with: "No Hyperliquid accounts configured"
- **TaskMaster TODO**: Add validation in main.rs or config loader
- **Test**: Empty accounts array, verify startup error

### Constraints

**C1: Maximum Concurrent Accounts**
- **Limit**: 20 accounts (recommended soft limit)
- **Reason**: Each account = 1 WebSocket + REST client (~2MB memory + OS sockets)
- **Enforcement**: Add warning log if accounts.len() > 20
- **TaskMaster TODO**: Add check in `AccountRegistry::new()`

**C2: Configuration File Permissions**
- **Constraint**: Config.toml SHOULD have 600 permissions (owner read/write only)
- **Reason**: Contains API secrets in plaintext
- **Enforcement**: Log warning if permissions too permissive (optional)
- **TaskMaster TODO**: Add permission check in config_loader.rs (nice-to-have)

**C3: Secret Serialization**
- **Constraint**: API secrets MUST use `secrecy::Secret<String>`
- **Reason**: Prevents accidental logging via Debug/Display traits
- **Enforcement**: Type system (compiler enforces)
- **TaskMaster TODO**: Update all AccountConfig fields

**C4: Rate Limiter Must Be Shared**
- **Constraint**: ALL HyperliquidClient instances MUST use same Arc<RateLimiter>
- **Reason**: Hyperliquid limits per-IP (1200 req/min total, not per-account)
- **Enforcement**: Pass via constructor (cannot create independent limiter)
- **TaskMaster TODO**: Update HyperliquidClient constructor

**C5: Database Schema Compatibility**
- **Constraint**: `positions` table already has `account_id VARCHAR(50)` column (from Phase 5)
- **Reason**: No migration needed
- **Enforcement**: Verify in integration tests
- **TaskMaster TODO**: Ensure position tracking uses account_id field

### Testing Requirements

**Unit Tests** (crates/core/tests/account_test.rs):
- [ ] `AccountRegistry::new()` rejects duplicate account_id
- [ ] `AccountRegistry::get()` returns correct account
- [ ] `AccountRegistry::list_ids()` returns all IDs
- [ ] `AccountConfig` deserializes from TOML with Secret fields
- [ ] Rate limiter is shared (same Arc address) across multiple get calls

**Integration Tests** (crates/cli/tests/multi_account_test.rs):
- [ ] Old single-account config migrates to new format
- [ ] Multiple bots can use different accounts simultaneously
- [ ] Account deletion triggers bot shutdown via config watcher
- [ ] Invalid account_id in bot creation returns 400 error
- [ ] Rate limiting prevents IP ban with multiple accounts (mock HTTP server)

**Manual Tests** (documented in README.md):
- [ ] Invalid credentials fail gracefully with clear error
- [ ] WebSocket reconnect works independently per account
- [ ] Position tracking isolated per account (check database)
- [ ] Hot-reload config with new account adds it without restart

---

## Section 6: TaskMaster Handoff Package

### MUST DO

1. ✅ Create `crates/core/src/account.rs` with `AccountConfig` and `AccountRegistry` (~90 lines)
2. ✅ Add `secrecy = { version = "0.8.0", features = ["serde"] }` to `crates/core/Cargo.toml:15`
3. ✅ Modify `crates/core/src/config.rs:15-30` to use `Vec<AccountConfig>` with migration fields
4. ✅ Add migration logic in `crates/core/src/config_loader.rs:50` for backward compatibility (~40 lines)
5. ✅ Add `account_id: String` field to `BotActor` (crates/bot-orchestrator/src/bot_actor.rs:25)
6. ✅ Update `BotActor::new()` constructor to accept account_id parameter (line 50)
7. ✅ Modify `HyperliquidClient` to accept `Arc<RateLimiter>` (crates/exchange-hyperliquid/src/client.rs:20)
8. ✅ Update `config/Config.toml:5-10` to use `[[hyperliquid.accounts]]` array syntax
9. ✅ Add account_id validation in `create_bot` handler (crates/web-api/src/handlers.rs:45)
10. ✅ Inject `AccountRegistry` into web API server state (crates/web-api/src/server.rs:30)
11. ✅ Create integration test `crates/cli/tests/multi_account_test.rs` (~70 lines)
12. ✅ Document multi-account setup in README.md:80 (~40 lines)

### MUST NOT DO

1. ❌ DO NOT break existing single-account configurations (MUST auto-migrate)
2. ❌ DO NOT create per-account rate limiters (MUST share via Arc)
3. ❌ DO NOT log API secrets (MUST use `secrecy::Secret<String>`)
4. ❌ DO NOT modify `DataProvider` or `ExecutionHandler` trait signatures (public API)
5. ❌ DO NOT use `f64` for financial values (MUST use `rust_decimal::Decimal`)
6. ❌ DO NOT change database schema for `positions` table (already has account_id)
7. ❌ DO NOT add external dependencies beyond `secrecy` (keep minimal)

### Exact File Modifications

#### **Task 1: Create account.rs**
- **File**: `crates/core/src/account.rs` (NEW FILE)
- **Lines**: ~90
- **Complexity**: MEDIUM
- **Dependencies**: None (can run immediately)
- **Content**: See Section 4 code block (AccountConfig + AccountRegistry)

#### **Task 2: Update core Cargo.toml**
- **File**: `crates/core/Cargo.toml`
- **Line**: 15 (in `[dependencies]` section)
- **Complexity**: LOW
- **Add**: `secrecy = { version = "0.8.0", features = ["serde"] }`

#### **Task 3: Update lib.rs**
- **File**: `crates/core/src/lib.rs`
- **Line**: 5 (after existing mod declarations)
- **Complexity**: LOW
- **Add**: `pub mod account;`

#### **Task 4: Modify config.rs**
- **File**: `crates/core/src/config.rs`
- **Lines**: 1 (add import), 15-30 (modify struct)
- **Complexity**: MEDIUM
- **Dependencies**: Requires Task 1 (account.rs exists)
- **Changes**:
  ```rust
  // Line 1: Add import
  use crate::account::AccountConfig;

  // Lines 15-30: Replace HyperliquidConfig
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct HyperliquidConfig {
      #[serde(default)]
      pub accounts: Vec<AccountConfig>,
      #[serde(default)]
      pub default_account: Option<String>,
      // Legacy fields for migration
      #[serde(skip_serializing)]
      api_key: Option<String>,
      #[serde(skip_serializing)]
      api_secret: Option<String>,
      #[serde(skip_serializing)]
      testnet: Option<bool>,
  }
  ```

#### **Task 5: Add migration logic in config_loader.rs**
- **File**: `crates/core/src/config_loader.rs`
- **Line**: 50 (inside `load_config` function, after Figment extraction)
- **Complexity**: HIGH
- **Dependencies**: Requires Task 4 (config.rs updated)
- **Add**:
  ```rust
  // Migration: Convert old single-account format
  if config.hyperliquid.accounts.is_empty() {
      if let (Some(key), Some(secret)) = (
          config.hyperliquid.api_key.take(),
          config.hyperliquid.api_secret.take(),
      ) {
          use secrecy::Secret;
          let testnet = config.hyperliquid.testnet.unwrap_or(false);
          let account = crate::account::AccountConfig {
              account_id: "default".to_string(),
              api_key: Secret::new(key),
              api_secret: Secret::new(secret),
              testnet,
          };
          config.hyperliquid.accounts.push(account);
          config.hyperliquid.default_account = Some("default".to_string());
          tracing::warn!("Migrated legacy config to new multi-account format");
      }
  }

  // Validation: Ensure at least one account
  if config.hyperliquid.accounts.is_empty() {
      anyhow::bail!("No Hyperliquid accounts configured");
  }
  ```

#### **Task 6: Update bot_actor.rs**
- **File**: `crates/bot-orchestrator/src/bot_actor.rs`
- **Lines**: 25 (add field), 50-60 (update constructor)
- **Complexity**: MEDIUM
- **Changes**:
  ```rust
  // Line 25: Add to struct
  pub struct BotActor {
      bot_id: String,
      account_id: String,  // NEW
      // ... existing fields
  }

  // Line 50: Update constructor
  pub fn new(
      bot_id: String,
      account_id: String,  // NEW parameter
      data_provider: LiveDataProvider,
      execution_handler: LiveExecutionHandler,
      strategies: Vec<Arc<Mutex<dyn Strategy>>>,
      risk_manager: Arc<dyn RiskManager>,
  ) -> Self {
      Self {
          bot_id,
          account_id,  // Store it
          data_provider,
          execution_handler,
          strategies,
          risk_manager,
      }
  }
  ```

#### **Task 7: Update HyperliquidClient**
- **File**: `crates/exchange-hyperliquid/src/client.rs`
- **Lines**: 10 (add import), 20-25 (modify struct), 35-45 (update constructor)
- **Complexity**: MEDIUM
- **Changes**:
  ```rust
  // Line 10: Add import
  use std::sync::Arc;

  // Line 20: Modify struct
  pub struct HyperliquidClient {
      account_id: String,  // NEW
      api_key: String,
      api_secret: String,
      http_client: reqwest::Client,
      rate_limiter: Arc<RateLimiter<NotKeyed, DefaultClock>>,  // Changed from owned
  }

  // Line 35: Update constructor
  pub fn new(
      account_id: String,  // NEW
      api_key: String,
      api_secret: String,
      rate_limiter: Arc<RateLimiter<NotKeyed, DefaultClock>>,  // NEW
  ) -> Self {
      Self {
          account_id,
          api_key,
          api_secret,
          http_client: reqwest::Client::new(),
          rate_limiter,  // Use provided limiter
      }
  }
  ```

#### **Task 8: Update Config.toml**
- **File**: `config/Config.toml`
- **Lines**: 5-15 (replace single account with array)
- **Complexity**: LOW
- **Change**:
  ```toml
  # BEFORE:
  [hyperliquid]
  api_key = "your-api-key"
  api_secret = "your-api-secret"
  testnet = false

  # AFTER:
  [[hyperliquid.accounts]]
  account_id = "main"
  api_key = "your-main-api-key"
  api_secret = "your-main-api-secret"
  testnet = false

  [[hyperliquid.accounts]]
  account_id = "test"
  api_key = "your-test-api-key"
  api_secret = "your-test-api-secret"
  testnet = true
  ```

#### **Task 9: Add validation in create_bot handler**
- **File**: `crates/web-api/src/handlers.rs`
- **Lines**: 20 (update request struct), 45-50 (add validation)
- **Complexity**: MEDIUM
- **Dependencies**: Requires AccountRegistry available in server state
- **Changes**:
  ```rust
  // Line 20: Add to request
  #[derive(Debug, Deserialize)]
  pub struct CreateBotRequest {
      pub strategy: String,
      pub account_id: String,  // NEW
      // ... existing fields
  }

  // Line 45: Add validation
  pub async fn create_bot(
      State(registry): State<Arc<BotRegistry>>,
      State(account_registry): State<Arc<AccountRegistry>>,  // NEW
      Json(request): Json<CreateBotRequest>,
  ) -> Result<Json<CreateBotResponse>, AppError> {
      // Validate account exists
      account_registry.get(&request.account_id)
          .ok_or_else(|| anyhow!(
              "Account '{}' not found. Available: {:?}",
              request.account_id,
              account_registry.list_ids()
          ))?;

      let bot_id = registry.create_bot(
          request.strategy,
          request.account_id,  // Pass account_id
          // ... other params
      ).await?;

      Ok(Json(CreateBotResponse { bot_id }))
  }
  ```

#### **Task 10: Inject AccountRegistry into server**
- **File**: `crates/web-api/src/server.rs`
- **Lines**: 30-35 (add to shared state)
- **Complexity**: MEDIUM
- **Changes**:
  ```rust
  pub async fn run_server(addr: SocketAddr, registry: Arc<BotRegistry>, account_registry: Arc<AccountRegistry>) -> Result<()> {
      let app = Router::new()
          .route("/api/bots", get(handlers::list_bots))
          .route("/api/bots", post(handlers::create_bot))
          // ... existing routes
          .layer(Extension(registry))
          .layer(Extension(account_registry));  // NEW

      axum::Server::bind(&addr)
          .serve(app.into_make_service())
          .await?;

      Ok(())
  }
  ```

#### **Task 11: Create integration test**
- **File**: `crates/cli/tests/multi_account_test.rs` (NEW)
- **Lines**: ~70
- **Complexity**: MEDIUM
- **Content**:
  ```rust
  use algo_trade_core::config::Config;
  use algo_trade_core::account::AccountRegistry;
  use anyhow::Result;

  #[tokio::test]
  async fn test_migration_from_old_config() -> Result<()> {
      // Create old-format config
      let config_str = r#"
          [hyperliquid]
          api_key = "test-key"
          api_secret = "test-secret"
          testnet = true
      "#;

      // Load and verify migration
      let config: Config = toml::from_str(config_str)?;
      assert_eq!(config.hyperliquid.accounts.len(), 1);
      assert_eq!(config.hyperliquid.accounts[0].account_id, "default");

      Ok(())
  }

  #[tokio::test]
  async fn test_multi_account_registry() -> Result<()> {
      // Test AccountRegistry with multiple accounts
      let accounts = vec![
          AccountConfig { account_id: "A".into(), ... },
          AccountConfig { account_id: "B".into(), ... },
      ];

      let registry = AccountRegistry::new(accounts)?;
      assert!(registry.get("A").is_some());
      assert!(registry.get("B").is_some());
      assert!(registry.get("C").is_none());

      Ok(())
  }

  #[tokio::test]
  async fn test_duplicate_account_id_rejected() {
      let accounts = vec![
          AccountConfig { account_id: "A".into(), ... },
          AccountConfig { account_id: "A".into(), ... },  // Duplicate!
      ];

      let result = AccountRegistry::new(accounts);
      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("Duplicate"));
  }
  ```

#### **Task 12: Document in README**
- **File**: `README.md`
- **Line**: 80 (add new section)
- **Complexity**: LOW
- **Add**:
  ```markdown
  ## Managing Multiple Accounts

  You can configure multiple Hyperliquid accounts for different strategies or risk profiles.

  ### Configuration

  In `config/Config.toml`, use TOML array syntax:

  ```toml
  [[hyperliquid.accounts]]
  account_id = "main"
  api_key = "your-main-api-key"
  api_secret = "your-main-api-secret"
  testnet = false

  [[hyperliquid.accounts]]
  account_id = "test"
  api_key = "your-test-api-key"
  api_secret = "your-test-api-secret"
  testnet = true
  ```

  ### Creating Bots with Specific Accounts

  When creating a bot via API:

  ```bash
  curl -X POST http://localhost:8080/api/bots \
    -H "Content-Type: application/json" \
    -d '{
      "strategy": "ma_crossover",
      "account_id": "main"
    }'
  ```

  ### Migration from Old Configs

  Old single-account configs are automatically migrated:

  ```toml
  # OLD (still works):
  [hyperliquid]
  api_key = "..."
  api_secret = "..."

  # Automatically becomes:
  [[hyperliquid.accounts]]
  account_id = "default"
  api_key = "..."
  api_secret = "..."
  ```

  ### Rate Limiting

  All accounts share a single rate limiter (1200 req/min per IP). This prevents Hyperliquid IP bans when running multiple accounts.
  ```

### Task Dependencies

```
Task 1 (account.rs) ──┬─→ Task 3 (lib.rs)
                      ├─→ Task 4 (config.rs)
                      └─→ Task 5 (migration)

Task 2 (Cargo.toml) ──→ Task 1

Task 4 ──→ Task 5 (migration logic)
      └─→ Task 6 (bot_actor.rs)

Task 6 ──→ Task 9 (validation)

Task 7 (client.rs) ──→ Task 10 (server state)

Task 9 ──┬─→ Task 10 (server)
         └─→ Task 11 (integration tests)

Task 8 (Config.toml) ──→ Task 11 (tests)

All tasks ──→ Task 12 (README)
```

**Critical Path**: 1 → 3 → 4 → 5 → 6 → 9 → 10 → 11 → 12 (~280 LOC, ~3.5 hours)

### Estimated Complexity

| Task | LOC | Time | Risk | Reason |
|------|-----|------|------|--------|
| 1 | 90 | 40m | MED | New module, rate limiter integration |
| 2 | 1 | 2m | LOW | Dependency add |
| 3 | 1 | 2m | LOW | Module declaration |
| 4 | 20 | 20m | MED | Struct change with migration fields |
| 5 | 40 | 50m | HIGH | Backward compat critical |
| 6 | 10 | 15m | LOW | Field + constructor update |
| 7 | 20 | 25m | MED | Shared state via Arc |
| 8 | 15 | 5m | LOW | Config file update |
| 9 | 15 | 20m | MED | Validation logic |
| 10 | 5 | 10m | LOW | State injection |
| 11 | 70 | 45m | MED | Integration test coverage |
| 12 | 40 | 20m | LOW | Documentation |

**Total**: ~327 LOC, ~4 hours, Risk: MEDIUM

### Verification Criteria

**Per-Task Verification** (run after each task):

- [ ] **Task 1**: `cargo build -p algo-trade-core --lib` succeeds
- [ ] **Task 2**: `cargo tree -p algo-trade-core | grep secrecy` shows 0.8.0
- [ ] **Task 3**: `cargo check -p algo-trade-core` succeeds
- [ ] **Task 4**: `cargo test -p algo-trade-core config` passes
- [ ] **Task 5**: Load old config, verify migration warning in logs
- [ ] **Task 6**: `cargo check -p algo-trade-bot-orchestrator` succeeds
- [ ] **Task 7**: `cargo check -p algo-trade-exchange-hyperliquid` succeeds
- [ ] **Task 8**: Config parses: `toml::from_str::<Config>(file_contents)`
- [ ] **Task 9**: `cargo check -p algo-trade-web-api` succeeds
- [ ] **Task 10**: `cargo build -p algo-trade-web-api` succeeds
- [ ] **Task 11**: `cargo test multi_account` passes
- [ ] **Task 12**: `mdl README.md` (markdown linter) passes

**Karen Quality Gates** (run after ALL tasks complete):

- [ ] **Phase 0**: `cargo build --workspace` succeeds
- [ ] **Phase 1**: `cargo clippy --workspace --all-targets -- -D warnings -W clippy::pedantic -W clippy::nursery` returns zero warnings
- [ ] **Phase 2**: `rust-analyzer diagnostics` shows zero issues
- [ ] **Phase 3**: Cross-file validation (no broken imports)
- [ ] **Phase 4**: Per-file verification (each file compiles independently)
- [ ] **Phase 6**: `cargo build --workspace --release && cargo test --workspace` succeeds

---

## Appendices

### Appendix A: Commands Executed

```bash
# Codebase reconnaissance
Glob: pattern="**/config*.rs"
Glob: pattern="**/websocket*.rs"
Grep: pattern="struct.*Config" output_mode="files_with_matches"
Grep: pattern="RateLimiter" output_mode="content" -n=true
Read: file_path="/home/a/Work/algo-trade/crates/core/src/config.rs"
Read: file_path="/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs"
Read: file_path="/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs"

# External research
WebSearch: query="rust secrecy crate API secrets"
WebFetch: url="https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint" prompt="Rate limiting per account vs per IP"
WebFetch: url="https://ryhl.io/blog/actors-with-tokio/" prompt="Managing multiple actor instances with shared resources"
```

### Appendix B: Files Examined

| File | Lines Read | Purpose |
|------|------------|---------|
| `crates/core/src/config.rs` | 1-80 | Understand current config structure |
| `crates/core/src/config_loader.rs` | 1-100 | See Figment usage pattern |
| `crates/exchange-hyperliquid/src/client.rs` | 1-60 | Check rate limiter implementation |
| `crates/exchange-hyperliquid/src/websocket.rs` | 1-100 | WebSocket connection management |
| `crates/bot-orchestrator/src/bot_actor.rs` | 1-150 | Bot actor pattern |
| `crates/bot-orchestrator/src/registry.rs` | 1-120 | Bot registry Arc<RwLock> pattern |
| `crates/web-api/src/handlers.rs` | 1-130 | API handler structure |
| `config/Config.toml` | 1-50 | Current config format |

### Appendix C: External References

1. **secrecy crate** (v0.8.0): https://docs.rs/secrecy/0.8.0/secrecy/
   - Prevents accidental secret exposure via Debug trait
   - Zeroizes memory on drop

2. **Hyperliquid API Docs**: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api
   - Rate limit: 1200 req/min per IP (not per account)
   - WebSocket subscriptions per account

3. **Alice Ryhl - Actors with Tokio**: https://ryhl.io/blog/actors-with-tokio/
   - DIY actor pattern without heavyweight frameworks
   - Shared resources via Arc

4. **Rust API Guidelines**: https://rust-lang.github.io/api-guidelines/
   - Error handling best practices
   - API stability guidelines

---

**Context Gatherer Report Complete** ✅

**Ready for TaskMaster**: YES
- Tasks: 12
- LOC: ~327
- Complexity: MEDIUM
- Critical decisions: 4 (all documented with rationale)

**Handoff to TaskMaster**: Section 6 contains complete implementation package with zero ambiguity.
