-- Phase 3: Cross-Market Correlation Arbitrage Schema
-- Stores cross-market opportunities for analysis and backtesting

-- ============================================================================
-- Cross-Market Opportunities
-- Records of cross-market correlation arbitrage opportunities detected
-- ============================================================================
CREATE TABLE IF NOT EXISTS cross_market_opportunities (
    id SERIAL,
    timestamp TIMESTAMPTZ NOT NULL,

    -- Coin pair identification
    coin1 TEXT NOT NULL,
    coin2 TEXT NOT NULL,
    combination TEXT NOT NULL CHECK (combination IN (
        'Coin1UpCoin2Down',
        'Coin1DownCoin2Up',
        'BothUp',
        'BothDown'
    )),

    -- Leg 1 details (first coin)
    leg1_direction TEXT NOT NULL CHECK (leg1_direction IN ('UP', 'DOWN')),
    leg1_price DECIMAL(10, 6) NOT NULL,
    leg1_token_id TEXT NOT NULL,

    -- Leg 2 details (second coin)
    leg2_direction TEXT NOT NULL CHECK (leg2_direction IN ('UP', 'DOWN')),
    leg2_price DECIMAL(10, 6) NOT NULL,
    leg2_token_id TEXT NOT NULL,

    -- Opportunity metrics
    total_cost DECIMAL(10, 6) NOT NULL,
    spread DECIMAL(10, 6) NOT NULL,
    expected_value DECIMAL(10, 6) NOT NULL,
    win_probability DECIMAL(5, 4) NOT NULL,
    assumed_correlation DECIMAL(5, 4) NOT NULL,

    -- Tracking
    session_id TEXT,

    PRIMARY KEY (id, timestamp)
);

-- Convert to hypertable for time-series optimization
SELECT create_hypertable('cross_market_opportunities', 'timestamp', if_not_exists => TRUE);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_cross_market_pair
    ON cross_market_opportunities (coin1, coin2, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_cross_market_combination
    ON cross_market_opportunities (combination, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_cross_market_spread
    ON cross_market_opportunities (spread DESC, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_cross_market_ev
    ON cross_market_opportunities (expected_value DESC, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_cross_market_session
    ON cross_market_opportunities (session_id, timestamp DESC)
    WHERE session_id IS NOT NULL;

-- Compression policy: compress chunks older than 7 days
ALTER TABLE cross_market_opportunities SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'coin1,coin2,combination'
);
SELECT add_compression_policy('cross_market_opportunities', INTERVAL '7 days', if_not_exists => TRUE);

-- ============================================================================
-- Cross-Market Statistics View
-- Aggregated view for quick analysis
-- ============================================================================
CREATE OR REPLACE VIEW cross_market_stats AS
SELECT
    coin1,
    coin2,
    combination,
    COUNT(*) as total_opportunities,
    AVG(total_cost) as avg_cost,
    AVG(spread) as avg_spread,
    AVG(expected_value) as avg_ev,
    AVG(win_probability) as avg_win_prob,
    MIN(total_cost) as min_cost,
    MAX(spread) as max_spread,
    MIN(timestamp) as first_seen,
    MAX(timestamp) as last_seen
FROM cross_market_opportunities
GROUP BY coin1, coin2, combination;

-- ============================================================================
-- Hourly Aggregation View
-- For time-series analysis of opportunity frequency
-- ============================================================================
CREATE OR REPLACE VIEW cross_market_hourly AS
SELECT
    time_bucket('1 hour', timestamp) as hour,
    coin1,
    coin2,
    COUNT(*) as opportunity_count,
    AVG(spread) as avg_spread,
    AVG(expected_value) as avg_ev,
    MIN(total_cost) as best_cost
FROM cross_market_opportunities
GROUP BY hour, coin1, coin2
ORDER BY hour DESC;
