-- Phase 2: Paper Trading Schema
-- Stores paper trades with full tracking for signal validation

-- ============================================================================
-- Paper Trades
-- Paper trading records with signal snapshots and P&L tracking
-- ============================================================================
CREATE TABLE IF NOT EXISTS paper_trades (
    id SERIAL,
    timestamp TIMESTAMPTZ NOT NULL,
    market_id TEXT NOT NULL,
    market_question TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('yes', 'no')),
    shares DECIMAL(20, 8) NOT NULL,
    entry_price DECIMAL(10, 6) NOT NULL,
    stake DECIMAL(20, 2) NOT NULL,
    estimated_prob DECIMAL(10, 6) NOT NULL,
    expected_value DECIMAL(20, 4) NOT NULL,
    kelly_fraction DECIMAL(10, 6),
    signal_strength DECIMAL(10, 6),
    signals_snapshot JSONB,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'settled', 'cancelled')),
    outcome TEXT CHECK (outcome IN ('win', 'loss', NULL)),
    pnl DECIMAL(20, 4),
    fees DECIMAL(20, 4),
    settled_at TIMESTAMPTZ,
    session_id TEXT NOT NULL,
    PRIMARY KEY (id, timestamp)
);

-- Convert to hypertable for time-series optimization
SELECT create_hypertable('paper_trades', 'timestamp', if_not_exists => TRUE);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_paper_trades_session
    ON paper_trades (session_id, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_paper_trades_market
    ON paper_trades (market_id, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_paper_trades_status
    ON paper_trades (status) WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_paper_trades_outcome
    ON paper_trades (outcome) WHERE outcome IS NOT NULL;

-- Compression policy: compress chunks older than 30 days
ALTER TABLE paper_trades SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'session_id,market_id'
);
SELECT add_compression_policy('paper_trades', INTERVAL '30 days', if_not_exists => TRUE);

-- ============================================================================
-- Paper Trade Statistics View
-- Aggregated view for quick performance analysis
-- ============================================================================
CREATE OR REPLACE VIEW paper_trade_stats AS
SELECT
    session_id,
    COUNT(*) as total_trades,
    COUNT(*) FILTER (WHERE status = 'settled') as settled_trades,
    COUNT(*) FILTER (WHERE status = 'pending') as pending_trades,
    COUNT(*) FILTER (WHERE outcome = 'win') as wins,
    COUNT(*) FILTER (WHERE outcome = 'loss') as losses,
    CASE
        WHEN COUNT(*) FILTER (WHERE status = 'settled') > 0
        THEN ROUND(
            COUNT(*) FILTER (WHERE outcome = 'win')::DECIMAL /
            COUNT(*) FILTER (WHERE status = 'settled')::DECIMAL * 100, 2
        )
        ELSE 0
    END as win_rate_pct,
    COALESCE(SUM(stake), 0) as total_stake,
    COALESCE(SUM(pnl) FILTER (WHERE status = 'settled'), 0) as total_pnl,
    COALESCE(SUM(fees) FILTER (WHERE status = 'settled'), 0) as total_fees,
    MIN(timestamp) as first_trade,
    MAX(timestamp) as last_trade,
    wilson_ci_lower(
        COUNT(*) FILTER (WHERE outcome = 'win')::INTEGER,
        COUNT(*) FILTER (WHERE status = 'settled')::INTEGER
    ) as wilson_ci_lower
FROM paper_trades
GROUP BY session_id;

-- ============================================================================
-- Entry Strategy Analysis Table (Optional)
-- Tracks entry timing performance
-- ============================================================================
CREATE TABLE IF NOT EXISTS paper_trade_entries (
    id SERIAL PRIMARY KEY,
    paper_trade_id INTEGER NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    entry_strategy TEXT NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    window_end TIMESTAMPTZ NOT NULL,
    entry_offset_secs INTEGER NOT NULL,
    edge_at_entry DECIMAL(10, 6),
    used_fallback BOOLEAN DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_paper_trade_entries_trade
    ON paper_trade_entries (paper_trade_id);

CREATE INDEX IF NOT EXISTS idx_paper_trade_entries_strategy
    ON paper_trade_entries (entry_strategy, timestamp DESC);
