-- Create OHLCV hypertable
CREATE TABLE IF NOT EXISTS ohlcv (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    open DECIMAL(20, 8) NOT NULL,
    high DECIMAL(20, 8) NOT NULL,
    low DECIMAL(20, 8) NOT NULL,
    close DECIMAL(20, 8) NOT NULL,
    volume DECIMAL(20, 8) NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange)
);

-- Convert to hypertable (partitioned by time)
SELECT create_hypertable('ohlcv', 'timestamp', if_not_exists => TRUE);

-- Create indexes for common queries
CREATE INDEX IF NOT EXISTS idx_ohlcv_symbol_time ON ohlcv (symbol, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_ohlcv_exchange_time ON ohlcv (exchange, timestamp DESC);

-- Enable compression for old data
ALTER TABLE ohlcv SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol, exchange'
);

-- Compress data older than 7 days
SELECT add_compression_policy('ohlcv', INTERVAL '7 days');

-- Create trades table
CREATE TABLE IF NOT EXISTS trades (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    price DECIMAL(20, 8) NOT NULL,
    size DECIMAL(20, 8) NOT NULL,
    side TEXT NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange)
);

SELECT create_hypertable('trades', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_trades_symbol_time ON trades (symbol, timestamp DESC);

-- Create fills table for tracking executed orders
CREATE TABLE IF NOT EXISTS fills (
    id SERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    order_id TEXT NOT NULL,
    symbol TEXT NOT NULL,
    direction TEXT NOT NULL,
    quantity DECIMAL(20, 8) NOT NULL,
    price DECIMAL(20, 8) NOT NULL,
    commission DECIMAL(20, 8) NOT NULL,
    strategy TEXT
);

CREATE INDEX IF NOT EXISTS idx_fills_timestamp ON fills (timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_fills_order_id ON fills (order_id);
CREATE INDEX IF NOT EXISTS idx_fills_symbol ON fills (symbol);

-- Create backtest_results table for storing backtest performance metrics
CREATE TABLE IF NOT EXISTS backtest_results (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    strategy_name TEXT NOT NULL,
    sharpe_ratio DECIMAL(10, 4),
    sortino_ratio DECIMAL(10, 4),
    total_pnl DECIMAL(20, 8) NOT NULL,
    total_return DECIMAL(10, 6),
    win_rate DECIMAL(5, 4),
    max_drawdown DECIMAL(5, 4),
    num_trades INTEGER NOT NULL,
    parameters JSONB,
    PRIMARY KEY (timestamp, symbol, exchange, strategy_name)
);

-- Convert to hypertable
SELECT create_hypertable('backtest_results', 'timestamp', if_not_exists => TRUE);

-- Create indexes for token selection queries
CREATE INDEX IF NOT EXISTS idx_backtest_results_symbol_time ON backtest_results (symbol, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_backtest_results_sharpe ON backtest_results (sharpe_ratio DESC);
CREATE INDEX IF NOT EXISTS idx_backtest_results_strategy ON backtest_results (strategy_name, timestamp DESC);

-- Enable compression for old backtest results
ALTER TABLE backtest_results SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol, exchange, strategy_name'
);

-- Compress backtest data older than 30 days
SELECT add_compression_policy('backtest_results', INTERVAL '30 days');
