-- V013: Options trading system tables
-- Supports the IB options trading workflow:
-- Python writes research/theses/recommendations → human approves → Rust executes

-- Watchlist of tickers to monitor
CREATE TABLE IF NOT EXISTS options_watchlist (
    ticker TEXT PRIMARY KEY,
    sector TEXT NOT NULL,
    added_at TIMESTAMPTZ DEFAULT NOW(),
    notes TEXT
);

-- Workflow execution tracking
CREATE TABLE IF NOT EXISTS workflow_runs (
    id SERIAL PRIMARY KEY,
    workflow_id TEXT NOT NULL,
    trigger TEXT NOT NULL,
    input JSONB NOT NULL,
    status TEXT DEFAULT 'running',
    result JSONB,
    started_at TIMESTAMPTZ DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX idx_workflow_runs_status ON workflow_runs(status);
CREATE INDEX idx_workflow_runs_workflow_id ON workflow_runs(workflow_id);

-- Step-level audit trail
CREATE TABLE IF NOT EXISTS workflow_step_logs (
    id SERIAL PRIMARY KEY,
    run_id INT REFERENCES workflow_runs(id),
    step_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    attempt INT NOT NULL,
    input JSONB NOT NULL,
    output JSONB,
    passed_gate BOOLEAN,
    duration_ms INT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_workflow_step_logs_run_id ON workflow_step_logs(run_id);

-- Research summaries (written by Python researcher agent)
CREATE TABLE IF NOT EXISTS research_summaries (
    id SERIAL PRIMARY KEY,
    run_id INT REFERENCES workflow_runs(id),
    ticker TEXT NOT NULL,
    mode TEXT NOT NULL,
    summary JSONB NOT NULL,
    opportunity_score INT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_research_summaries_ticker ON research_summaries(ticker);
CREATE INDEX idx_research_summaries_created ON research_summaries(created_at DESC);

-- Theses (written by Python analyst agent)
CREATE TABLE IF NOT EXISTS theses (
    id SERIAL PRIMARY KEY,
    run_id INT REFERENCES workflow_runs(id),
    ticker TEXT NOT NULL,
    direction TEXT NOT NULL,
    thesis_text TEXT NOT NULL,
    catalyst JSONB,
    scores JSONB NOT NULL,
    supporting_evidence JSONB,
    risks JSONB,
    overall_score FLOAT NOT NULL,
    status TEXT DEFAULT 'scored',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_theses_ticker ON theses(ticker);
CREATE INDEX idx_theses_overall_score ON theses(overall_score DESC);
CREATE INDEX idx_theses_status ON theses(status);

-- Trade recommendations (Python writes, human approves, Rust reads)
CREATE TABLE IF NOT EXISTS trade_recommendations (
    id SERIAL PRIMARY KEY,
    thesis_id INT REFERENCES theses(id),
    run_id INT REFERENCES workflow_runs(id),
    ticker TEXT NOT NULL,
    "right" TEXT NOT NULL,
    strike DECIMAL NOT NULL,
    expiry DATE NOT NULL,
    strategy TEXT DEFAULT 'naked',
    entry_price_low DECIMAL,
    entry_price_high DECIMAL,
    position_size_pct DECIMAL,
    position_size_usd DECIMAL,
    exit_targets JSONB,
    stop_loss TEXT,
    max_hold_days INT,
    risk_verification JSONB,
    status TEXT DEFAULT 'pending_review',
    reviewed_by TEXT,
    approved_at TIMESTAMPTZ,
    rejected_reason TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_trade_recommendations_status ON trade_recommendations(status);
CREATE INDEX idx_trade_recommendations_ticker ON trade_recommendations(ticker);

-- Options positions (written by Rust after IB fill)
CREATE TABLE IF NOT EXISTS options_positions (
    id BIGSERIAL PRIMARY KEY,
    recommendation_id INT REFERENCES trade_recommendations(id),
    ticker TEXT NOT NULL,
    "right" TEXT NOT NULL,
    strike DECIMAL NOT NULL,
    expiry DATE NOT NULL,
    quantity INT NOT NULL,
    avg_fill_price DECIMAL NOT NULL,
    current_price DECIMAL,
    unrealized_pnl DECIMAL,
    realized_pnl DECIMAL DEFAULT 0,
    greeks JSONB,
    cost_basis DECIMAL NOT NULL,
    status TEXT DEFAULT 'open',
    opened_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,
    close_reason TEXT,
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_options_positions_status ON options_positions(status);
CREATE INDEX idx_options_positions_ticker ON options_positions(ticker);

-- Position reviews (written by Python reviewer agent)
CREATE TABLE IF NOT EXISTS position_reviews (
    id SERIAL PRIMARY KEY,
    run_id INT REFERENCES workflow_runs(id),
    position_id BIGINT REFERENCES options_positions(id),
    review_type TEXT NOT NULL,
    thesis_still_valid BOOLEAN,
    pnl_pct FLOAT,
    recommended_action TEXT,
    reasoning TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_position_reviews_position_id ON position_reviews(position_id);

-- Options chain snapshots (written by Rust, read by Python for analysis)
CREATE TABLE IF NOT EXISTS options_chain_snapshots (
    timestamp TIMESTAMPTZ NOT NULL,
    ticker TEXT NOT NULL,
    expiry DATE NOT NULL,
    strike DECIMAL NOT NULL,
    "right" TEXT NOT NULL,
    bid DECIMAL,
    ask DECIMAL,
    last DECIMAL,
    volume BIGINT,
    open_interest BIGINT,
    iv FLOAT,
    delta FLOAT,
    gamma FLOAT,
    theta FLOAT,
    vega FLOAT
);

-- Make options_chain_snapshots a hypertable for time-series queries
SELECT create_hypertable('options_chain_snapshots', 'timestamp', if_not_exists => TRUE);

CREATE INDEX idx_options_chain_ticker_expiry ON options_chain_snapshots(ticker, expiry, timestamp DESC);

-- Weekly battle plans (written by Python, read by human)
CREATE TABLE IF NOT EXISTS weekly_battle_plans (
    id SERIAL PRIMARY KEY,
    run_id INT REFERENCES workflow_runs(id),
    week_start DATE NOT NULL,
    macro_view TEXT,
    sector_analysis JSONB,
    performance_review JSONB,
    top_ideas JSONB,
    focus_tickers JSONB,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Seed initial watchlist (mega cap tech)
INSERT INTO options_watchlist (ticker, sector, notes) VALUES
    ('NVDA', 'tech', 'AI capex bellwether, high vol, massive options liquidity'),
    ('AAPL', 'tech', 'Consumer tech proxy, earnings mover, enormous options market'),
    ('MSFT', 'tech', 'Cloud/AI narrative, steady mover, great for spread strategies'),
    ('GOOG', 'tech', 'Search/AI competition narrative, tends to gap on earnings'),
    ('META', 'tech', 'Ad revenue proxy, social sentiment, big earnings moves'),
    ('AMZN', 'tech', 'E-commerce + AWS, wide range of catalysts'),
    ('TSLA', 'tech', 'High vol, narrative-driven, options premiums are rich'),
    ('AMD', 'tech', 'NVDA satellite play, cheaper options, high beta')
ON CONFLICT (ticker) DO NOTHING;
