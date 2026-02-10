-- V011: Window Settlement History
-- Records the outcome of every 15-minute window for all coins,
-- regardless of whether we traded it. Essential for backtesting.

CREATE TABLE IF NOT EXISTS window_settlements (
    window_start TIMESTAMPTZ NOT NULL,
    coin TEXT NOT NULL,
    window_end TIMESTAMPTZ NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('UP', 'DOWN')),
    settlement_source TEXT NOT NULL CHECK (settlement_source IN ('gamma', 'chainlink')),

    -- Gamma API data (when available)
    gamma_slug TEXT,
    condition_id TEXT,

    -- Chainlink oracle prices
    chainlink_start_price DECIMAL(20, 8),
    chainlink_end_price DECIMAL(20, 8),

    -- Metadata
    settled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    session_id TEXT,

    PRIMARY KEY (window_start, coin)
);

SELECT create_hypertable('window_settlements', 'window_start', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_window_settlements_coin_time
    ON window_settlements (coin, window_start DESC);

CREATE INDEX IF NOT EXISTS idx_window_settlements_outcome
    ON window_settlements (coin, outcome, window_start DESC);

CREATE INDEX IF NOT EXISTS idx_window_settlements_source
    ON window_settlements (settlement_source, window_start DESC);

CREATE INDEX IF NOT EXISTS idx_window_settlements_session
    ON window_settlements (session_id, window_start DESC)
    WHERE session_id IS NOT NULL;

ALTER TABLE window_settlements SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'coin'
);
SELECT add_compression_policy('window_settlements', INTERVAL '7 days', if_not_exists => TRUE);
SELECT add_retention_policy('window_settlements', INTERVAL '365 days', if_not_exists => TRUE);
