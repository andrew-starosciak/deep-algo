-- Phase 3b: Cross-Market Outcome Tracking & Analytics
-- Adds settlement tracking, outcome validation, and performance analytics

-- ============================================================================
-- Add Outcome Tracking Columns to Opportunities Table
-- ============================================================================

-- Settlement status: pending, settled, expired, error
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS status TEXT DEFAULT 'pending'
        CHECK (status IN ('pending', 'settled', 'expired', 'error'));

-- When the 15-min window expires (for settlement lookup)
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS window_end TIMESTAMPTZ;

-- Actual outcomes (populated after settlement)
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS coin1_outcome TEXT CHECK (coin1_outcome IN ('UP', 'DOWN'));

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS coin2_outcome TEXT CHECK (coin2_outcome IN ('UP', 'DOWN'));

-- Did the trade win? (at least one leg correct)
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS trade_result TEXT
        CHECK (trade_result IN ('WIN', 'LOSE', 'DOUBLE_WIN'));

-- Actual P&L if traded (payout - cost - fees)
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS actual_pnl DECIMAL(10, 6);

-- Was the correlation model correct? (coins moved together)
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS correlation_correct BOOLEAN;

-- Timestamps for tracking
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS settled_at TIMESTAMPTZ;

-- ============================================================================
-- Order Book Depth Tracking (for fill probability analysis)
-- ============================================================================

-- Leg 1 order book depth at detection time
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg1_bid_depth DECIMAL(18, 6);  -- Total $ available at bid

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg1_ask_depth DECIMAL(18, 6);  -- Total $ available at ask

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg1_spread_bps DECIMAL(10, 4);  -- Bid-ask spread in basis points

-- Leg 2 order book depth at detection time
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg2_bid_depth DECIMAL(18, 6);

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg2_ask_depth DECIMAL(18, 6);

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg2_spread_bps DECIMAL(10, 4);

-- Execution tracking (if trade was attempted)
ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS executed BOOLEAN DEFAULT FALSE;

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg1_fill_price DECIMAL(10, 6);  -- Actual fill price

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS leg2_fill_price DECIMAL(10, 6);

ALTER TABLE cross_market_opportunities
    ADD COLUMN IF NOT EXISTS slippage DECIMAL(10, 6);  -- Difference from expected

-- Index for settlement processing
CREATE INDEX IF NOT EXISTS idx_cross_market_pending
    ON cross_market_opportunities (status, window_end)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_cross_market_settled
    ON cross_market_opportunities (settled_at DESC)
    WHERE status = 'settled';

CREATE INDEX IF NOT EXISTS idx_cross_market_executed
    ON cross_market_opportunities (executed, timestamp DESC)
    WHERE executed = true;

-- ============================================================================
-- Coin Performance Analytics Table
-- Tracks which coins provide best arbitrage opportunities
-- ============================================================================
CREATE TABLE IF NOT EXISTS cross_market_coin_stats (
    id SERIAL PRIMARY KEY,

    -- Time bucket (hourly aggregation)
    hour TIMESTAMPTZ NOT NULL,

    -- Coin being analyzed
    coin TEXT NOT NULL,

    -- Opportunity metrics
    opportunities_as_leg1 INTEGER DEFAULT 0,
    opportunities_as_leg2 INTEGER DEFAULT 0,

    -- Average prices when this coin appears
    avg_up_price DECIMAL(10, 6),
    avg_down_price DECIMAL(10, 6),

    -- Price spread (how directional is the market on this coin)
    avg_price_spread DECIMAL(10, 6),  -- |up_price - down_price|

    -- Win rates when this coin is in the trade
    total_trades INTEGER DEFAULT 0,
    wins INTEGER DEFAULT 0,
    double_wins INTEGER DEFAULT 0,
    losses INTEGER DEFAULT 0,

    -- Correlation accuracy
    correlation_matches INTEGER DEFAULT 0,  -- Coin moved with majority
    correlation_breaks INTEGER DEFAULT 0,   -- Coin moved against majority

    -- P&L stats
    total_pnl DECIMAL(12, 6) DEFAULT 0,
    avg_pnl_per_trade DECIMAL(10, 6),
    best_trade_pnl DECIMAL(10, 6),
    worst_trade_pnl DECIMAL(10, 6),

    UNIQUE (hour, coin)
);

CREATE INDEX IF NOT EXISTS idx_coin_stats_coin ON cross_market_coin_stats (coin, hour DESC);
CREATE INDEX IF NOT EXISTS idx_coin_stats_hour ON cross_market_coin_stats (hour DESC);

-- ============================================================================
-- Pair Performance Analytics Table
-- Tracks which coin pairs provide best opportunities
-- ============================================================================
CREATE TABLE IF NOT EXISTS cross_market_pair_stats (
    id SERIAL PRIMARY KEY,

    -- Time bucket
    hour TIMESTAMPTZ NOT NULL,

    -- Pair (normalized: coin1 < coin2 alphabetically)
    coin1 TEXT NOT NULL,
    coin2 TEXT NOT NULL,

    -- Opportunity metrics
    total_opportunities INTEGER DEFAULT 0,
    avg_spread DECIMAL(10, 6),
    max_spread DECIMAL(10, 6),
    avg_ev DECIMAL(10, 6),

    -- By combination type
    coin1up_coin2down_count INTEGER DEFAULT 0,
    coin1down_coin2up_count INTEGER DEFAULT 0,
    bothup_count INTEGER DEFAULT 0,
    bothdown_count INTEGER DEFAULT 0,

    -- Settlement results
    total_settled INTEGER DEFAULT 0,
    wins INTEGER DEFAULT 0,
    double_wins INTEGER DEFAULT 0,
    losses INTEGER DEFAULT 0,

    -- Actual vs predicted
    predicted_win_rate DECIMAL(5, 4),
    actual_win_rate DECIMAL(5, 4),
    win_rate_error DECIMAL(5, 4),  -- actual - predicted

    -- Correlation accuracy for this pair
    times_moved_together INTEGER DEFAULT 0,
    times_diverged INTEGER DEFAULT 0,
    measured_correlation DECIMAL(5, 4),  -- Actual observed correlation

    -- P&L
    total_pnl DECIMAL(12, 6) DEFAULT 0,
    avg_pnl_per_trade DECIMAL(10, 6),

    UNIQUE (hour, coin1, coin2)
);

CREATE INDEX IF NOT EXISTS idx_pair_stats_pair ON cross_market_pair_stats (coin1, coin2, hour DESC);
CREATE INDEX IF NOT EXISTS idx_pair_stats_hour ON cross_market_pair_stats (hour DESC);

-- ============================================================================
-- Model Calibration Table
-- Tracks predicted vs actual win rates for model validation
-- ============================================================================
CREATE TABLE IF NOT EXISTS cross_market_calibration (
    id SERIAL PRIMARY KEY,

    -- Time bucket (daily for calibration)
    day DATE NOT NULL,

    -- Predicted probability bucket (0.50-0.55, 0.55-0.60, etc.)
    prob_bucket_lower DECIMAL(5, 4) NOT NULL,
    prob_bucket_upper DECIMAL(5, 4) NOT NULL,

    -- Sample counts
    total_predictions INTEGER DEFAULT 0,
    actual_wins INTEGER DEFAULT 0,

    -- Calibration metrics
    predicted_win_rate DECIMAL(5, 4),
    actual_win_rate DECIMAL(5, 4),
    calibration_error DECIMAL(5, 4),  -- |actual - predicted|

    -- Brier score component for this bucket
    brier_score_sum DECIMAL(12, 8) DEFAULT 0,

    UNIQUE (day, prob_bucket_lower)
);

CREATE INDEX IF NOT EXISTS idx_calibration_day ON cross_market_calibration (day DESC);

-- ============================================================================
-- Session Summary Table
-- Aggregates results per scanning session
-- ============================================================================
CREATE TABLE IF NOT EXISTS cross_market_sessions (
    id SERIAL PRIMARY KEY,
    session_id TEXT UNIQUE NOT NULL,

    -- Timing
    started_at TIMESTAMPTZ NOT NULL,
    ended_at TIMESTAMPTZ,

    -- Configuration used
    max_cost_threshold DECIMAL(10, 6),
    min_spread_threshold DECIMAL(10, 6),
    min_ev_threshold DECIMAL(10, 6),
    assumed_correlation DECIMAL(5, 4),
    coins_scanned TEXT[],

    -- Opportunity metrics
    total_opportunities INTEGER DEFAULT 0,
    avg_spread DECIMAL(10, 6),
    max_spread DECIMAL(10, 6),
    avg_predicted_ev DECIMAL(10, 6),

    -- Settlement results (populated after all opportunities settle)
    opportunities_settled INTEGER DEFAULT 0,
    total_wins INTEGER DEFAULT 0,
    total_losses INTEGER DEFAULT 0,
    double_wins INTEGER DEFAULT 0,

    -- Performance
    predicted_win_rate DECIMAL(5, 4),
    actual_win_rate DECIMAL(5, 4),
    total_pnl DECIMAL(12, 6),
    roi_pct DECIMAL(8, 4),  -- total_pnl / total_cost

    -- Model validation
    correlation_accuracy DECIMAL(5, 4),  -- % of times correlation held
    brier_score DECIMAL(8, 6),  -- Lower = better calibrated

    -- Status
    status TEXT DEFAULT 'active' CHECK (status IN ('active', 'completed', 'analyzing'))
);

CREATE INDEX IF NOT EXISTS idx_sessions_status ON cross_market_sessions (status, started_at DESC);

-- ============================================================================
-- Performance Views
-- ============================================================================

-- Real-time win rate by pair (settled opportunities only)
CREATE OR REPLACE VIEW cross_market_pair_performance AS
SELECT
    coin1,
    coin2,
    combination,
    COUNT(*) FILTER (WHERE status = 'settled') as settled_count,
    COUNT(*) FILTER (WHERE trade_result = 'WIN') as wins,
    COUNT(*) FILTER (WHERE trade_result = 'DOUBLE_WIN') as double_wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,

    -- Win rate (WIN or DOUBLE_WIN counts as win)
    ROUND(
        (COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::DECIMAL /
         NULLIF(COUNT(*) FILTER (WHERE status = 'settled'), 0)) * 100, 2
    ) as actual_win_rate_pct,

    -- Average predicted vs actual
    ROUND(AVG(win_probability) * 100, 2) as avg_predicted_win_rate_pct,

    -- P&L
    SUM(actual_pnl) as total_pnl,
    ROUND(AVG(actual_pnl), 4) as avg_pnl,

    -- Correlation accuracy
    ROUND(
        (COUNT(*) FILTER (WHERE correlation_correct = true)::DECIMAL /
         NULLIF(COUNT(*) FILTER (WHERE status = 'settled'), 0)) * 100, 2
    ) as correlation_accuracy_pct

FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY coin1, coin2, combination
ORDER BY total_pnl DESC;

-- Overall model performance
CREATE OR REPLACE VIEW cross_market_model_performance AS
SELECT
    DATE(timestamp) as date,
    COUNT(*) as total_opportunities,
    COUNT(*) FILTER (WHERE status = 'settled') as settled,

    -- Win rates
    ROUND(AVG(win_probability) * 100, 2) as avg_predicted_win_rate,
    ROUND(
        (COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::DECIMAL /
         NULLIF(COUNT(*) FILTER (WHERE status = 'settled'), 0)) * 100, 2
    ) as actual_win_rate,

    -- Calibration error
    ROUND(
        ABS(
            AVG(win_probability) -
            (COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::DECIMAL /
             NULLIF(COUNT(*) FILTER (WHERE status = 'settled'), 0))
        ) * 100, 2
    ) as calibration_error_pct,

    -- P&L
    SUM(actual_pnl) FILTER (WHERE status = 'settled') as total_pnl,
    ROUND(AVG(actual_pnl) FILTER (WHERE status = 'settled'), 4) as avg_pnl,

    -- Correlation accuracy
    ROUND(
        (COUNT(*) FILTER (WHERE correlation_correct = true)::DECIMAL /
         NULLIF(COUNT(*) FILTER (WHERE status = 'settled'), 0)) * 100, 2
    ) as correlation_accuracy_pct,

    -- Best opportunities
    MAX(spread) as best_spread,
    MAX(actual_pnl) as best_trade_pnl

FROM cross_market_opportunities
GROUP BY DATE(timestamp)
ORDER BY date DESC;

-- Hourly opportunity heatmap
CREATE OR REPLACE VIEW cross_market_hourly_heatmap AS
SELECT
    EXTRACT(HOUR FROM timestamp) as hour_of_day,
    EXTRACT(DOW FROM timestamp) as day_of_week,
    COUNT(*) as opportunity_count,
    AVG(spread) as avg_spread,
    AVG(expected_value) as avg_ev,
    SUM(actual_pnl) FILTER (WHERE status = 'settled') as total_pnl,
    ROUND(
        (COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::DECIMAL /
         NULLIF(COUNT(*) FILTER (WHERE status = 'settled'), 0)) * 100, 2
    ) as win_rate_pct
FROM cross_market_opportunities
GROUP BY EXTRACT(HOUR FROM timestamp), EXTRACT(DOW FROM timestamp)
ORDER BY hour_of_day, day_of_week;

-- Token-level edge analysis
CREATE OR REPLACE VIEW cross_market_token_edge AS
WITH token_stats AS (
    SELECT
        coin1 as coin,
        leg1_direction as direction,
        leg1_price as entry_price,
        coin1_outcome as actual_outcome,
        CASE
            WHEN leg1_direction = coin1_outcome THEN 1
            ELSE 0
        END as leg_won,
        actual_pnl,
        timestamp
    FROM cross_market_opportunities
    WHERE status = 'settled'

    UNION ALL

    SELECT
        coin2 as coin,
        leg2_direction as direction,
        leg2_price as entry_price,
        coin2_outcome as actual_outcome,
        CASE
            WHEN leg2_direction = coin2_outcome THEN 1
            ELSE 0
        END as leg_won,
        actual_pnl,
        timestamp
    FROM cross_market_opportunities
    WHERE status = 'settled'
)
SELECT
    coin,
    direction,
    COUNT(*) as trade_count,
    ROUND(AVG(entry_price), 4) as avg_entry_price,
    ROUND(SUM(leg_won)::DECIMAL / COUNT(*) * 100, 2) as leg_win_rate_pct,
    ROUND(AVG(actual_pnl), 4) as avg_pnl_contribution
FROM token_stats
GROUP BY coin, direction
ORDER BY coin, direction;

-- ============================================================================
-- Comments
-- ============================================================================
COMMENT ON COLUMN cross_market_opportunities.status IS 'pending=awaiting settlement, settled=outcome known, expired=market ended without data, error=settlement failed';
COMMENT ON COLUMN cross_market_opportunities.trade_result IS 'WIN=one leg won, DOUBLE_WIN=both legs won, LOSE=both legs lost';
COMMENT ON COLUMN cross_market_opportunities.correlation_correct IS 'true if both coins moved in same direction (correlation held)';
COMMENT ON TABLE cross_market_coin_stats IS 'Hourly aggregated stats per coin for identifying which coins offer best edges';
COMMENT ON TABLE cross_market_pair_stats IS 'Hourly aggregated stats per pair for identifying best pair combinations';
COMMENT ON TABLE cross_market_calibration IS 'Model calibration data - compares predicted vs actual win rates by probability bucket';
COMMENT ON TABLE cross_market_sessions IS 'Session-level summary for tracking scanning session performance';

-- ============================================================================
-- Order Book Depth Analysis View
-- Analyzes fill probability based on depth
-- ============================================================================
CREATE OR REPLACE VIEW cross_market_depth_analysis AS
SELECT
    coin1,
    coin2,
    combination,
    COUNT(*) as total_opportunities,

    -- Average depth at detection
    AVG(leg1_bid_depth) as avg_leg1_bid_depth,
    AVG(leg1_ask_depth) as avg_leg1_ask_depth,
    AVG(leg2_bid_depth) as avg_leg2_bid_depth,
    AVG(leg2_ask_depth) as avg_leg2_ask_depth,

    -- Spread analysis
    AVG(leg1_spread_bps) as avg_leg1_spread_bps,
    AVG(leg2_spread_bps) as avg_leg2_spread_bps,

    -- Execution stats
    COUNT(*) FILTER (WHERE executed = true) as executed_count,
    AVG(slippage) FILTER (WHERE executed = true) as avg_slippage,

    -- Depth buckets (how often is there enough depth for $100 trade)
    COUNT(*) FILTER (WHERE LEAST(leg1_bid_depth, leg1_ask_depth) >= 100) as depth_over_100,
    COUNT(*) FILTER (WHERE LEAST(leg1_bid_depth, leg1_ask_depth) >= 500) as depth_over_500,
    COUNT(*) FILTER (WHERE LEAST(leg1_bid_depth, leg1_ask_depth) >= 1000) as depth_over_1000

FROM cross_market_opportunities
WHERE leg1_bid_depth IS NOT NULL
GROUP BY coin1, coin2, combination
ORDER BY total_opportunities DESC;

-- ============================================================================
-- Slippage Analysis View
-- Tracks execution quality
-- ============================================================================
CREATE OR REPLACE VIEW cross_market_slippage_analysis AS
SELECT
    DATE(timestamp) as date,
    coin1,
    coin2,

    -- Execution counts
    COUNT(*) FILTER (WHERE executed = true) as trades_executed,

    -- Fill quality
    AVG(slippage) as avg_slippage,
    MAX(slippage) as worst_slippage,
    MIN(slippage) as best_slippage,

    -- Depth correlation with slippage
    CORR(
        LEAST(leg1_bid_depth, leg2_bid_depth),
        slippage
    ) as depth_slippage_correlation,

    -- Profitability after slippage
    AVG(actual_pnl) FILTER (WHERE executed = true) as avg_pnl_after_slippage,
    SUM(actual_pnl) FILTER (WHERE executed = true) as total_pnl_after_slippage

FROM cross_market_opportunities
WHERE executed = true
GROUP BY DATE(timestamp), coin1, coin2
ORDER BY date DESC, trades_executed DESC;
