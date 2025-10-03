# Playbook: Hyperliquid OHLCV Data Fetching for Backtesting

**Date**: 2025-10-02
**Status**: Ready for Execution
**Context Report**: `.claude/context/2025-10-02_hyperliquid-ohlcv-fetching.md`

---

## User Request

> "Use the context gather agent to help me be able to pull ohlv by tokens from hyperliquid in order to run backtests we create. How can we access the latest ohlv data from hyperliquid to be used in our algo trading system. Can we create a command that can fetch data and backtest with it to test our algorithms. What is the best approach that aligns with our current system."

---

## Scope Boundaries

### MUST DO

- [x] Create `crates/data/src/csv_storage.rs` with `CsvStorage::write_ohlcv()` method
- [x] Export `CsvStorage` from `crates/data/src/lib.rs`
- [x] Add OHLCV fetching methods to `HyperliquidClient` (fetch_candles, fetch_candles_chunk, interval_to_millis)
- [x] Add `FetchData` CLI subcommand with args: symbol, interval, start, end, output
- [x] Add `run_fetch_data()` handler function to CLI
- [x] Update README.md with fetch-data usage examples
- [x] Ensure CSV format matches existing `HistoricalDataProvider::from_csv()` expectations
- [x] Implement pagination for >5000 candles (Hyperliquid limit)
- [x] Use `rust_decimal::Decimal` for all OHLCV values
- [x] Respect Hyperliquid rate limits (1200 req/min)

### MUST NOT DO

- ❌ DO NOT modify existing `Backtest` command signature
- ❌ DO NOT change `HistoricalDataProvider::from_csv()` CSV format expectations
- ❌ DO NOT use `f64` for OHLCV values (MUST use `rust_decimal::Decimal`)
- ❌ DO NOT bypass rate limiter
- ❌ DO NOT hardcode API URL (use environment variable or config)
- ❌ DO NOT panic on API errors (MUST return `anyhow::Result`)
- ❌ DO NOT implement S3 bucket fetching in this phase
- ❌ DO NOT modify `DataProvider` trait interface
- ❌ DO NOT break existing backtest functionality

---

## Atomic Tasks

### Task 1: Create CSV Storage Module

**File**: `crates/data/src/csv_storage.rs` (NEW FILE)
**Lines**: ~65
**Complexity**: LOW

**Action**: Create new module with `CsvStorage::write_ohlcv()` method that formats `OhlcvRecord` to CSV compatible with existing `HistoricalDataProvider::from_csv()`.

**Implementation**:
```rust
use anyhow::{Context, Result};
use crate::database::OhlcvRecord;
use std::fs::File;
use csv::Writer;

pub struct CsvStorage;

impl CsvStorage {
    /// Writes OHLCV records to CSV file compatible with HistoricalDataProvider
    ///
    /// Format: timestamp,symbol,open,high,low,close,volume
    ///
    /// # Errors
    /// Returns error if file cannot be created or writing fails
    pub fn write_ohlcv(path: &str, records: &[OhlcvRecord]) -> Result<()> {
        let file = File::create(path)
            .with_context(|| format!("Failed to create CSV file: {}", path))?;
        let mut writer = Writer::from_writer(file);

        // Write header
        writer.write_record(&["timestamp", "symbol", "open", "high", "low", "close", "volume"])?;

        // Sort records by timestamp (ascending) to match backtest expectations
        let mut sorted = records.to_vec();
        sorted.sort_by_key(|r| r.timestamp);

        // Write data rows
        for record in sorted {
            writer.write_record(&[
                record.timestamp.to_rfc3339(),  // ISO 8601 format
                record.symbol.clone(),
                record.open.to_string(),
                record.high.to_string(),
                record.low.to_string(),
                record.close.to_string(),
                record.volume.to_string(),
            ])?;
        }

        writer.flush()?;
        Ok(())
    }
}
```

**Verification**:
```bash
cargo check -p algo-trade-data
```

**Acceptance**:
- File exists at `crates/data/src/csv_storage.rs`
- `CsvStorage` struct defined
- `write_ohlcv()` method accepts `&[OhlcvRecord]`
- CSV format: `timestamp,symbol,open,high,low,close,volume`
- Timestamp uses ISO 8601 format (`.to_rfc3339()`)
- Records sorted chronologically
- No compilation errors

**Estimated Lines Changed**: 65

---

### Task 2: Export CSV Storage from Data Module

**File**: `crates/data/src/lib.rs`
**Location**: Lines 1-8
**Complexity**: LOW

**Action**: Add module declaration and public export for `csv_storage`.

**Changes**:
```rust
// Line 1: Add module
pub mod csv_storage;  // NEW
pub mod database;
pub mod parquet_storage;

// Line 6-8: Add export
pub use csv_storage::CsvStorage;  // NEW
pub use database::{DatabaseClient, OhlcvRecord};
pub use parquet_storage::ParquetStorage;
```

**Verification**:
```bash
cargo build -p algo-trade-data
```

**Acceptance**:
- `pub mod csv_storage;` declared
- `pub use csv_storage::CsvStorage;` exported
- Module compiles without errors

**Estimated Lines Changed**: 2

---

### Task 3: Add Imports to Hyperliquid Client

**File**: `crates/exchange-hyperliquid/src/client.rs`
**Location**: Lines 1-10 (add to imports section)
**Complexity**: LOW

**Action**: Add required imports for OHLCV fetching functionality.

**Changes**:
```rust
// Add these imports at the top of the file:
use algo_trade_data::database::OhlcvRecord;
use chrono::{DateTime, Utc, Duration};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::collections::HashMap;
```

**Verification**:
```bash
cargo check -p algo-trade-exchange-hyperliquid
```

**Acceptance**:
- All imports added
- No compilation errors
- No unused import warnings

**Estimated Lines Changed**: 5

---

### Task 4: Add interval_to_millis Helper Method

**File**: `crates/exchange-hyperliquid/src/client.rs`
**Location**: After existing methods (around line 54)
**Complexity**: LOW

**Action**: Add helper method to convert interval strings to milliseconds. This validates user input and supports all 14 Hyperliquid intervals.

**Implementation**:
```rust
impl HyperliquidClient {
    /// Converts interval string to milliseconds
    ///
    /// # Errors
    /// Returns error if interval is not supported
    fn interval_to_millis(interval: &str) -> Result<i64> {
        Ok(match interval {
            "1m" => 60 * 1000,
            "3m" => 3 * 60 * 1000,
            "5m" => 5 * 60 * 1000,
            "15m" => 15 * 60 * 1000,
            "30m" => 30 * 60 * 1000,
            "1h" => 60 * 60 * 1000,
            "2h" => 2 * 60 * 60 * 1000,
            "4h" => 4 * 60 * 60 * 1000,
            "8h" => 8 * 60 * 60 * 1000,
            "12h" => 12 * 60 * 60 * 1000,
            "1d" => 24 * 60 * 60 * 1000,
            "3d" => 3 * 24 * 60 * 60 * 1000,
            "1w" => 7 * 24 * 60 * 60 * 1000,
            "1M" => 30 * 24 * 60 * 60 * 1000,  // Approximate
            _ => anyhow::bail!(
                "Unsupported interval: '{}'. Valid: 1m, 3m, 5m, 15m, 30m, 1h, 2h, 4h, 8h, 12h, 1d, 3d, 1w, 1M",
                interval
            ),
        })
    }
}
```

**Verification**:
```bash
cargo check -p algo-trade-exchange-hyperliquid
```

**Acceptance**:
- Method converts all 14 intervals correctly
- Invalid intervals return error with helpful message
- No compilation errors

**Estimated Lines Changed**: 25

---

### Task 5: Add fetch_candles_chunk Helper Method

**File**: `crates/exchange-hyperliquid/src/client.rs`
**Location**: After `interval_to_millis()` method
**Complexity**: MEDIUM

**Action**: Add helper method to fetch single chunk of candles (up to 5000). Handles API call and response parsing.

**Implementation**:
```rust
impl HyperliquidClient {
    /// Fetches single chunk of candles (up to 5000)
    async fn fetch_candles_chunk(
        &self,
        symbol: &str,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let request_body = serde_json::json!({
            "type": "candleSnapshot",
            "req": {
                "coin": symbol,
                "interval": interval,
                "startTime": start.timestamp_millis(),
                "endTime": end.timestamp_millis(),
            }
        });

        let response = self.post("/info", request_body).await?;

        // Parse response array
        let candles = response.as_array()
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid response is not an array"))?;

        let mut records = Vec::new();
        for candle in candles {
            let timestamp_millis = candle["t"].as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing timestamp in candle data"))?;
            let timestamp = DateTime::from_timestamp_millis(timestamp_millis)
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp: {}", timestamp_millis))?;

            let record = OhlcvRecord {
                timestamp,
                symbol: candle["s"].as_str().unwrap_or(symbol).to_string(),
                exchange: "hyperliquid".to_string(),
                open: Decimal::from_str(candle["o"].as_str().unwrap_or("0"))
                    .with_context(|| "Failed to parse open price")?,
                high: Decimal::from_str(candle["h"].as_str().unwrap_or("0"))
                    .with_context(|| "Failed to parse high price")?,
                low: Decimal::from_str(candle["l"].as_str().unwrap_or("0"))
                    .with_context(|| "Failed to parse low price")?,
                close: Decimal::from_str(candle["c"].as_str().unwrap_or("0"))
                    .with_context(|| "Failed to parse close price")?,
                volume: Decimal::from_str(candle["v"].as_str().unwrap_or("0"))
                    .with_context(|| "Failed to parse volume")?,
            };
            records.push(record);
        }

        Ok(records)
    }
}
```

**Verification**:
```bash
cargo check -p algo-trade-exchange-hyperliquid
```

**Acceptance**:
- Method calls Hyperliquid candleSnapshot endpoint
- Parses JSON response to `Vec<OhlcvRecord>`
- Uses `Decimal::from_str()` for all prices/volumes
- Proper error handling with context
- No compilation errors

**Estimated Lines Changed**: 50

---

### Task 6: Add fetch_candles Main Method with Pagination

**File**: `crates/exchange-hyperliquid/src/client.rs`
**Location**: After `interval_to_millis()` method (before `fetch_candles_chunk`)
**Complexity**: HIGH

**Action**: Add main `fetch_candles()` method with automatic pagination logic to handle Hyperliquid's 5000 candle limit.

**Implementation**:
```rust
impl HyperliquidClient {
    /// Fetches OHLCV candles from Hyperliquid API with automatic pagination
    ///
    /// Handles Hyperliquid's 5000 candle limit by splitting large requests
    /// into multiple API calls and deduplicating results.
    ///
    /// # Arguments
    /// * `symbol` - Trading symbol (e.g., "BTC" for perpetuals)
    /// * `interval` - Candle interval (e.g., "1h", "1d")
    /// * `start` - Start time (inclusive)
    /// * `end` - End time (inclusive)
    ///
    /// # Errors
    /// Returns error if API request fails or response parsing fails
    pub async fn fetch_candles(
        &self,
        symbol: &str,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let interval_millis = Self::interval_to_millis(interval)?;
        let total_candles = ((end.timestamp_millis() - start.timestamp_millis()) / interval_millis) as usize;

        tracing::info!(
            "Fetching {} candles for {} (interval: {}, {} to {})",
            total_candles, symbol, interval, start, end
        );

        const MAX_CANDLES_PER_REQUEST: usize = 5000;
        let mut all_records = HashMap::new();  // Deduplicate by timestamp

        if total_candles <= MAX_CANDLES_PER_REQUEST {
            // Single request
            let records = self.fetch_candles_chunk(symbol, interval, start, end).await?;
            for record in records {
                all_records.insert(record.timestamp, record);
            }
        } else {
            // Multiple requests (pagination backward from end)
            let num_requests = (total_candles + MAX_CANDLES_PER_REQUEST - 1) / MAX_CANDLES_PER_REQUEST;
            tracing::info!("Requires {} paginated requests (Hyperliquid limit: 5000 candles/request)", num_requests);

            let mut current_end = end;
            for i in 0..num_requests {
                let chunk_duration = Duration::milliseconds(interval_millis * MAX_CANDLES_PER_REQUEST as i64);
                let chunk_start = current_end - chunk_duration;
                let chunk_start = chunk_start.max(start);  // Don't go before requested start

                tracing::debug!("Request {}/{}: {} to {}", i + 1, num_requests, chunk_start, current_end);

                let records = self.fetch_candles_chunk(symbol, interval, chunk_start, current_end).await?;
                for record in records {
                    all_records.insert(record.timestamp, record);
                }

                current_end = chunk_start;
                if current_end <= start {
                    break;
                }
            }
        }

        // Convert to sorted vector
        let mut records: Vec<OhlcvRecord> = all_records.into_values().collect();
        records.sort_by_key(|r| r.timestamp);

        tracing::info!("Fetched {} unique candles for {}", records.len(), symbol);

        // Warn if significantly fewer candles than expected (possible data gaps)
        if records.len() < total_candles * 9 / 10 {
            tracing::warn!(
                "Expected ~{} candles but got {}. There may be data gaps.",
                total_candles, records.len()
            );
        }

        Ok(records)
    }
}
```

**Verification**:
```bash
cargo build -p algo-trade-exchange-hyperliquid
```

**Acceptance**:
- Method handles single request (<= 5000 candles)
- Method paginates for > 5000 candles
- Deduplicates via HashMap
- Sorts results chronologically
- Logs warnings for data gaps
- No compilation errors

**Estimated Lines Changed**: 80

---

### Task 7: Add FetchData CLI Command

**File**: `crates/cli/src/main.rs`
**Location**: After `Backtest` command in `Commands` enum (around line 27)
**Complexity**: LOW

**Action**: Add new `FetchData` subcommand variant with required parameters.

**Changes**:
```rust
#[derive(Subcommand)]
enum Commands {
    // ... existing Run, Backtest, Server commands ...

    /// Fetch historical OHLCV data from Hyperliquid
    FetchData {
        /// Symbol/coin to fetch (e.g., "BTC", "ETH")
        #[arg(short, long)]
        symbol: String,

        /// Candle interval (1m, 5m, 15m, 1h, 4h, 1d, etc.)
        #[arg(short, long)]
        interval: String,

        /// Start time in ISO 8601 format (e.g., "2025-01-01T00:00:00Z")
        #[arg(short, long)]
        start: String,

        /// End time in ISO 8601 format (e.g., "2025-02-01T00:00:00Z")
        #[arg(short, long)]
        end: String,

        /// Output CSV file path
        #[arg(short, long)]
        output: String,
    },
}
```

**Verification**:
```bash
cargo check -p algo-trade-cli
```

**Acceptance**:
- `FetchData` variant added to enum
- All 5 fields present: symbol, interval, start, end, output
- Doc comments added
- No compilation errors

**Estimated Lines Changed**: 20

---

### Task 8: Add FetchData Match Arm

**File**: `crates/cli/src/main.rs`
**Location**: In match statement (around line 58)
**Complexity**: LOW

**Action**: Add match arm to handle `FetchData` command.

**Changes**:
```rust
// In the match statement:
Commands::FetchData { symbol, interval, start, end, output } => {
    run_fetch_data(&symbol, &interval, &start, &end, &output).await?;
}
```

**Verification**:
```bash
cargo check -p algo-trade-cli
```

**Acceptance**:
- Match arm added for `FetchData`
- Calls `run_fetch_data()` with all parameters
- No compilation errors

**Estimated Lines Changed**: 3

---

### Task 9: Add run_fetch_data Handler Function

**File**: `crates/cli/src/main.rs`
**Location**: After `run_server()` function (around line 133)
**Complexity**: MEDIUM

**Action**: Implement handler function that fetches data and writes CSV.

**Implementation**:
```rust
async fn run_fetch_data(
    symbol: &str,
    interval: &str,
    start_str: &str,
    end_str: &str,
    output_path: &str,
) -> Result<()> {
    use algo_trade_exchange_hyperliquid::HyperliquidClient;
    use algo_trade_data::csv_storage::CsvStorage;
    use chrono::DateTime;

    tracing::info!("Fetching OHLCV data for {} ({} interval)", symbol, interval);

    // Parse timestamps
    let start: DateTime<Utc> = start_str.parse()
        .context("Invalid start time. Use ISO 8601 format (e.g., 2025-01-01T00:00:00Z)")?;
    let end: DateTime<Utc> = end_str.parse()
        .context("Invalid end time. Use ISO 8601 format (e.g., 2025-02-01T00:00:00Z)")?;

    if start >= end {
        anyhow::bail!("Start time must be before end time");
    }

    // Create client (no auth needed for public candle data)
    let api_url = std::env::var("HYPERLIQUID_API_URL")
        .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string());

    // Use dummy credentials for public data endpoint
    let client = HyperliquidClient::new(
        "".to_string(),
        "".to_string(),
        api_url,
        false,  // testnet
    )?;

    // Fetch candles
    let records = client.fetch_candles(symbol, interval, start, end).await?;

    if records.is_empty() {
        tracing::warn!("No candle data returned. Symbol may not exist or date range may be invalid.");
        anyhow::bail!("No data fetched for {} {}", symbol, interval);
    }

    tracing::info!("Fetched {} candles, writing to {}", records.len(), output_path);

    // Write to CSV
    CsvStorage::write_ohlcv(output_path, &records)?;

    tracing::info!("✅ Successfully wrote {} candles to {}", records.len(), output_path);
    tracing::info!("You can now run: algo-trade backtest --data {} --strategy <strategy>", output_path);

    Ok(())
}
```

**Verification**:
```bash
cargo build -p algo-trade-cli
```

**Acceptance**:
- Function parses ISO 8601 timestamps
- Validates start < end
- Creates HyperliquidClient
- Calls `fetch_candles()`
- Writes CSV via `CsvStorage::write_ohlcv()`
- Proper error handling
- Helpful logging
- No compilation errors

**Estimated Lines Changed**: 55

---

### Task 10: Update README with Usage Examples

**File**: `README.md`
**Location**: After "Running" section (around line 95)
**Complexity**: LOW

**Action**: Add new section documenting `fetch-data` command with examples.

**Changes**:
```markdown
## Fetching Historical Data

Fetch OHLCV candle data from Hyperliquid for backtesting:

```bash
# Fetch 1 month of hourly BTC data
cargo run -p algo-trade-cli -- fetch-data \
  --symbol BTC \
  --interval 1h \
  --start 2025-01-01T00:00:00Z \
  --end 2025-02-01T00:00:00Z \
  --output data/btc_jan2025.csv

# Fetch 1 week of 5-minute ETH data
cargo run -p algo-trade-cli -- fetch-data \
  --symbol ETH \
  --interval 5m \
  --start 2025-01-15T00:00:00Z \
  --end 2025-01-22T00:00:00Z \
  --output data/eth_week.csv

# Fetch daily SOL data for 3 months
cargo run -p algo-trade-cli -- fetch-data \
  --symbol SOL \
  --interval 1d \
  --start 2024-10-01T00:00:00Z \
  --end 2025-01-01T00:00:00Z \
  --output data/sol_q4.csv
```

**Supported intervals**: `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`, `8h`, `12h`, `1d`, `3d`, `1w`, `1M`

**Note**: Hyperliquid limits responses to 5000 candles. Larger requests are automatically paginated.

Then run backtest with fetched data:

```bash
cargo run -p algo-trade-cli -- backtest \
  --data data/btc_jan2025.csv \
  --strategy ma_crossover
```
```

**Verification**:
- README renders correctly
- Examples are accurate
- All intervals listed

**Acceptance**:
- New section added
- 3+ usage examples provided
- Supported intervals documented
- Pagination note included
- Renders correctly in Markdown viewer

**Estimated Lines Changed**: 45

---

### Task 11: Add csv Dependency

**File**: `crates/data/Cargo.toml`
**Location**: In `[dependencies]` section
**Complexity**: LOW

**Action**: Add `csv` crate dependency for CSV writing.

**Changes**:
```toml
[dependencies]
# ... existing dependencies ...
csv = "1.3"
```

**Verification**:
```bash
cargo build -p algo-trade-data
```

**Acceptance**:
- `csv = "1.3"` added to dependencies
- Builds successfully

**Estimated Lines Changed**: 1

---

## Verification Checklist

### Per-Task Verification
- [ ] Task 1: `cargo check -p algo-trade-data` passes
- [ ] Task 2: `cargo build -p algo-trade-data` passes
- [ ] Task 3: `cargo check -p algo-trade-exchange-hyperliquid` passes
- [ ] Task 4: `cargo check -p algo-trade-exchange-hyperliquid` passes
- [ ] Task 5: `cargo check -p algo-trade-exchange-hyperliquid` passes
- [ ] Task 6: `cargo build -p algo-trade-exchange-hyperliquid` passes
- [ ] Task 7: `cargo check -p algo-trade-cli` passes
- [ ] Task 8: `cargo check -p algo-trade-cli` passes
- [ ] Task 9: `cargo build -p algo-trade-cli` passes
- [ ] Task 10: README.md renders correctly
- [ ] Task 11: `cargo build -p algo-trade-data` passes

### Full Integration Verification

```bash
# 1. Build entire workspace
cargo build --workspace

# 2. Test help text
cargo run -p algo-trade-cli -- fetch-data --help

# 3. Fetch small dataset (24 hours of 1h BTC candles)
cargo run -p algo-trade-cli -- fetch-data \
  --symbol BTC \
  --interval 1h \
  --start 2025-01-01T00:00:00Z \
  --end 2025-01-02T00:00:00Z \
  --output /tmp/btc_test.csv

# 4. Verify CSV format
head -5 /tmp/btc_test.csv
# Expected: header line + ~24 data rows

# 5. Verify CSV loads in backtest
cargo run -p algo-trade-cli -- backtest \
  --data /tmp/btc_test.csv \
  --strategy ma_crossover

# 6. Run clippy
cargo clippy --workspace --all-targets -- -D warnings

# 7. Run tests
cargo test --workspace
```

### Karen Quality Gate (MANDATORY)

**After ALL tasks completed**:

```bash
# Invoke Karen agent for comprehensive review
Task(
  subagent_type: "general-purpose",
  description: "Karen code quality review - OHLCV fetching",
  prompt: "Act as Karen agent from .claude/agents/karen.md. Review packages algo-trade-data, algo-trade-exchange-hyperliquid, and algo-trade-cli following ALL 6 phases. Include actual terminal outputs for each phase."
)
```

**Karen Success Criteria**:
- [ ] Phase 0: `cargo build --workspace` succeeds
- [ ] Phase 1: Zero clippy warnings (default + pedantic + nursery)
- [ ] Phase 2: Zero rust-analyzer diagnostics
- [ ] Phase 3: All cross-file references valid
- [ ] Phase 4: Each modified file compiles individually
- [ ] Phase 5: Complete report with terminal outputs
- [ ] Phase 6: Release build + tests compile

**If Karen Finds Issues**:
1. STOP - Do not mark playbook complete
2. Document all findings
3. Fix each issue atomically
4. Re-run Karen
5. Iterate until zero issues

**Playbook is ONLY complete after Karen review passes.**

---

## Rollback Plan

If verification fails at any task:

1. **Identify failing task** from error message
2. **Revert changes** for that task:
   ```bash
   git checkout -- <failed-file>
   ```
3. **Review Context Report** Section 6 for that specific task
4. **Re-implement** with corrected approach
5. **Re-verify** before proceeding to next task

If integration verification fails:

1. **Revert all changes**:
   ```bash
   git checkout -- crates/data/src/csv_storage.rs
   git checkout -- crates/data/src/lib.rs
   git checkout -- crates/exchange-hyperliquid/src/client.rs
   git checkout -- crates/cli/src/main.rs
   git checkout -- README.md
   git checkout -- crates/data/Cargo.toml
   ```
2. **Review error logs** to identify root cause
3. **Update playbook** with lessons learned
4. **Re-execute** from Task 1

---

## Task Dependencies

```
Task 11 (csv dep) ──→ Task 1 (csv_storage.rs)
                  └──→ Task 2 (lib.rs export)

Task 3 (imports) ──→ Task 4 (interval_to_millis)
                 ├──→ Task 5 (fetch_candles_chunk)
                 └──→ Task 6 (fetch_candles)

Task 7 (FetchData enum) ──→ Task 8 (match arm)
                        └──→ Task 9 (handler)

Task 2 ──┬──→ Task 9 (handler uses CsvStorage)
Task 6 ──┘

All tasks ──→ Task 10 (README)
          └──→ Karen Review
```

**Execution Order**: 11 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → Verify → Karen

---

## Estimated Completion

**Total Lines of Code**: ~349
**Estimated Time**: ~4.5 hours
**Risk Level**: MEDIUM-HIGH

**Highest Risk Areas**:
1. Task 6 (fetch_candles pagination) - Complex logic
2. Task 5 (API response parsing) - Depends on Hyperliquid format
3. Task 1 (CSV format) - Must match existing parser exactly

---

**Status**: Ready for execution
**Next Step**: Execute Task 11
