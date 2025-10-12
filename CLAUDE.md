# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Hyperliquid algorithmic trading system in Rust with modular architecture enabling backtest-live parity. Event-driven design ensures identical strategy code runs in backtesting and production.

## Architecture

### Core Design Pattern

**Event-Driven Architecture**: All components process discrete events sequentially, eliminating look-ahead bias and matching real-time trading exactly.

**Trait Abstraction**: `DataProvider` and `ExecutionHandler` traits enable swapping between backtest (historical data, simulated fills) and live (WebSocket data, real orders) without changing strategy code.

**Actor Pattern**: Bots use Tokio channels (mpsc for commands, watch for config updates, broadcast for status) following Alice Ryhl's DIY actor guide—no heavyweight frameworks.

### Workspace Structure

```
crates/
├── core/               # Event types, traits, TradingSystem engine
├── exchange-hyperliquid/ # REST/WebSocket, rate limiting, auth
├── data/               # TimescaleDB, Arrow, Parquet
├── strategy/           # Strategy trait impls (MA, RSI, etc.)
├── execution/          # Order management
├── backtest/           # Historical simulation, metrics
├── bot-orchestrator/   # Multi-bot coordination
├── web-api/            # Axum REST + WebSocket
└── cli/                # Command-line interface
```

### Event Flow

```
MarketEvent → Strategy::on_market_event() → SignalEvent
SignalEvent → RiskManager::evaluate_signal() → OrderEvent
OrderEvent → ExecutionHandler::execute_order() → FillEvent
```

### Key Dependencies

- **tokio**: Async runtime (all async code uses Tokio)
- **axum**: Web framework for API (preferred over actix-web for memory efficiency)
- **sqlx**: PostgreSQL/TimescaleDB client (async, compile-time checked queries)
- **polars**: DataFrame processing (10-100x faster than pandas)
- **arrow/parquet**: Columnar storage
- **figment**: Multi-source config (TOML + env + JSON)
- **hyperliquid-rust-sdk**: Official exchange SDK (maintain fork for production)

## Development Commands

### Building

```bash
# Check all crates
cargo check

# Build release
cargo build --release

# Build specific crate
cargo build -p algo-trade-core
```

### Testing

```bash
# All tests
cargo test

# Integration tests only
cargo test --test integration_test

# Specific crate
cargo test -p algo-trade-backtest
```

### Running

```bash
# Backtest
cargo run -p algo-trade-cli -- backtest --data tests/data/sample.csv --strategy ma_crossover

# Live trading
cargo run -p algo-trade-cli -- run --config config/Config.toml

# Web API only
cargo run -p algo-trade-cli -- server --addr 0.0.0.0:8080

# Interactive TUI for managing live trading bots
cargo run -p algo-trade-cli -- live-bot-tui

# Interactive TUI for viewing backtest results and token selection
cargo run -p algo-trade-cli -- backtest-manager-tui

# Backtest-driven bot deployment daemon (auto-deploy paper bots)
cargo run -p algo-trade-cli -- backtest-daemon --config config/Config.toml --strategy quad_ma

# With debug logging
RUST_LOG=debug cargo run -p algo-trade-cli -- run
```

### Linting

```bash
# Clippy (all warnings as errors)
cargo clippy -- -D warnings

# Clippy for specific crate
cargo clippy -p algo-trade-core -- -D warnings

# Format
cargo fmt
```

## Critical Patterns

### 1. Financial Precision

**ALWAYS use `rust_decimal::Decimal` for prices, quantities, PnL**. Never use `f64` for financial calculations—rounding errors compound over thousands of operations.

```rust
// CORRECT
use rust_decimal::Decimal;
let price: Decimal = "42750.50".parse()?;

// WRONG - will accumulate errors
let price: f64 = 42750.50;
```

### 2. Backtest-Live Parity

Strategy and RiskManager implementations must be provider-agnostic. Only `DataProvider` and `ExecutionHandler` differ between backtest and live.

```rust
// Strategy sees MarketEvent - doesn't know if backtest or live
async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
    // Same logic runs everywhere
}
```

### 3. Actor Pattern for Bots

Each bot is a spawned task owning `mpsc::Receiver<BotCommand>`. Handle is `Clone` with `mpsc::Sender` for multiple controllers.

```rust
// Spawn bot
let (tx, rx) = mpsc::channel(32);
let handle = BotHandle::new(tx);
tokio::spawn(async move { BotActor::new(config, rx).run().await });
```

### 4. Rate Limiting

Use `governor` crate with per-exchange quotas:
- Hyperliquid: 1200 weight/min (20 req/s)
- Binance: 1200 req/min
- Apply backoff on rate limit errors

### 5. Database Operations

**Batch writes for performance**: Single inserts ~390µs, batching 100 inserts ~13ms (3x speedup per record).

```rust
// Collect records, then batch insert
db.insert_ohlcv_batch(records).await?;
```

**Use hypertables**: TimescaleDB's `create_hypertable()` for time-series data, automatic partitioning.

### 6. Configuration Hot-Reload

Config updates flow via `tokio::sync::watch` channels. Bots subscribe and receive latest config without restart.

```rust
let (watcher, mut config_rx) = ConfigWatcher::new(config);
tokio::select! {
    _ = config_rx.changed() => {
        let new_config = config_rx.borrow().clone();
        // Apply new config
    }
}
```

## TUI Applications

The CLI includes three interactive terminal user interfaces built with ratatui:

### 1. Multi-Token Backtest TUI (`tui-backtest`)

Run parameter sweep backtests across multiple tokens:
- Fetch OHLCV data for selected tokens
- Run backtests with configurable parameter ranges
- View results table with sortable metrics
- Filter by performance criteria

**Usage:**
```bash
cargo run -p algo-trade-cli -- tui-backtest --start 2025-01-01T00:00:00Z --end 2025-02-01T00:00:00Z
```

### 2. Live Bot Management TUI (`live-bot-tui`)

Monitor and control live trading bots:
- Dashboard with real-time bot metrics (PnL, Sharpe, drawdown)
- Start/stop/configure individual bots
- View open positions and recent trades
- Monitor bot health and heartbeats

**Usage:**
```bash
cargo run -p algo-trade-cli -- live-bot-tui

# With file logging (prevents terminal corruption)
cargo run -p algo-trade-cli -- live-bot-tui --log-file /tmp/live-bot.log
```

**Access via Docker:**
```bash
docker compose up -d app
# Access at http://localhost:7681
```

### 3. Backtest Manager TUI (`backtest-manager-tui`)

View historical backtest results and token selection status:
- **Dashboard**: Scheduler status, backtest counts, token approval summary
- **Reports**: Table of all backtest results with sortable metrics (Sharpe, win rate, etc.)
- **Token Selection**: View approved/rejected tokens with filtering criteria
- **Config**: View scheduler and selector configuration
- **Report Detail**: Detailed view of individual backtest metrics

**Usage:**
```bash
cargo run -p algo-trade-cli -- backtest-manager-tui

# With file logging
cargo run -p algo-trade-cli -- backtest-manager-tui --log-file /tmp/backtest-manager.log
```

**Access via Docker:**
```bash
docker compose up -d backtest-manager
# Access at http://localhost:7682
```

**Keyboard Navigation:**
- `d` - Dashboard screen
- `r` - Reports list
- `t` - Token selection
- `c` - Configuration view
- `↑/↓` - Navigate lists
- `Enter` - View report detail
- `Esc` - Return to reports list
- `q` - Quit

**Data Source:** Reads from TimescaleDB `backtest_results` table populated by `backtest-daemon` or `scheduled-backtest`.

## Adding New Features

### New Strategy

1. Implement `Strategy` trait in `crates/strategy/src/`
2. Add state (price buffers, indicators) as struct fields
3. Process `MarketEvent` in `on_market_event()`
4. Return `SignalEvent` on signal generation

```rust
pub struct MyStrategy { /* state */ }

#[async_trait]
impl Strategy for MyStrategy {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        // Update state, generate signal
    }
    fn name(&self) -> &str { "My Strategy" }
}
```

### New Exchange Integration

1. Create crate `crates/exchange-{name}/`
2. Implement `DataProvider` for WebSocket market data
3. Implement `ExecutionHandler` for order execution
4. Add rate limiting with `governor`
5. Handle authentication and reconnection

### New REST Endpoint

Add to `crates/web-api/src/handlers.rs`:

```rust
pub async fn my_handler(
    State(registry): State<Arc<BotRegistry>>,
    Json(req): Json<MyRequest>,
) -> Result<Json<MyResponse>, StatusCode> {
    // Implementation
}
```

Add route in `crates/web-api/src/server.rs`:

```rust
.route("/api/my-endpoint", post(handlers::my_handler))
```

## Database Schema

### OHLCV Table (Hypertable)

```sql
CREATE TABLE ohlcv (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    open DECIMAL(20, 8) NOT NULL,
    high DECIMAL(20, 8) NOT NULL,
    low DECIMAL(20, 8) NOT NULL,
    close DECIMAL(20, 8) NOT NULL,
    volume DECIMAL(20, 8) NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange)
);
```

- **DECIMAL(20, 8)**: Precise financial data (never FLOAT/DOUBLE)
- **Hypertable**: Automatic time-based partitioning
- **Compression**: Enabled for data >7 days old

## Troubleshooting

### "Task panicked" errors

Check Tokio runtime: all async code must run inside `#[tokio::main]` or spawned tasks.

### Rate limit errors from Hyperliquid

Check `governor` quota configuration. Hyperliquid allows 1200 weight/min, most requests cost 1 weight.

### Database connection errors

Verify TimescaleDB extension: `CREATE EXTENSION IF NOT EXISTS timescaledb;`

### WebSocket disconnects

Check auto-reconnect logic in `HyperliquidWebSocket::reconnect()`. Should have exponential backoff.

### Backtest vs Live divergence

Strategy implementation likely has look-ahead bias. Ensure all logic works event-by-event, not on future data.

## Docker Deployment

### Services

The docker-compose.yml includes five services:

1. **timescaledb** - TimescaleDB for backtest results and OHLCV data
2. **app** - Main trading system with web API and live bot TUI (port 7681)
3. **backtest-manager** - Backtest results viewer TUI (port 7682)
4. **scheduled-backtest** - Scheduled backtest daemon (no TUI, cron-based)
5. **backtest-daemon** - Backtest-driven bot deployment daemon (auto-deploys paper bots)

### Running with Docker Compose

```bash
# Start all services
docker compose up -d

# Start specific service
docker compose up -d backtest-manager

# View logs
docker compose logs -f backtest-manager

# Access TUIs via browser
# Live Bot TUI: http://localhost:7681
# Backtest Manager TUI: http://localhost:7682

# Stop all services
docker compose down
```

### Environment Variables

Key environment variables (set in .env or docker-compose.yml):

- `DB_PASSWORD` - PostgreSQL/TimescaleDB password (required)
- `BACKTEST_TUI_PORT` - Host port for backtest manager (default: 7682)
- `TUI_PORT` - Host port for live bot TUI (default: 7681)
- `API_PORT` - Web API port (default: 8080)
- `RUST_LOG` - Log level (default: info)
- `HYPERLIQUID_API_URL` - Hyperliquid API endpoint
- `HYPERLIQUID_WS_URL` - Hyperliquid WebSocket endpoint

### Service Differences

**app service:**
- Runs trading daemon + live bot TUI
- Needs bot database (SQLite) for persistence
- Requires Hyperliquid credentials for live trading

**backtest-manager service:**
- Runs backtest manager TUI only (no daemon)
- Read-only access to TimescaleDB
- No credentials required (view-only mode)

**scheduled-backtest service:**
- Runs scheduled backtests on cron schedule (configured in Config.toml)
- Fetches OHLCV data from Hyperliquid automatically
- Stores backtest results in TimescaleDB
- No TUI (daemon only)
- No bot persistence needed (doesn't manage bots)

**backtest-daemon service:**
- Runs scheduled-backtest in background (inherits cron schedule)
- Monitors token selection results every 5 minutes
- Auto-deploys paper trading bots for approved tokens
- Needs bot database (SQLite) for bot persistence
- No TUI (daemon only)
- Default strategy: quad_ma (can be changed via command in docker-compose.yml)

## Playbook Reference

The complete implementation plan is available in `.claude/playbooks/2025-10-01_hyperliquid-trading-system.md`. This playbook contains:

- 10 phases of atomic implementation tasks
- Exact file paths and line-by-line code specifications
- Verification steps for each phase
- Architecture decisions based on research-validated patterns
- **Karen quality gates at every phase boundary (mandatory)**

## Agent Orchestration Workflow

This project follows **Anthropic's 3-step AI Orchestration Cycle** with three specialized agents:

### The Three Agents

1. **Context Gatherer** (Information Gathering)
   - **Purpose**: Front-load comprehensive research before implementation
   - **When**: New features, external integrations, architectural decisions
   - **Output**: Context reports in `.claude/context/YYYY-MM-DD_feature-name.md`
   - **Specification**: `.claude/agents/context-gatherer.md`

2. **TaskMaster** (Task Creation)
   - **Purpose**: Break work into atomic, verifiable tasks
   - **When**: All medium/large features (3+ file changes)
   - **Output**: Playbooks in `.claude/playbooks/YYYY-MM-DD_feature-name.md`
   - **Specification**: `.claude/agents/taskmaster.md`

3. **Karen** (Quality Assurance)
   - **Purpose**: Enforce zero-tolerance quality standards
   - **When**: After every phase completion (MANDATORY)
   - **Output**: Quality review reports with terminal outputs
   - **Specification**: `.claude/agents/karen.md`

### Full Workflow (Complex Features)

```
User Request: "Add support for trading futures on Hyperliquid"
     │
     ▼
┌────────────────────────────────────────────────────────┐
│ STEP 1: Information Gathering (Context Gatherer)       │
├────────────────────────────────────────────────────────┤
│ • Research Hyperliquid futures API                     │
│ • Analyze existing spot trading implementation         │
│ • Evaluate design patterns (futures vs spot)           │
│ • Identify edge cases (funding, expiry)                │
│ • Generate architectural recommendations               │
└────────────┬───────────────────────────────────────────┘
             │
             ▼ Produces: .claude/context/2025-10-02_hyperliquid-futures.md
             │           (Section 6: TaskMaster Handoff Package)
             │
┌────────────▼───────────────────────────────────────────┐
│ STEP 2: Task Creation (TaskMaster)                     │
├────────────────────────────────────────────────────────┤
│ • Read Context Report Section 6                        │
│ • Extract MUST DO / MUST NOT DO                        │
│ • Convert to atomic tasks (~50 LOC each)               │
│ • Add verification criteria per task                   │
│ • Define task dependencies                             │
└────────────┬───────────────────────────────────────────┘
             │
             ▼ Produces: .claude/playbooks/2025-10-02_hyperliquid-futures.md
             │
┌────────────▼───────────────────────────────────────────┐
│ Execution: Complete all atomic tasks in playbook       │
└────────────┬───────────────────────────────────────────┘
             │
             ▼
┌────────────▼───────────────────────────────────────────┐
│ STEP 3: Quality Assurance (Karen) ← MANDATORY          │
├────────────────────────────────────────────────────────┤
│ • Phase 0: Compilation check                           │
│ • Phase 1: Clippy (default + pedantic + nursery)       │
│ • Phase 2: rust-analyzer diagnostics                   │
│ • Phase 3: Cross-file validation                       │
│ • Phase 4: Per-file verification                       │
│ • Phase 5: Report generation                           │
│ • Phase 6: Final verification                          │
└────────────┬───────────────────────────────────────────┘
             │
             ▼
        Pass? ┬─ YES → Feature Complete
              └─ NO  → Fix Issues → Re-run Karen
```

### Simplified Workflow (Simple Features)

For changes affecting <5 files with clear requirements:

```
User Request
     ↓
TaskMaster (minimal analysis + playbook generation)
     ↓
Execute Tasks
     ↓
Karen Review ← MANDATORY
     ↓
Complete
```

**Skip Context Gatherer when**:
- No external research needed
- Clear, specific user request
- Affects <5 files
- No architectural decisions

### Context Gatherer Deep Dive

**7-Phase Process**:
1. **Request Analysis** - Extract explicit/implicit requirements
2. **Codebase Reconnaissance** - Map existing patterns, find integration points
3. **External Research** - Evaluate crates, APIs, design patterns
4. **Analysis & Synthesis** - Architectural recommendations with rationale
5. **Edge Case Identification** - Document error scenarios, constraints
6. **TaskMaster Package Creation** - MUST DO/MUST NOT DO with exact file paths
7. **Report Generation** - Save to `.claude/context/`

**Example Report Structure**:
- Section 1: Request Analysis
- Section 2: Codebase Context (files with line numbers)
- Section 3: External Research (crate comparison tables)
- Section 4: Architectural Recommendations (proposed design)
- Section 5: Edge Cases & Constraints
- **Section 6: TaskMaster Handoff Package** ← This feeds directly into TaskMaster

**Invoke Context Gatherer when**:
```bash
# User says: "Add support for perpetual futures on Hyperliquid"

# Response:
"This feature requires comprehensive research. I recommend invoking Context Gatherer first to:
1. Research Hyperliquid perpetual futures API (funding rates, margin)
2. Analyze existing spot trading architecture (integration points)
3. Evaluate design patterns (extend OrderEvent vs new FuturesOrder type)
4. Identify edge cases (funding payments, position rollovers)
5. Generate TaskMaster Handoff Package with exact scope

Would you like me to proceed with Context Gatherer?"
```

### TaskMaster Deep Dive

**Atomic Task Specification**:
- **One change per task** (single file, function, or struct)
- **Exact locations** (file path + line numbers)
- **Clear verification** (cargo check, cargo test command)
- **< 50 LOC per task** (prevents scope creep)

**With Context Report** (preferred):
1. Read `.claude/context/YYYY-MM-DD_feature-name.md` Section 6
2. Extract pre-defined scope boundaries (MUST DO / MUST NOT DO)
3. Use exact file paths and line numbers from report
4. Generate playbook with minimal additional research

**Without Context Report** (fallback):
1. Analyze user request
2. Locate files with Glob/Grep/Read
3. Define minimal scope
4. Generate playbook (may need to request Context Gatherer if complex)

**Playbook Structure**:
```markdown
## User Request
[Verbatim request]

## Scope Boundaries
### MUST DO
- [ ] Task 1 (file: path, lines: 10-50)
### MUST NOT DO
- No new features beyond request

## Atomic Tasks
### Task 1: [Specific Goal]
**File**: /path/to/file.rs
**Location**: Function `foo()` (lines 100-120)
**Action**: Change parameter type from String to &str
**Verification**: cargo check -p package-name
**Estimated LOC**: 3

## Verification Checklist
- [ ] cargo build succeeds
- [ ] cargo clippy passes
- [ ] Karen review passes ← MANDATORY
```

### Karen Agent Deep Dive

**Zero Tolerance Quality Enforcement** - Karen blocks phase progression until all issues fixed.

**Karen's 6-Phase Review**:
1. **Phase 0**: Compilation check (`cargo build --package <pkg> --lib`)
2. **Phase 1**: Clippy at ALL levels (default + pedantic + nursery) - Zero warnings
3. **Phase 2**: rust-analyzer diagnostics - Zero issues
4. **Phase 3**: Cross-file validation - No broken references
5. **Phase 4**: Per-file verification - Each file passes individually
6. **Phase 5**: Report generation with actual terminal outputs
7. **Phase 6**: Final verification (release build + tests compile)

**Invocation**:
```bash
Task(
  subagent_type: "general-purpose",
  description: "Karen code quality review",
  prompt: "Act as Karen agent from .claude/agents/karen.md. Review package <package-name> following ALL 6 phases. Include actual terminal outputs for each phase."
)
```

**Zero Tolerance Standard**:
- ✅ Zero rustc errors/warnings
- ✅ Zero clippy warnings (including pedantic/nursery)
- ✅ Zero unused imports or dead code
- ✅ All public APIs documented with `# Errors` and `# Panics`
- ✅ All financial values use `rust_decimal::Decimal` (never f64)
- ✅ Consistent patterns throughout codebase

**Blocking Requirement**:
If Karen finds issues:
1. **STOP** - Do not proceed to next phase (blocking failure)
2. **Fix Atomically** - Address each issue as atomic task (follow TaskMaster rules)
3. **Re-verify** - Run Karen again after ALL fixes applied
4. **Iterate** - Repeat fix → verify cycle until zero issues

**A phase is ONLY complete after Karen review passes with zero issues.**

---

## Agent Interaction Examples

### Example 1: Complex Feature with Research Needed

**User**: "Add support for Binance Futures integration"

**Claude Response**:
```
This feature requires comprehensive research. I recommend the 3-agent workflow:

STEP 1: Context Gatherer
- Research Binance Futures API (authentication, WebSocket feeds, margin types)
- Analyze existing Hyperliquid integration (reusable patterns)
- Evaluate design: New crate vs extend existing?
- Document edge cases: funding rates, position modes, liquidation

STEP 2: TaskMaster
- Read Context Report Section 6 (pre-defined scope)
- Generate atomic playbook with exact file paths
- Estimated: 8 tasks, ~400 LOC

STEP 3: Karen
- Review after implementation
- Enforce zero-tolerance quality

Proceed with Context Gatherer?
```

**User**: "Yes"

**Claude**: [Invokes Context Gatherer agent, generates `.claude/context/2025-10-02_binance-futures.md`]

### Example 2: Simple Feature, No Research Needed

**User**: "Add a timeout field to the bot configuration"

**Claude Response**:
```
This is a simple feature affecting 2 files. Skipping Context Gatherer.

Invoking TaskMaster to generate playbook...

[Generates .claude/playbooks/2025-10-02_add-bot-timeout.md with 3 atomic tasks:
 Task 1: Add timeout field to BotConfig struct (core/src/config.rs:25)
 Task 2: Use timeout in BotActor::new() (bot-orchestrator/src/bot_actor.rs:50)
 Task 3: Update Config.toml example (config/Config.toml:15)]

Ready to execute. Approve?
```

**User**: "Approved"

**Claude**: [Executes tasks, then invokes Karen for review]

### Example 3: TaskMaster Recommends Context Gatherer Mid-Stream

**User**: "Add support for options trading"

**Claude**: [Starts TaskMaster analysis, realizes complexity]

**Claude Response**:
```
While analyzing this request, I found:
- Options require new order types (calls, puts, strikes, expiry)
- Margin calculations differ significantly from spot/futures
- Multiple design patterns possible (extend OrderEvent vs new OptionsOrder type)

This requires architectural decisions. I recommend Context Gatherer:
1. Research options trading APIs (strike selection, Greeks calculation)
2. Analyze margin requirements vs existing risk management
3. Evaluate design patterns (integration points)
4. Document edge cases (assignment, exercise, expiry)

Proceed with Context Gatherer before TaskMaster?
```

---

## When to Use Each Agent

### Use Context Gatherer When:
- ✅ New exchange integration (research API, auth, rate limits)
- ✅ Adding asset type (futures, options, perpetuals - requires margin research)
- ✅ External crate evaluation needed (multiple options to compare)
- ✅ Architectural decision (multiple design patterns possible)
- ✅ User request is vague ("make it better", "add trading features")

### Use TaskMaster When:
- ✅ All medium/large features (3+ file changes)
- ✅ Any refactoring work
- ✅ Performance optimization
- ✅ Bug fixes affecting multiple files
- ✅ ALWAYS after Context Gatherer completes

### Use Karen When:
- ✅ **ALWAYS after every phase/playbook completion (MANDATORY)**
- ✅ Before marking any phase "complete"
- ✅ After fixing issues (re-run until zero issues)

### Skip All Agents When:
- ❌ Single-line changes (fix typo, remove unused import)
- ❌ Trivial updates (change constant value)
- ❌ Emergency hotfixes (but document after with playbook)
- ❌ Changes to <3 files with <5 lines each

## References

- **Barter-rs**: Event-driven architecture patterns (https://github.com/barter-rs/barter-rs)
- **Hyperliquid Docs**: API reference (https://hyperliquid.gitbook.io)
- **Alice Ryhl's Actor Guide**: Tokio channel patterns (https://ryhl.io/blog/actors-with-tokio/)
- **TimescaleDB**: Time-series best practices (https://docs.timescale.com)
