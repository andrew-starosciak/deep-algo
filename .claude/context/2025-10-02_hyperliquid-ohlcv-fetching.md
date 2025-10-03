# Context Report: Hyperliquid OHLCV Data Fetching for Backtesting

**Date**: 2025-10-02
**Agent**: Context Gatherer
**Status**: ✅ Complete
**TaskMaster Handoff**: ✅ Ready

---

## Section 1: Request Analysis

### User Request (Verbatim)
"Use the context gather agent to help me be able to pull ohlv by tokens from hyperliquid in order to run backtests we create. How can we access the latest ohlv data from hyperliquid to be used in our algo trading system. Can we create a command that can fetch data and backtest with it to test our algorithms. What is the best approach that aligns with our current system."

### Explicit Requirements
1. **OHLCV Data Fetching**: Pull OHLCV (Open/High/Low/Close/Volume) data from Hyperliquid by tokens
2. **Backtest Integration**: Use fetched data to run backtests
3. **CLI Command**: Create a command that can fetch data and backtest with it
4. **System Alignment**: Solution must align with current system architecture

### Implicit Requirements
1. **Data Storage**: Need to store fetched OHLCV data (CSV, Parquet, or Database)
2. **Multiple Tokens**: Support fetching data for different tokens/symbols
3. **Time Ranges**: Support specifying time ranges for historical data
4. **Interval Selection**: Support different intervals (1m, 5m, 1h, 1d, etc.)
5. **Data Validation**: Validate fetched data before use
6. **Rate Limiting**: Respect Hyperliquid API rate limits
7. **Pagination**: Handle Hyperliquid's 5000 candle limit with pagination
8. **Error Handling**: Graceful handling of missing data, API failures
9. **Unified Workflow**: Optionally combine fetch + backtest in one command
10. **Configuration**: Allow configuration of data source (CSV vs Parquet vs DB)

### Open Questions
1. **Storage Format**: CSV for simplicity or Parquet for performance? Or both?
2. **Database Integration**: Should we store in PostgreSQL TimescaleDB for production use?
3. **Command Design**: Separate `fetch-data` and `backtest` commands, or unified `backtest --fetch`?
4. **Data Caching**: Should we cache fetched data to avoid re-fetching?
5. **Bulk Downloads**: Should we support downloading from S3 bucket for older data?
6. **Multiple Tokens**: Fetch multiple symbols in parallel or sequentially?

### Success Criteria
- [ ] User can run `algo-trade fetch-data --symbol BTC --interval 1h --start 2025-01-01 --end 2025-02-01 --output data/btc.csv`
- [ ] Fetched CSV data is compatible with existing `HistoricalDataProvider::from_csv()`
- [ ] User can run `algo-trade backtest --data data/btc.csv --strategy ma_crossover`
- [ ] Optionally: `algo-trade backtest --symbol BTC --interval 1h --start 2025-01-01 --end 2025-02-01 --strategy ma_crossover` (fetch + backtest)
- [ ] Data fetching respects Hyperliquid rate limits (1200 req/min)
- [ ] Pagination handles time ranges > 5000 candles automatically
- [ ] Missing data gaps are logged with warnings

---

## Section 2: Codebase Context

### Existing Architecture

**Backtest Module** (`crates/backtest/src/data_provider.rs`):
- **Lines 9-62**: `HistoricalDataProvider` struct loads data from CSV
- **Line 24**: `from_csv(path: &str)` constructor expects format: `timestamp,symbol,open,high,low,close,volume`
- **Lines 31-37**: Parses CSV records into `MarketEvent::Bar` with `rust_decimal::Decimal` for OHLCV
- **Lines 50-55**: Sorts events by timestamp to ensure chronological order
- **Pattern**: Sequential iteration via `next_event()` (Lines 66-74)

**CLI Module** (`crates/cli/src/main.rs`):
- **Lines 11-34**: Uses `clap` with `Subcommand` enum for CLI structure
- **Lines 19-27**: Existing `Backtest` command takes `--data` (CSV path) and `--strategy`
- **Lines 82-121**: `run_backtest()` function loads CSV → creates system → runs backtest
- **Pattern**: Each subcommand has dedicated async handler function

**Hyperliquid Client** (`crates/exchange-hyperliquid/src/client.rs`):
- **Lines 7-29**: `HyperliquidClient` struct with rate limiter (20 req/sec = 1200 req/min)
- **Lines 35-41**: `get()` method for REST API calls with rate limiting
- **Lines 47-53**: `post()` method for POST requests with JSON body
- **Pattern**: Rate limiter waits before each request (`self.rate_limiter.until_ready().await`)
- **Missing**: No OHLCV/candles endpoint implementation yet

**Data Module** (`crates/data/src/`):
- **Lines 1-6** (lib.rs): Exports `DatabaseClient` and `ParquetStorage`
- **Lines 6-95** (database.rs): `DatabaseClient` with `insert_ohlcv_batch()` and `query_ohlcv()`
- **Lines 85-95** (database.rs): `OhlcvRecord` struct matches expected schema
- **Lines 14-91** (parquet_storage.rs): `ParquetStorage::write_ohlcv_batch()` writes Parquet files
- **Pattern**: Both storage backends use `OhlcvRecord` as common data model

**Configuration** (`crates/core/src/config.rs`):
- **Lines 1-45**: `AppConfig` with `HyperliquidConfig` containing `api_url` and `ws_url`
- **Line 40**: Default API URL: `https://api.hyperliquid.xyz`
- **Pattern**: TOML-based config via `serde`

### Current Patterns

1. **CLI Design**: Subcommand-based with clap (Run, Backtest, Server)
2. **Data Loading**: CSV → `HistoricalDataProvider` → `TradingSystem`
3. **Storage Options**: CSV (simple), Parquet (efficient), PostgreSQL (queryable)
4. **Financial Precision**: `rust_decimal::Decimal` for all OHLCV values
5. **Error Handling**: `anyhow::Result` with context propagation
6. **Async Pattern**: Tokio-based async/await throughout

### Integration Points

Files requiring modification:

1. **`crates/exchange-hyperliquid/src/client.rs:54`** (AFTER `post()` method)
   - Add `fetch_candles()` method to call Hyperliquid candleSnapshot endpoint
   - Handle pagination for >5000 candles
   - Parse response JSON to `OhlcvRecord` structs

2. **`crates/cli/src/main.rs:27`** (AFTER `Backtest` command)
   - Add new `FetchData` subcommand with parameters: symbol, interval, start, end, output
   - Add handler function `run_fetch_data()`

3. **`crates/cli/src/main.rs:53`** (UPDATE `Backtest` command)
   - Optional enhancement: Add `--fetch` flag or `--symbol/--interval/--start/--end` params
   - If present, fetch data first, then backtest

4. **`crates/data/src/lib.rs:6`** (NEW MODULE)
   - Create `csv_storage.rs` module with `CsvStorage::write_ohlcv()` function
   - Formats `OhlcvRecord` to CSV compatible with `HistoricalDataProvider::from_csv()`

### Constraints

**MUST Preserve**:
- ✅ CSV format must match existing `HistoricalDataProvider::from_csv()` parser (line 30-37)
- ✅ Use `rust_decimal::Decimal` for OHLCV values (not f64)
- ✅ Chronological sorting of data (existing backtest expects this)
- ✅ `DataProvider` trait interface unchanged

**CANNOT Break**:
- ❌ Existing `backtest` command must still work with `--data` CSV path
- ❌ Rate limiting must respect Hyperliquid's 1200 req/min limit
- ❌ Database schema for `ohlcv` table (already defined in database.rs)

---

## Section 3: External Research

### API Documentation Analysis

**Source**: Hyperliquid API Docs (https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint)

**Candle Snapshot Endpoint**:

**Endpoint**: POST `https://api.hyperliquid.xyz/info`

**Request Format**:
```json
{
  "type": "candleSnapshot",
  "req": {
    "coin": "BTC",
    "interval": "15m",
    "startTime": 1681923600000,
    "endTime": 1681924499999
  }
}
```

**Supported Intervals**:
- Minutes: `"1m"`, `"3m"`, `"5m"`, `"15m"`, `"30m"`
- Hours: `"1h"`, `"2h"`, `"4h"`, `"8h"`, `"12h"`
- Days/Weeks: `"1d"`, `"3d"`, `"1w"`, `"1M"`

**Critical Limitation**: Maximum 5000 candles returned per request

**Response Format**:
```json
[
  {
    "T": 1681924499999,
    "c": "29258.0",
    "h": "29309.0",
    "i": "15m",
    "l": "29250.0",
    "n": 189,
    "o": "29295.0",
    "s": "BTC",
    "t": 1681923600000,
    "v": "0.98639"
  }
]
```

**Field Mapping**:
- `t`: timestamp start (epoch millis)
- `T`: timestamp end (epoch millis)
- `o`: open price (string)
- `h`: high price (string)
- `l`: low price (string)
- `c`: close price (string)
- `v`: volume (string)
- `s`: symbol
- `i`: interval
- `n`: number of trades

**Rate Limiting**: 1200 requests/minute per IP (shared across all accounts)

### Pagination Strategy

Given Hyperliquid's 5000 candle limit:

1. Calculate total candles needed: `(end - start) / interval_millis`
2. If > 5000, split into multiple requests
3. Work backwards from `endTime`, fetching 5000 candles at a time
4. Combine results and deduplicate by timestamp

**Example** (1h interval, 1 year of data):
- Total candles: 365 * 24 = 8,760 candles
- Requests needed: ceil(8760 / 5000) = 2 requests
- Request 1: Last 5000 candles
- Request 2: Remaining 3760 candles

### CSV Format Compatibility

Existing parser expects: `timestamp,symbol,open,high,low,close,volume`

Output format:
```csv
timestamp,symbol,open,high,low,close,volume
2025-01-01T00:00:00Z,BTC,42000.5,42100.0,41900.0,42050.0,123.456
```

**Timestamp format**: ISO 8601 UTC (parseable by `DateTime<Utc>::parse()`)

---

## Section 4: Architectural Recommendations

### Proposed Design

**Two-Command Approach** (recommended):

```
User: algo-trade fetch-data --symbol BTC --interval 1h
      --start 2025-01-01T00:00:00Z --end 2025-02-01T00:00:00Z
      --output data/btc.csv
     ↓
CLI Handler: run_fetch_data()
     ↓
HyperliquidClient::fetch_candles()
  - Calculates total candles
  - Paginates if > 5000
  - Multiple API calls
  - Deduplicates by timestamp
     ↓
Vec<OhlcvRecord>
     ↓
CsvStorage::write_ohlcv()
  - Sorts chronologically
  - Formats to CSV
     ↓
data/btc.csv

User: algo-trade backtest --data data/btc.csv --strategy ma_crossover
     ↓
HistoricalDataProvider::from_csv()
     ↓
TradingSystem::run()
```

**Rationale**:
- **Separation of Concerns**: Data fetching and backtesting are distinct
- **Caching**: Fetched data persists, reusable for multiple backtests
- **Debugging**: Users can inspect CSV before backtesting
- **Flexibility**: Can fetch without backtesting, or backtest existing data

### Module Changes

#### **1. Create `crates/data/src/csv_storage.rs`** (NEW FILE, ~65 lines)

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

#### **2. Add Candle Fetching to `crates/exchange-hyperliquid/src/client.rs:54`** (~180 lines total)

```rust
use algo_trade_data::database::OhlcvRecord;
use chrono::{DateTime, Utc, Duration};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::collections::HashMap;

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
                "Unsupported interval: '{}'. Valid intervals: 1m, 3m, 5m, 15m, 30m, 1h, 2h, 4h, 8h, 12h, 1d, 3d, 1w, 1M",
                interval
            ),
        })
    }
}
```

#### **3. Add `FetchData` Command to CLI** (~55 lines in main.rs)

```rust
// In Commands enum (after Backtest):
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

// In match statement:
Commands::FetchData { symbol, interval, start, end, output } => {
    run_fetch_data(&symbol, &interval, &start, &end, &output).await?;
}

// Handler function:
async fn run_fetch_data(
    symbol: &str,
    interval: &str,
    start_str: &str,
    end_str: &str,
    output_path: &str,
) -> Result<()> {
    use algo_trade_exchange_hyperliquid::HyperliquidClient;
    use algo_trade_data::csv_storage::CsvStorage;

    tracing::info!("Fetching OHLCV data for {} ({} interval)", symbol, interval);

    // Parse timestamps
    let start: DateTime<Utc> = start_str.parse()
        .context("Invalid start time. Use ISO 8601 format (e.g., 2025-01-01T00:00:00Z)")?;
    let end: DateTime<Utc> = end_str.parse()
        .context("Invalid end time. Use ISO 8601 format (e.g., 2025-02-01T00:00:00Z)")?;

    if start >= end {
        anyhow::bail!("Start time must be before end time");
    }

    // Create client (no auth needed for public data)
    let api_url = std::env::var("HYPERLIQUID_API_URL")
        .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string());
    let client = HyperliquidClient::new("".to_string(), "".to_string(), api_url, false)?;

    // Fetch candles
    let records = client.fetch_candles(symbol, interval, start, end).await?;

    if records.is_empty() {
        tracing::warn!("No candle data returned. Symbol may not exist or date range may be invalid.");
        anyhow::bail!("No data fetched");
    }

    tracing::info!("Fetched {} candles, writing to {}", records.len(), output_path);

    // Write to CSV
    CsvStorage::write_ohlcv(output_path, &records)?;

    tracing::info!("✅ Successfully wrote {} candles to {}", records.len(), output_path);
    tracing::info!("You can now run: algo-trade backtest --data {} --strategy <strategy>", output_path);

    Ok(())
}
```

### Critical Decisions

**Decision 1: CSV as Primary Storage**
- **Rationale**: Existing `HistoricalDataProvider::from_csv()` works, minimizes changes
- **Trade-off**: CSV slower than Parquet, but simpler and human-readable
- **Impact**: Easy debugging, compatible with existing backtest

**Decision 2: Separate fetch-data Command**
- **Rationale**: Clear separation between acquisition and backtesting
- **Trade-off**: Two commands vs one, but more flexible
- **Impact**: Fetch once, backtest many times

**Decision 3: Backward Pagination**
- **Rationale**: Hyperliquid 5000 candle limit requires chunking
- **Trade-off**: Some complexity, but gets most recent data first
- **Impact**: Handles large time ranges gracefully

**Decision 4: HashMap Deduplication**
- **Rationale**: Overlapping pagination may return duplicates
- **Trade-off**: Extra memory, but O(1) dedup
- **Impact**: Guarantees unique candles

---

## Section 5: Edge Cases & Constraints

### Edge Cases

**EC1: Time Range > 5000 Candles**
- **Expected**: Automatic pagination
- **Test**: Fetch 1 year 1h data (8760 candles)

**EC2: Invalid Interval**
- **Expected**: Error with valid interval list
- **Test**: Try `--interval 5min`

**EC3: Start After End**
- **Expected**: Error "Start must be before end"
- **Test**: Swap start/end values

**EC4: Empty Response**
- **Expected**: Warning + error
- **Test**: Fetch future date

**EC5: Network Failure**
- **Expected**: Error with partial count
- **Test**: Mock network failure

**EC6: Duplicate Timestamps**
- **Expected**: Dedup via HashMap
- **Test**: Mock duplicate response

**EC7: Invalid Symbol**
- **Expected**: API error propagated
- **Test**: Use "BITCOIN" instead of "BTC"

**EC8: File Exists**
- **Expected**: Overwrite with warning
- **Test**: Run twice

**EC9: Data Gaps**
- **Expected**: Warning log
- **Test**: Mock gapped data

**EC10: Rate Limit**
- **Expected**: Queue, eventual completion
- **Test**: Concurrent fetches

### Constraints

**C1: Decimal Precision** - Must use `rust_decimal::Decimal` (not f64)
**C2: CSV Format** - Must match `HistoricalDataProvider::from_csv()` exactly
**C3: Rate Limiting** - Must respect 1200 req/min
**C4: Interval Support** - All 14 Hyperliquid intervals
**C5: Timestamp Format** - ISO 8601 UTC

---

## Section 6: TaskMaster Handoff Package

### MUST DO

1. ✅ Create `crates/data/src/csv_storage.rs` with `CsvStorage::write_ohlcv()` (~65 lines)
2. ✅ Add `pub mod csv_storage;` to `crates/data/src/lib.rs:3`
3. ✅ Add `pub use csv_storage::CsvStorage;` to `crates/data/src/lib.rs:8`
4. ✅ Add imports to `crates/exchange-hyperliquid/src/client.rs:1-6`
5. ✅ Add `fetch_candles()` to `HyperliquidClient` (client.rs:54, ~80 lines)
6. ✅ Add `fetch_candles_chunk()` to `HyperliquidClient` (~50 lines)
7. ✅ Add `interval_to_millis()` to `HyperliquidClient` (~25 lines)
8. ✅ Add `FetchData` to `Commands` enum (cli/src/main.rs:27, ~15 lines)
9. ✅ Add `FetchData` match arm (cli/src/main.rs:58)
10. ✅ Add `run_fetch_data()` handler (cli/src/main.rs:133, ~55 lines)
11. ✅ Update README.md with fetch-data examples (~40 lines)

### MUST NOT DO

1. ❌ DO NOT modify existing `Backtest` command
2. ❌ DO NOT change `HistoricalDataProvider::from_csv()` format
3. ❌ DO NOT use `f64` for OHLCV (use `Decimal`)
4. ❌ DO NOT bypass rate limiter
5. ❌ DO NOT hardcode API URL
6. ❌ DO NOT panic on errors
7. ❌ DO NOT implement S3 fetching in Phase 1

### Verification Criteria

**Per-Task**:
- [ ] Task 1-3: `cargo build -p algo-trade-data` succeeds
- [ ] Task 4-7: `cargo build -p algo-trade-exchange-hyperliquid` succeeds
- [ ] Task 8-10: `cargo build -p algo-trade-cli` succeeds
- [ ] Task 11: README renders correctly

**Integration**:
```bash
# Fetch 24 hours of 1h BTC data
cargo run -- fetch-data \
  --symbol BTC --interval 1h \
  --start 2025-01-01T00:00:00Z \
  --end 2025-01-02T00:00:00Z \
  --output /tmp/test.csv

# Verify CSV
head -5 /tmp/test.csv

# Run backtest
cargo run -- backtest --data /tmp/test.csv --strategy ma_crossover
```

**Karen Gates**:
- [ ] Phase 0: Compilation
- [ ] Phase 1: Clippy (all levels)
- [ ] Phase 6: Final verification

---

**Ready for TaskMaster**: Section 6 contains complete implementation package.
