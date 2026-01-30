# Phase 1: Data Collection Guide

This guide covers setting up and running the real-time data collection system for the Bitcoin 15-minute Polymarket trading engine.

## Quick Start with Docker

```bash
# 1. Set up secrets
echo "your_secure_password" > secrets/db_password.txt

# 2. Create .env file
cp .env.example .env
# Edit .env and set DB_PASSWORD to match secrets/db_password.txt

# 3. Start database
docker compose up -d timescaledb

# 4. Wait for database to be ready
docker compose logs -f timescaledb  # Wait for "database system is ready"

# 5. Start signal collection (5 minute test)
COLLECT_DURATION=5m docker compose --profile collect up signal-collector

# 6. Check data health
curl http://localhost:8080/api/data/health
```

For production collection:
```bash
# Collect all sources for 24 hours
docker compose --profile collect up -d signal-collector

# Monitor logs
docker compose logs -f signal-collector

# Check database records
docker compose exec timescaledb psql -U postgres -d algo_trade \
  -c "SELECT COUNT(*) FROM orderbook_snapshots;"
```

---

## Overview

Phase 1 implements data collection from multiple sources:

| Source | Data Type | Frequency | Exchange/API |
|--------|-----------|-----------|--------------|
| Order Book | 20-level depth snapshots | 1/second | Binance Futures |
| Funding Rates | Predicted funding with percentile/z-score | Real-time | Binance Futures |
| Liquidations | Individual events + rolling aggregates | Real-time | Binance Futures |
| Polymarket Odds | Yes/No prices for BTC markets | Every 30s | Polymarket CLOB |
| News | Categorized with urgency scoring | Every 60s | CryptoPanic |

## Prerequisites

### 1. TimescaleDB

The system requires TimescaleDB (PostgreSQL with time-series extensions).

**Using Docker (recommended):**
```bash
docker run -d --name timescaledb \
  -p 5432:5432 \
  -e POSTGRES_PASSWORD=your_password \
  -e POSTGRES_DB=trading \
  timescale/timescaledb:latest-pg16
```

**Verify installation:**
```bash
psql -h localhost -U postgres -d trading -c "SELECT extversion FROM pg_extension WHERE extname = 'timescaledb';"
```

### 2. API Keys

| Service | Required | Purpose | Get Key |
|---------|----------|---------|---------|
| Binance | No | Public WebSocket streams | N/A |
| CryptoPanic | Yes (for news) | News aggregation | [cryptopanic.com/developers/api](https://cryptopanic.com/developers/api/) |
| Polymarket | No | Public CLOB API | N/A |

### 3. Rust Toolchain

```bash
rustup update stable
cargo --version  # Should be 1.75+
```

## Environment Setup

### 1. Create `.env` file

```bash
cp .env.example .env
```

### 2. Configure environment variables

Edit `.env`:
```bash
# Database (REQUIRED)
DATABASE_URL=postgres://postgres:your_password@localhost:5432/trading

# CryptoPanic API (optional, for news collection)
CRYPTOPANIC_API_KEY=your_api_key_here

# Logging
RUST_LOG=info
```

## Database Setup

### 1. Run migrations

Apply the Phase 1 schema:

```bash
psql $DATABASE_URL -f scripts/migrations/V001__phase1_tables.sql
```

### 2. Verify tables

```bash
psql $DATABASE_URL -c "\dt"
```

Expected tables:
- `orderbook_snapshots`
- `funding_rates`
- `liquidations`
- `liquidation_aggregates`
- `polymarket_odds`
- `news_events`
- `binary_trades`

### 3. Verify hypertables

```bash
psql $DATABASE_URL -c "SELECT hypertable_name FROM timescaledb_information.hypertables;"
```

## Running Data Collection

### Basic Usage

Collect all data sources for 24 hours:
```bash
cargo run -p algo-trade-cli -- collect-signals --duration 24h
```

### Options

```bash
cargo run -p algo-trade-cli -- collect-signals \
    --duration 1h \                    # Collection duration (1h, 24h, 7d)
    --sources orderbook,funding \      # Specific sources only
    --symbol btcusdt \                 # Trading pair
    --db-url postgres://...            # Override DATABASE_URL
```

### Available Sources

| Source | Flag | Description |
|--------|------|-------------|
| `orderbook` | Order book snapshots | Binance depth stream |
| `funding` | Funding rates | Binance mark price |
| `liquidations` | Liquidation events | Binance force orders |
| `polymarket` | Polymarket odds | CLOB API polling |
| `news` | News events | CryptoPanic API |

### Examples

**Collect only market data (no API keys needed):**
```bash
cargo run -p algo-trade-cli -- collect-signals \
    --duration 1h \
    --sources orderbook,funding,liquidations
```

**Collect everything for 7 days:**
```bash
RUST_LOG=info cargo run --release -p algo-trade-cli -- collect-signals \
    --duration 7d
```

**Test run (5 minutes):**
```bash
RUST_LOG=debug cargo run -p algo-trade-cli -- collect-signals \
    --duration 5m \
    --sources orderbook
```

## Monitoring

### Health Endpoint

Start the web API server:
```bash
cargo run -p algo-trade-cli -- server
```

Check data health:
```bash
curl http://localhost:8080/api/data/health | jq
```

**Example response:**
```json
{
  "status": "healthy",
  "timestamp": "2026-01-30T12:00:00Z",
  "sources": [
    {
      "source": "orderbook_snapshots",
      "last_record": "2026-01-30T11:59:55Z",
      "records_last_hour": 3600,
      "staleness_seconds": 5,
      "status": "healthy"
    },
    {
      "source": "funding_rates",
      "last_record": "2026-01-30T11:59:50Z",
      "records_last_hour": 720,
      "staleness_seconds": 10,
      "status": "healthy"
    }
  ],
  "summary": {
    "healthy": 6,
    "degraded": 0,
    "unhealthy": 0,
    "total_records_last_hour": 10000
  }
}
```

### Health Status Thresholds

| Source | Healthy | Degraded | Unhealthy |
|--------|---------|----------|-----------|
| Order Book | < 10s | < 60s | > 60s |
| Funding Rates | < 30s | < 120s | > 120s |
| Liquidations | < 5min | < 15min | > 15min |
| Polymarket | < 60s | < 180s | > 180s |
| News | < 2min | < 10min | > 10min |

### Database Queries

**Check record counts:**
```sql
SELECT
    'orderbook_snapshots' as table_name,
    COUNT(*) as total,
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '1 hour') as last_hour
FROM orderbook_snapshots
UNION ALL
SELECT
    'funding_rates',
    COUNT(*),
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '1 hour')
FROM funding_rates
UNION ALL
SELECT
    'liquidations',
    COUNT(*),
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '1 hour')
FROM liquidations;
```

**Check for data gaps (order book):**
```sql
WITH gaps AS (
    SELECT
        timestamp,
        LAG(timestamp) OVER (ORDER BY timestamp) as prev_timestamp,
        EXTRACT(EPOCH FROM (timestamp - LAG(timestamp) OVER (ORDER BY timestamp))) as gap_seconds
    FROM orderbook_snapshots
    WHERE timestamp > NOW() - INTERVAL '1 hour'
)
SELECT * FROM gaps WHERE gap_seconds > 5 ORDER BY gap_seconds DESC LIMIT 10;
```

**Latest funding rate with statistics:**
```sql
SELECT
    symbol,
    funding_rate,
    rate_percentile,
    rate_zscore,
    CASE
        WHEN rate_zscore > 2 THEN 'Extreme High'
        WHEN rate_zscore < -2 THEN 'Extreme Low'
        ELSE 'Normal'
    END as signal
FROM funding_rates
WHERE symbol = 'BTCUSDT'
ORDER BY timestamp DESC
LIMIT 1;
```

## Troubleshooting

### Connection Issues

**WebSocket disconnects:**
- The collectors automatically reconnect with exponential backoff
- Check `RUST_LOG=debug` for connection status
- Verify network connectivity to Binance

**Database connection errors:**
```bash
# Test connection
psql $DATABASE_URL -c "SELECT 1;"

# Check pool status
RUST_LOG=sqlx=debug cargo run -p algo-trade-cli -- collect-signals --duration 1m
```

### Missing Data

**No order book data:**
```bash
# Check WebSocket connectivity
websocat "wss://fstream.binance.com/stream?streams=btcusdt@depth20@100ms"
```

**No news data:**
- Verify `CRYPTOPANIC_API_KEY` is set
- Check API quota at [cryptopanic.com](https://cryptopanic.com)

**No Polymarket data:**
- Polymarket may not have active BTC markets
- Check market discovery: the collector searches for "Bitcoin" or "BTC" in market questions

### Performance

**High CPU usage:**
- Use release builds: `cargo run --release -p algo-trade-cli -- collect-signals`
- Reduce sources if not all are needed

**Database growing too fast:**
- Compression policies are set to 7-30 days
- Run manual compression:
  ```sql
  SELECT compress_chunk(c) FROM show_chunks('orderbook_snapshots') c;
  ```

### Logs

**Enable debug logging:**
```bash
RUST_LOG=debug cargo run -p algo-trade-cli -- collect-signals --duration 5m
```

**Filter by component:**
```bash
RUST_LOG=algo_trade_signals::collector=debug cargo run -p algo-trade-cli -- collect-signals
```

## Docker Deployment

### Services

| Service | Description | Profile |
|---------|-------------|---------|
| `timescaledb` | TimescaleDB database | default |
| `app` | Web API + Trading daemon | default |
| `signal-collector` | Data collection | `collect` |

### Environment Variables

Set these in `.env`:

```bash
# Required
DB_PASSWORD=your_secure_password

# Optional - Signal Collection
COLLECT_DURATION=24h                    # How long to collect
COLLECT_SOURCES=orderbook,funding,liquidations  # Which sources
CRYPTOPANIC_API_KEY=your_key            # For news collection

# Optional - Tuning
TS_TUNE_MEMORY=4GB
TS_TUNE_NUM_CPUS=4
RUST_LOG=info
```

### Running Signal Collection

**Start database only:**
```bash
docker compose up -d timescaledb
```

**Start signal collector (foreground for testing):**
```bash
COLLECT_DURATION=5m docker compose --profile collect up signal-collector
```

**Start signal collector (background for production):**
```bash
docker compose --profile collect up -d signal-collector
```

**View logs:**
```bash
docker compose logs -f signal-collector
```

**Stop collection:**
```bash
docker compose --profile collect down signal-collector
```

### Database Access

**Connect to database:**
```bash
docker compose exec timescaledb psql -U postgres -d algo_trade
```

**Check table counts:**
```bash
docker compose exec timescaledb psql -U postgres -d algo_trade -c "
SELECT
  'orderbook_snapshots' as table_name, COUNT(*) as count FROM orderbook_snapshots
UNION ALL SELECT 'funding_rates', COUNT(*) FROM funding_rates
UNION ALL SELECT 'liquidations', COUNT(*) FROM liquidations;
"
```

### Rebuilding After Code Changes

```bash
docker compose build signal-collector
docker compose --profile collect up -d signal-collector
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        collect-signals CLI                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐           │
│  │  OrderBook   │  │   Funding    │  │ Liquidation  │           │
│  │  Collector   │  │  Collector   │  │  Collector   │           │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘           │
│         │                 │                 │                    │
│         ▼                 ▼                 ▼                    │
│  ┌──────────────────────────────────────────────────┐           │
│  │              Binance Futures WebSocket            │           │
│  │     wss://fstream.binance.com/stream?streams=    │           │
│  └──────────────────────────────────────────────────┘           │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐                             │
│  │    Odds      │  │    News      │                             │
│  │  Collector   │  │  Collector   │                             │
│  └──────┬───────┘  └──────┬───────┘                             │
│         │                 │                                      │
│         ▼                 ▼                                      │
│  ┌─────────────┐  ┌─────────────┐                               │
│  │ Polymarket  │  │ CryptoPanic │                               │
│  │  CLOB API   │  │    API      │                               │
│  └─────────────┘  └─────────────┘                               │
│                                                                  │
├─────────────────────────────────────────────────────────────────┤
│                     Database Writers (Actors)                    │
│         Batch inserts: 100 records or 5 second flush            │
├─────────────────────────────────────────────────────────────────┤
│                         TimescaleDB                              │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐   │
│  │ orderbook  │ │  funding   │ │liquidation │ │ polymarket │   │
│  │ _snapshots │ │  _rates    │ │    s       │ │   _odds    │   │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Next Steps

After collecting sufficient data (recommended: 24+ hours), proceed to:

1. **Phase 2: Signal Development** - Transform raw data into trading signals
2. **Signal Validation** - Statistical validation of signal predictive power

```bash
# Validate signal quality (after Phase 2)
cargo run -p algo-trade-cli -- validate-signals --start 2025-01-01 --end 2025-01-28
```

## Reference

- [Binance Futures WebSocket API](https://binance-docs.github.io/apidocs/futures/en/#websocket-market-streams)
- [Polymarket CLOB API](https://docs.polymarket.com/)
- [CryptoPanic API](https://cryptopanic.com/developers/api/)
- [TimescaleDB Documentation](https://docs.timescale.com/)
