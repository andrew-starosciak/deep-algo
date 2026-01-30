# Performance Optimization

## Database Performance

### Batch Operations
Single inserts are slow. Always batch:

```rust
// BAD: Single inserts
for snapshot in snapshots {
    insert_snapshot(&pool, &snapshot).await?;
}

// GOOD: Batch insert
insert_snapshots_batch(&pool, &snapshots).await?;
```

### TimescaleDB Optimization
- Use hypertables for time-series data
- Enable compression for data >7 days old
- Create appropriate indexes

```sql
-- Hypertable with compression
SELECT create_hypertable('orderbook_snapshots', 'timestamp');
ALTER TABLE orderbook_snapshots SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange'
);
SELECT add_compression_policy('orderbook_snapshots', INTERVAL '7 days');
```

### Query Optimization
- Always filter by time range first
- Use appropriate indexes
- Limit result sets

```rust
// GOOD: Time-bounded query
sqlx::query!(
    "SELECT * FROM orderbook_snapshots
     WHERE timestamp > $1 AND timestamp < $2
     AND symbol = $3
     ORDER BY timestamp DESC
     LIMIT 1000",
    start_time, end_time, symbol
)
```

## Async Performance

### Concurrent Data Collection
```rust
use futures::future::join_all;

// Collect from multiple sources concurrently
let results = join_all(vec![
    fetch_orderbook(),
    fetch_funding_rate(),
    fetch_liquidations(),
]).await;
```

### Channel Sizing
```rust
// Size channels appropriately
let (tx, rx) = mpsc::channel(1000);  // Buffer for bursts
```

## Memory Optimization

### Streaming Large Datasets
```rust
// BAD: Load all into memory
let all_data: Vec<Record> = fetch_all().await?;

// GOOD: Stream and process
let mut stream = fetch_stream();
while let Some(record) = stream.next().await {
    process(record?);
}
```

### Pre-allocated Buffers
```rust
// Pre-allocate when size is known
let mut buffer = Vec::with_capacity(expected_size);
```

## Backtest Performance

### Parallel Backtests
```rust
use rayon::prelude::*;

// Run parameter sweep in parallel
let results: Vec<_> = param_combinations
    .par_iter()
    .map(|params| run_backtest(params))
    .collect();
```

### Caching Historical Data
- Load once, reuse across backtests
- Use memory-mapped files for large datasets
- Cache computed signals

## Profiling

```bash
# CPU profiling
cargo build --release
perf record ./target/release/binary
perf report

# Memory profiling
valgrind --tool=massif ./target/release/binary

# Flamegraph
cargo flamegraph --bin algo-trade-cli -- backtest
```

## Model Selection (Claude Agents)

**Haiku** - Use for:
- Simple code generation
- Quick reviews
- Lightweight tasks

**Sonnet** - Use for:
- Main development work
- Complex analysis
- Multi-file changes

**Opus** - Use for:
- Architectural decisions
- Deep statistical reasoning
- Complex debugging
