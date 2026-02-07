-- V009: Price History & Session Tracking
-- Adds CLOB price snapshots, Chainlink window prices,
-- and fixes stale pending records.

-- ============================================================================
-- CLOB Price Snapshots
-- Raw UP/DOWN prices per coin per scan interval for offline analysis
-- ============================================================================
CREATE TABLE IF NOT EXISTS clob_price_snapshots (
    timestamp TIMESTAMPTZ NOT NULL,
    coin TEXT NOT NULL,
    up_price DECIMAL(10, 6) NOT NULL,
    down_price DECIMAL(10, 6) NOT NULL,
    up_token_id TEXT NOT NULL,
    down_token_id TEXT NOT NULL,
    -- Order book depth (nullable, only when WebSocket connected)
    up_bid_depth DECIMAL(18, 6),
    up_ask_depth DECIMAL(18, 6),
    down_bid_depth DECIMAL(18, 6),
    down_ask_depth DECIMAL(18, 6),
    session_id TEXT,
    PRIMARY KEY (timestamp, coin)
);

SELECT create_hypertable('clob_price_snapshots', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_clob_snapshots_coin_time
    ON clob_price_snapshots (coin, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_clob_snapshots_session
    ON clob_price_snapshots (session_id, timestamp DESC)
    WHERE session_id IS NOT NULL;

-- Compress after 3 days (high volume, ~4 rows/sec with 4 coins)
ALTER TABLE clob_price_snapshots SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'coin'
);
SELECT add_compression_policy('clob_price_snapshots', INTERVAL '3 days', if_not_exists => TRUE);

-- Retain 30 days of raw snapshots
SELECT add_retention_policy('clob_price_snapshots', INTERVAL '30 days', if_not_exists => TRUE);


-- ============================================================================
-- Chainlink Window Prices
-- Oracle start/end prices per 15-min window per coin
-- ============================================================================
CREATE TABLE IF NOT EXISTS chainlink_window_prices (
    window_start TIMESTAMPTZ NOT NULL,
    coin TEXT NOT NULL,
    start_price DECIMAL(20, 8) NOT NULL,
    end_price DECIMAL(20, 8),
    outcome TEXT CHECK (outcome IN ('UP', 'DOWN')),
    closed BOOLEAN NOT NULL DEFAULT FALSE,
    poll_count INTEGER NOT NULL DEFAULT 1,
    first_polled_at TIMESTAMPTZ NOT NULL,
    last_polled_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (window_start, coin)
);

SELECT create_hypertable('chainlink_window_prices', 'window_start', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_chainlink_window_coin
    ON chainlink_window_prices (coin, window_start DESC);

CREATE INDEX IF NOT EXISTS idx_chainlink_window_closed
    ON chainlink_window_prices (closed, window_start DESC)
    WHERE closed = TRUE;

ALTER TABLE chainlink_window_prices SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'coin'
);
SELECT add_compression_policy('chainlink_window_prices', INTERVAL '30 days', if_not_exists => TRUE);

-- Retain 90 days of window prices
SELECT add_retention_policy('chainlink_window_prices', INTERVAL '90 days', if_not_exists => TRUE);


-- ============================================================================
-- Mark stale pending records as expired
-- Any opportunity still 'pending' with window_end > 30 min ago is stale
-- ============================================================================
UPDATE cross_market_opportunities
SET status = 'expired', settled_at = NOW()
WHERE status = 'pending'
  AND window_end < NOW() - INTERVAL '30 minutes';
