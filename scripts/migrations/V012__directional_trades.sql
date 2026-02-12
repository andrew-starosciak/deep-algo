-- V012: Directional Trading Persistence
-- Records every directional trade and session for cross-session
-- performance analysis and restart recovery.

CREATE TABLE IF NOT EXISTS directional_trades (
    id SERIAL,
    timestamp TIMESTAMPTZ NOT NULL,
    trade_id TEXT NOT NULL,
    coin TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('UP', 'DOWN')),
    token_id TEXT NOT NULL,
    entry_price DECIMAL(10, 6) NOT NULL,
    shares DECIMAL(18, 6) NOT NULL,
    cost DECIMAL(18, 6) NOT NULL,
    estimated_edge DECIMAL(8, 6),
    win_probability DECIMAL(8, 6),
    delta_pct DECIMAL(10, 6),
    signal_timestamp TIMESTAMPTZ NOT NULL,
    window_end TIMESTAMPTZ NOT NULL,
    -- Settlement
    settled BOOLEAN NOT NULL DEFAULT FALSE,
    won BOOLEAN,
    pnl DECIMAL(18, 6),
    settled_at TIMESTAMPTZ,
    -- Tracking
    session_id TEXT,
    mode TEXT NOT NULL DEFAULT 'paper',
    PRIMARY KEY (id, timestamp)
);

SELECT create_hypertable('directional_trades', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_directional_trades_pending
    ON directional_trades (settled, timestamp DESC)
    WHERE settled = FALSE;

CREATE INDEX IF NOT EXISTS idx_directional_trades_session
    ON directional_trades (session_id, timestamp DESC)
    WHERE session_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_directional_trades_coin_time
    ON directional_trades (coin, timestamp DESC);

ALTER TABLE directional_trades SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'coin'
);
SELECT add_compression_policy('directional_trades', INTERVAL '7 days', if_not_exists => TRUE);
SELECT add_retention_policy('directional_trades', INTERVAL '365 days', if_not_exists => TRUE);

-- Session tracking
CREATE TABLE IF NOT EXISTS directional_sessions (
    session_id TEXT PRIMARY KEY,
    started_at TIMESTAMPTZ NOT NULL,
    ended_at TIMESTAMPTZ,
    mode TEXT NOT NULL DEFAULT 'paper',
    coins TEXT,
    signals_received INTEGER DEFAULT 0,
    orders_filled INTEGER DEFAULT 0,
    wins INTEGER DEFAULT 0,
    losses INTEGER DEFAULT 0,
    total_pnl DECIMAL(18, 6) DEFAULT 0,
    status TEXT DEFAULT 'active'
);
