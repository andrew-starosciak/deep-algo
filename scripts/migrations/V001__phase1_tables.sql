-- Phase 1A: Statistical Trading Engine Database Schema
-- TimescaleDB hypertables for high-frequency trading data

-- Enable TimescaleDB extension
CREATE EXTENSION IF NOT EXISTS timescaledb;

-- ============================================================================
-- Order Book Snapshots
-- Captures order book state at 1/sec frequency
-- ============================================================================
CREATE TABLE IF NOT EXISTS orderbook_snapshots (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    bid_levels JSONB NOT NULL,
    ask_levels JSONB NOT NULL,
    bid_volume DECIMAL(20, 8) NOT NULL,
    ask_volume DECIMAL(20, 8) NOT NULL,
    imbalance DECIMAL(10, 8) NOT NULL,
    mid_price DECIMAL(20, 8),
    spread_bps DECIMAL(10, 4),
    PRIMARY KEY (timestamp, symbol, exchange)
);

SELECT create_hypertable('orderbook_snapshots', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_orderbook_symbol_time
    ON orderbook_snapshots (symbol, timestamp DESC);

-- Compression policy: compress chunks older than 7 days
ALTER TABLE orderbook_snapshots SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange'
);
SELECT add_compression_policy('orderbook_snapshots', INTERVAL '7 days', if_not_exists => TRUE);

-- ============================================================================
-- Funding Rates
-- Perpetual futures funding rates with statistical context
-- ============================================================================
CREATE TABLE IF NOT EXISTS funding_rates (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    funding_rate DECIMAL(20, 12) NOT NULL,
    annual_rate DECIMAL(10, 4),
    rate_percentile DECIMAL(5, 4),
    rate_zscore DECIMAL(10, 6),
    PRIMARY KEY (timestamp, symbol, exchange)
);

SELECT create_hypertable('funding_rates', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_funding_symbol_time
    ON funding_rates (symbol, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_funding_zscore
    ON funding_rates (rate_zscore) WHERE rate_zscore IS NOT NULL;

ALTER TABLE funding_rates SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange'
);
SELECT add_compression_policy('funding_rates', INTERVAL '30 days', if_not_exists => TRUE);

-- ============================================================================
-- Liquidations
-- Individual liquidation events (threshold: >$3K USD)
-- ============================================================================
CREATE TABLE IF NOT EXISTS liquidations (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    side TEXT NOT NULL CHECK (side IN ('long', 'short')),
    quantity DECIMAL(20, 8) NOT NULL,
    price DECIMAL(20, 8) NOT NULL,
    usd_value DECIMAL(20, 2) NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange, side, price)
);

SELECT create_hypertable('liquidations', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_liquidations_symbol_time
    ON liquidations (symbol, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_liquidations_usd_value
    ON liquidations (usd_value DESC);

ALTER TABLE liquidations SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange'
);
SELECT add_compression_policy('liquidations', INTERVAL '30 days', if_not_exists => TRUE);

-- ============================================================================
-- Liquidation Aggregates
-- Rolling window aggregates for cascade detection
-- ============================================================================
CREATE TABLE IF NOT EXISTS liquidation_aggregates (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    window_minutes INTEGER NOT NULL,
    long_volume DECIMAL(20, 2) NOT NULL,
    short_volume DECIMAL(20, 2) NOT NULL,
    net_delta DECIMAL(20, 2) NOT NULL,
    count_long INTEGER NOT NULL,
    count_short INTEGER NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange, window_minutes)
);

SELECT create_hypertable('liquidation_aggregates', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_liq_agg_symbol_window
    ON liquidation_aggregates (symbol, window_minutes, timestamp DESC);

ALTER TABLE liquidation_aggregates SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol,exchange,window_minutes'
);
SELECT add_compression_policy('liquidation_aggregates', INTERVAL '14 days', if_not_exists => TRUE);

-- ============================================================================
-- Polymarket Odds
-- Binary outcome market prices
-- ============================================================================
CREATE TABLE IF NOT EXISTS polymarket_odds (
    timestamp TIMESTAMPTZ NOT NULL,
    market_id TEXT NOT NULL,
    question TEXT NOT NULL,
    outcome_yes_price DECIMAL(10, 6) NOT NULL,
    outcome_no_price DECIMAL(10, 6) NOT NULL,
    volume_24h DECIMAL(20, 2),
    liquidity DECIMAL(20, 2),
    end_date TIMESTAMPTZ,
    PRIMARY KEY (timestamp, market_id)
);

SELECT create_hypertable('polymarket_odds', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_polymarket_market_time
    ON polymarket_odds (market_id, timestamp DESC);

ALTER TABLE polymarket_odds SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'market_id'
);
SELECT add_compression_policy('polymarket_odds', INTERVAL '30 days', if_not_exists => TRUE);

-- ============================================================================
-- News Events
-- News with urgency scoring for sentiment signals
-- ============================================================================
CREATE TABLE IF NOT EXISTS news_events (
    timestamp TIMESTAMPTZ NOT NULL,
    source TEXT NOT NULL,
    title TEXT NOT NULL,
    url TEXT,
    categories TEXT[],
    currencies TEXT[],
    sentiment TEXT CHECK (sentiment IN ('positive', 'negative', 'neutral', NULL)),
    urgency_score DECIMAL(5, 4),
    raw_data JSONB,
    PRIMARY KEY (timestamp, source, title)
);

SELECT create_hypertable('news_events', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_news_source_time
    ON news_events (source, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_news_urgency
    ON news_events (urgency_score DESC) WHERE urgency_score IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_news_currencies
    ON news_events USING GIN (currencies);

ALTER TABLE news_events SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'source'
);
SELECT add_compression_policy('news_events', INTERVAL '30 days', if_not_exists => TRUE);

-- ============================================================================
-- Binary Trades
-- Trade tracking for backtest and live execution
-- ============================================================================
CREATE TABLE IF NOT EXISTS binary_trades (
    id SERIAL,
    timestamp TIMESTAMPTZ NOT NULL,
    market_id TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('yes', 'no')),
    shares DECIMAL(20, 8) NOT NULL,
    price DECIMAL(10, 6) NOT NULL,
    stake DECIMAL(20, 2) NOT NULL,
    signals_snapshot JSONB,
    outcome TEXT CHECK (outcome IN ('win', 'loss', NULL)),
    pnl DECIMAL(20, 2),
    settled_at TIMESTAMPTZ,
    PRIMARY KEY (id, timestamp)
);

SELECT create_hypertable('binary_trades', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_trades_market_time
    ON binary_trades (market_id, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_trades_outcome
    ON binary_trades (outcome) WHERE outcome IS NOT NULL;

ALTER TABLE binary_trades SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'market_id'
);
SELECT add_compression_policy('binary_trades', INTERVAL '90 days', if_not_exists => TRUE);

-- ============================================================================
-- Utility Functions
-- ============================================================================

-- Function to calculate order book imbalance
CREATE OR REPLACE FUNCTION calculate_imbalance(bid_vol DECIMAL, ask_vol DECIMAL)
RETURNS DECIMAL AS $$
BEGIN
    IF bid_vol + ask_vol = 0 THEN
        RETURN 0;
    END IF;
    RETURN (bid_vol - ask_vol) / (bid_vol + ask_vol);
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- Function to get Wilson score confidence interval lower bound
-- Used for win rate confidence estimation
CREATE OR REPLACE FUNCTION wilson_ci_lower(wins INTEGER, total INTEGER, z DECIMAL DEFAULT 1.96)
RETURNS DECIMAL AS $$
DECLARE
    p DECIMAL;
    n DECIMAL;
    denom DECIMAL;
    center DECIMAL;
    spread DECIMAL;
BEGIN
    IF total = 0 THEN
        RETURN 0;
    END IF;

    p := wins::DECIMAL / total::DECIMAL;
    n := total::DECIMAL;
    denom := 1.0 + z * z / n;
    center := p + z * z / (2.0 * n);
    spread := z * SQRT(p * (1.0 - p) / n + z * z / (4.0 * n * n));

    RETURN (center - spread) / denom;
END;
$$ LANGUAGE plpgsql IMMUTABLE;
