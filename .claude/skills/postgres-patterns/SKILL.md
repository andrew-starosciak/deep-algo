---
name: postgres-patterns
description: PostgreSQL/TimescaleDB patterns for time-series trading data. Query optimization, hypertables, and batch operations.
---

# PostgreSQL/TimescaleDB Patterns

Patterns for high-performance time-series data storage in trading systems.

## When to Activate

- Creating database migrations
- Writing time-series queries
- Optimizing slow queries
- Designing schema for signals/trades
- Setting up hypertables

## TimescaleDB Essentials

### Creating Hypertables

```sql
-- Order book snapshots (high frequency)
CREATE TABLE orderbook_snapshots (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    bid_levels JSONB NOT NULL,
    ask_levels JSONB NOT NULL,
    imbalance DECIMAL(10, 8),
    PRIMARY KEY (timestamp, symbol, exchange)
);

SELECT create_hypertable('orderbook_snapshots', 'timestamp');

-- Compression for older data
ALTER TABLE orderbook_snapshots SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange'
);

SELECT add_compression_policy('orderbook_snapshots', INTERVAL '7 days');
```

### Retention Policies

```sql
-- Keep raw data for 30 days, compressed for 1 year
SELECT add_retention_policy('orderbook_snapshots', INTERVAL '30 days');

-- Continuous aggregate for longer-term analysis
CREATE MATERIALIZED VIEW orderbook_1h
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', timestamp) AS bucket,
    symbol,
    exchange,
    AVG(imbalance) AS avg_imbalance,
    MAX(imbalance) AS max_imbalance,
    MIN(imbalance) AS min_imbalance
FROM orderbook_snapshots
GROUP BY bucket, symbol, exchange;

SELECT add_continuous_aggregate_policy('orderbook_1h',
    start_offset => INTERVAL '2 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour');
```

## Trading Data Schema

### Signals and Predictions

```sql
CREATE TABLE signal_snapshots (
    timestamp TIMESTAMPTZ NOT NULL,
    signal_name TEXT NOT NULL,
    direction TEXT NOT NULL,  -- 'UP', 'DOWN', 'NEUTRAL'
    strength DECIMAL(5, 4) NOT NULL,
    confidence DECIMAL(5, 4),
    metadata JSONB,
    PRIMARY KEY (timestamp, signal_name)
);

SELECT create_hypertable('signal_snapshots', 'timestamp');
```

### Binary Trades

```sql
CREATE TABLE binary_trades (
    id SERIAL,
    timestamp TIMESTAMPTZ NOT NULL,
    market_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    shares DECIMAL(20, 8) NOT NULL,
    price_per_share DECIMAL(5, 4) NOT NULL,
    stake_usd DECIMAL(20, 2) NOT NULL,
    fee_usd DECIMAL(20, 4),
    -- Signal snapshot at decision time
    signals JSONB NOT NULL,
    -- Result (filled after settlement)
    settlement_outcome TEXT,
    pnl DECIMAL(20, 2),
    is_win BOOLEAN,
    PRIMARY KEY (timestamp, id)
);

SELECT create_hypertable('binary_trades', 'timestamp');
```

### Backtest Results

```sql
CREATE TABLE backtest_results (
    id SERIAL PRIMARY KEY,
    run_timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    strategy_name TEXT NOT NULL,
    start_date DATE NOT NULL,
    end_date DATE NOT NULL,
    -- Core metrics
    total_bets INTEGER NOT NULL,
    wins INTEGER NOT NULL,
    win_rate DECIMAL(5, 4) NOT NULL,
    -- Statistical metrics
    wilson_ci_lower DECIMAL(5, 4),
    wilson_ci_upper DECIMAL(5, 4),
    binomial_p_value DECIMAL(10, 8),
    -- Financial metrics
    total_pnl DECIMAL(20, 2),
    ev_per_bet DECIMAL(10, 4),
    kelly_fraction DECIMAL(5, 4),
    max_drawdown DECIMAL(10, 4),
    -- Parameters
    parameters JSONB
);

CREATE INDEX idx_backtest_strategy ON backtest_results (strategy_name, run_timestamp DESC);
```

## Query Patterns

### Time-Bounded Queries (Always Use!)

```sql
-- GOOD: Time filter first
SELECT * FROM orderbook_snapshots
WHERE timestamp > NOW() - INTERVAL '1 hour'
  AND symbol = 'BTCUSDT'
ORDER BY timestamp DESC
LIMIT 100;

-- BAD: Full table scan
SELECT * FROM orderbook_snapshots
WHERE symbol = 'BTCUSDT'
ORDER BY timestamp DESC;
```

### Latest Value Per Symbol

```sql
-- Get latest snapshot for each symbol
SELECT DISTINCT ON (symbol, exchange)
    timestamp, symbol, exchange, imbalance
FROM orderbook_snapshots
WHERE timestamp > NOW() - INTERVAL '5 minutes'
ORDER BY symbol, exchange, timestamp DESC;
```

### Time Bucket Aggregations

```sql
-- Aggregate imbalance by 5-minute buckets
SELECT
    time_bucket('5 minutes', timestamp) AS bucket,
    symbol,
    AVG(imbalance) AS avg_imbalance,
    MAX(imbalance) AS max_imbalance,
    COUNT(*) AS samples
FROM orderbook_snapshots
WHERE timestamp > NOW() - INTERVAL '1 day'
GROUP BY bucket, symbol
ORDER BY bucket DESC;
```

### Signal Correlation Analysis

```sql
-- Correlate signals with outcomes
WITH signal_outcomes AS (
    SELECT
        s.timestamp,
        s.direction,
        s.strength,
        CASE
            WHEN t.is_win THEN 1.0
            ELSE 0.0
        END AS outcome
    FROM signal_snapshots s
    JOIN binary_trades t ON
        t.timestamp BETWEEN s.timestamp AND s.timestamp + INTERVAL '15 minutes'
    WHERE s.signal_name = 'orderbook_imbalance'
      AND s.timestamp > NOW() - INTERVAL '30 days'
)
SELECT
    direction,
    COUNT(*) AS total,
    SUM(outcome) AS wins,
    AVG(outcome) AS win_rate,
    AVG(strength) AS avg_strength
FROM signal_outcomes
GROUP BY direction;
```

### Rolling Statistics

```sql
-- Rolling 100-bet win rate
SELECT
    timestamp,
    is_win,
    AVG(CASE WHEN is_win THEN 1.0 ELSE 0.0 END)
        OVER (ORDER BY timestamp ROWS BETWEEN 99 PRECEDING AND CURRENT ROW) AS rolling_win_rate
FROM binary_trades
WHERE timestamp > NOW() - INTERVAL '7 days'
ORDER BY timestamp;
```

## Rust Integration (sqlx)

### Batch Insert

```rust
pub async fn insert_snapshots_batch(
    pool: &PgPool,
    snapshots: &[OrderBookSnapshot],
) -> Result<()> {
    let mut tx = pool.begin().await?;

    for chunk in snapshots.chunks(100) {
        let timestamps: Vec<_> = chunk.iter().map(|s| s.timestamp).collect();
        let symbols: Vec<_> = chunk.iter().map(|s| &s.symbol).collect();
        let exchanges: Vec<_> = chunk.iter().map(|s| &s.exchange).collect();
        let imbalances: Vec<_> = chunk.iter().map(|s| s.imbalance).collect();

        sqlx::query!(
            r#"
            INSERT INTO orderbook_snapshots (timestamp, symbol, exchange, imbalance)
            SELECT * FROM UNNEST($1::timestamptz[], $2::text[], $3::text[], $4::decimal[])
            "#,
            &timestamps,
            &symbols as _,
            &exchanges as _,
            &imbalances as _,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
```

### Streaming Query

```rust
use futures::TryStreamExt;

pub async fn stream_snapshots(
    pool: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> impl Stream<Item = Result<OrderBookSnapshot>> {
    sqlx::query_as!(
        OrderBookSnapshot,
        r#"
        SELECT timestamp, symbol, exchange, imbalance
        FROM orderbook_snapshots
        WHERE timestamp BETWEEN $1 AND $2
        ORDER BY timestamp
        "#,
        start,
        end
    )
    .fetch(pool)
}
```

### Compile-Time Checked Queries

```rust
// This fails at compile time if schema doesn't match
let result = sqlx::query!(
    r#"
    SELECT
        timestamp,
        symbol,
        imbalance as "imbalance: Decimal"
    FROM orderbook_snapshots
    WHERE timestamp > $1
    LIMIT 1
    "#,
    start_time
)
.fetch_optional(pool)
.await?;
```

## Index Strategies

```sql
-- Time + symbol (most common query pattern)
CREATE INDEX idx_ob_time_symbol ON orderbook_snapshots (timestamp DESC, symbol);

-- For latest-per-symbol queries
CREATE INDEX idx_ob_symbol_time ON orderbook_snapshots (symbol, timestamp DESC);

-- For imbalance threshold queries
CREATE INDEX idx_ob_imbalance ON orderbook_snapshots (imbalance)
WHERE imbalance > 0.1 OR imbalance < -0.1;
```

## Performance Tips

1. **Always filter by time first** - TimescaleDB partitions by time
2. **Use hypertables** for all time-series data
3. **Batch inserts** - 100+ rows per INSERT
4. **Compress old data** - Automatic with policies
5. **Use continuous aggregates** for dashboards
6. **LIMIT queries** - Don't fetch unbounded results
7. **Use streaming** for large result sets

## Anti-Pattern Detection

```sql
-- Find slow queries
SELECT query, mean_exec_time, calls
FROM pg_stat_statements
WHERE mean_exec_time > 100
ORDER BY mean_exec_time DESC
LIMIT 10;

-- Check chunk sizes
SELECT hypertable_name, chunk_name, range_start, range_end
FROM timescaledb_information.chunks
WHERE hypertable_name = 'orderbook_snapshots'
ORDER BY range_start DESC
LIMIT 10;

-- Compression status
SELECT
    hypertable_name,
    before_compression_total_bytes,
    after_compression_total_bytes,
    compression_ratio
FROM hypertable_compression_stats('orderbook_snapshots');
```
