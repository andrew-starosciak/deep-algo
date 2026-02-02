-- V006__cvd_tables.sql
-- CVD (Cumulative Volume Delta) infrastructure tables
--
-- Trade ticks capture individual trade executions for CVD calculation.
-- CVD aggregates store pre-computed volume delta over time windows.

-- ============================================================================
-- Trade Ticks Table
-- ============================================================================
-- Stores individual trade executions from exchange WebSocket feeds.
-- High volume table - expect ~1000+ trades per minute for BTCUSDT.

CREATE TABLE IF NOT EXISTS trade_ticks (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol VARCHAR(20) NOT NULL,
    exchange VARCHAR(20) NOT NULL,
    trade_id BIGINT NOT NULL,
    price DECIMAL(20, 8) NOT NULL,
    quantity DECIMAL(20, 8) NOT NULL,
    side VARCHAR(4) NOT NULL CHECK (side IN ('buy', 'sell')),
    usd_value DECIMAL(20, 2) NOT NULL,

    -- Primary key on timestamp and trade_id for deduplication
    PRIMARY KEY (timestamp, symbol, exchange, trade_id)
);

-- Convert to hypertable for time-series optimization
SELECT create_hypertable('trade_ticks', 'timestamp', if_not_exists => TRUE);

-- Index for querying trades by symbol and time range
CREATE INDEX IF NOT EXISTS idx_trade_ticks_symbol_time
    ON trade_ticks (symbol, exchange, timestamp DESC);

-- Index for aggregation queries
CREATE INDEX IF NOT EXISTS idx_trade_ticks_side_time
    ON trade_ticks (symbol, exchange, side, timestamp DESC);

-- Enable compression for older data (trades older than 1 day)
ALTER TABLE trade_ticks SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange,side'
);

SELECT add_compression_policy('trade_ticks', INTERVAL '1 day', if_not_exists => TRUE);

-- Retention policy: Keep trade ticks for 7 days (high volume)
SELECT add_retention_policy('trade_ticks', INTERVAL '7 days', if_not_exists => TRUE);


-- ============================================================================
-- CVD Aggregates Table
-- ============================================================================
-- Pre-computed CVD aggregates over configurable time windows.
-- Much lower volume than trade_ticks - one record per symbol per window.

CREATE TABLE IF NOT EXISTS cvd_aggregates (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol VARCHAR(20) NOT NULL,
    exchange VARCHAR(20) NOT NULL,
    window_seconds INT NOT NULL,
    buy_volume DECIMAL(20, 8) NOT NULL,
    sell_volume DECIMAL(20, 8) NOT NULL,
    cvd DECIMAL(20, 8) NOT NULL,  -- buy_volume - sell_volume
    trade_count INT NOT NULL,
    avg_price DECIMAL(20, 8),      -- VWAP for the window
    close_price DECIMAL(20, 8),    -- Last trade price in window

    -- Primary key ensures one aggregate per symbol per window per timestamp
    PRIMARY KEY (timestamp, symbol, exchange, window_seconds)
);

-- Convert to hypertable
SELECT create_hypertable('cvd_aggregates', 'timestamp', if_not_exists => TRUE);

-- Index for querying by symbol and window size
CREATE INDEX IF NOT EXISTS idx_cvd_agg_symbol_window_time
    ON cvd_aggregates (symbol, exchange, window_seconds, timestamp DESC);

-- Index for signal computation queries (recent data)
CREATE INDEX IF NOT EXISTS idx_cvd_agg_recent
    ON cvd_aggregates (symbol, timestamp DESC)
    WHERE window_seconds = 60;  -- Most common window size

-- Enable compression for older aggregates
ALTER TABLE cvd_aggregates SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange,window_seconds'
);

SELECT add_compression_policy('cvd_aggregates', INTERVAL '7 days', if_not_exists => TRUE);

-- Retention policy: Keep aggregates for 90 days
SELECT add_retention_policy('cvd_aggregates', INTERVAL '90 days', if_not_exists => TRUE);


-- ============================================================================
-- Continuous Aggregate for 1-minute CVD (materialized view)
-- ============================================================================
-- Automatically computes 1-minute CVD from trade_ticks.
-- This runs in background and stays up-to-date.

CREATE MATERIALIZED VIEW IF NOT EXISTS cvd_1min
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 minute', timestamp) AS bucket,
    symbol,
    exchange,
    SUM(CASE WHEN side = 'buy' THEN quantity ELSE 0 END) AS buy_volume,
    SUM(CASE WHEN side = 'sell' THEN quantity ELSE 0 END) AS sell_volume,
    SUM(CASE WHEN side = 'buy' THEN quantity ELSE -quantity END) AS cvd,
    COUNT(*) AS trade_count,
    SUM(usd_value) / NULLIF(SUM(quantity), 0) AS vwap,
    LAST(price, timestamp) AS close_price
FROM trade_ticks
GROUP BY bucket, symbol, exchange
WITH NO DATA;

-- Refresh policy: Compute aggregates for data older than 1 minute
SELECT add_continuous_aggregate_policy('cvd_1min',
    start_offset => INTERVAL '1 hour',
    end_offset => INTERVAL '1 minute',
    schedule_interval => INTERVAL '1 minute',
    if_not_exists => TRUE
);


-- ============================================================================
-- Continuous Aggregate for 5-minute CVD
-- ============================================================================

CREATE MATERIALIZED VIEW IF NOT EXISTS cvd_5min
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('5 minutes', timestamp) AS bucket,
    symbol,
    exchange,
    SUM(CASE WHEN side = 'buy' THEN quantity ELSE 0 END) AS buy_volume,
    SUM(CASE WHEN side = 'sell' THEN quantity ELSE 0 END) AS sell_volume,
    SUM(CASE WHEN side = 'buy' THEN quantity ELSE -quantity END) AS cvd,
    COUNT(*) AS trade_count,
    SUM(usd_value) / NULLIF(SUM(quantity), 0) AS vwap,
    LAST(price, timestamp) AS close_price
FROM trade_ticks
GROUP BY bucket, symbol, exchange
WITH NO DATA;

SELECT add_continuous_aggregate_policy('cvd_5min',
    start_offset => INTERVAL '6 hours',
    end_offset => INTERVAL '5 minutes',
    schedule_interval => INTERVAL '5 minutes',
    if_not_exists => TRUE
);


-- ============================================================================
-- Continuous Aggregate for 15-minute CVD (matches trading window)
-- ============================================================================

CREATE MATERIALIZED VIEW IF NOT EXISTS cvd_15min
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('15 minutes', timestamp) AS bucket,
    symbol,
    exchange,
    SUM(CASE WHEN side = 'buy' THEN quantity ELSE 0 END) AS buy_volume,
    SUM(CASE WHEN side = 'sell' THEN quantity ELSE 0 END) AS sell_volume,
    SUM(CASE WHEN side = 'buy' THEN quantity ELSE -quantity END) AS cvd,
    COUNT(*) AS trade_count,
    SUM(usd_value) / NULLIF(SUM(quantity), 0) AS vwap,
    LAST(price, timestamp) AS close_price
FROM trade_ticks
GROUP BY bucket, symbol, exchange
WITH NO DATA;

SELECT add_continuous_aggregate_policy('cvd_15min',
    start_offset => INTERVAL '1 day',
    end_offset => INTERVAL '15 minutes',
    schedule_interval => INTERVAL '15 minutes',
    if_not_exists => TRUE
);
