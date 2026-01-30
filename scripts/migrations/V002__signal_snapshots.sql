-- V002__signal_snapshots.sql
-- Signal snapshot logging for statistical validation and debugging

-- Signal snapshots table for logging signal outputs
CREATE TABLE IF NOT EXISTS signal_snapshots (
    id BIGSERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    signal_name VARCHAR(100) NOT NULL,
    symbol VARCHAR(50) NOT NULL,
    exchange VARCHAR(50) NOT NULL,
    direction VARCHAR(10) NOT NULL CHECK (direction IN ('up', 'down', 'neutral')),
    strength DECIMAL(10, 6) NOT NULL CHECK (strength >= 0 AND strength <= 1),
    confidence DECIMAL(10, 6) NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
    metadata JSONB DEFAULT '{}',
    -- Forward return for validation (filled in later)
    forward_return_15m DECIMAL(20, 8),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create hypertable for time-series optimization
SELECT create_hypertable('signal_snapshots', 'timestamp',
    if_not_exists => TRUE,
    chunk_time_interval => INTERVAL '1 day'
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_signal_snapshots_signal_name
    ON signal_snapshots (signal_name, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_signal_snapshots_symbol_exchange
    ON signal_snapshots (symbol, exchange, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_signal_snapshots_direction
    ON signal_snapshots (direction, timestamp DESC);

-- Index for validation queries (find snapshots without forward returns)
CREATE INDEX IF NOT EXISTS idx_signal_snapshots_pending_validation
    ON signal_snapshots (timestamp)
    WHERE forward_return_15m IS NULL;

-- Compression policy for older data
SELECT add_compression_policy('signal_snapshots', INTERVAL '7 days', if_not_exists => TRUE);

-- Retention policy: keep 90 days of data
SELECT add_retention_policy('signal_snapshots', INTERVAL '90 days', if_not_exists => TRUE);

-- Comment on table
COMMENT ON TABLE signal_snapshots IS 'Logs signal generator outputs for statistical validation and debugging';

COMMENT ON COLUMN signal_snapshots.direction IS 'Signal direction: up (bullish), down (bearish), neutral';
COMMENT ON COLUMN signal_snapshots.strength IS 'Signal strength from 0.0 (weakest) to 1.0 (strongest)';
COMMENT ON COLUMN signal_snapshots.confidence IS 'Statistical confidence from 0.0 to 1.0';
COMMENT ON COLUMN signal_snapshots.metadata IS 'Additional signal-specific data for debugging';
COMMENT ON COLUMN signal_snapshots.forward_return_15m IS '15-minute forward return for validation (filled after settlement)';
