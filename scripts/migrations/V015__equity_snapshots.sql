-- Equity snapshots for PnL chart time series.
-- Position manager writes a snapshot on each tick (~30s during market hours).

CREATE TABLE IF NOT EXISTS equity_snapshots (
    timestamp         TIMESTAMPTZ NOT NULL,
    net_liquidation   DECIMAL NOT NULL,
    total_unrealized_pnl DECIMAL NOT NULL,
    total_realized_pnl   DECIMAL NOT NULL,
    open_positions_count INT NOT NULL DEFAULT 0,
    total_options_exposure DECIMAL NOT NULL DEFAULT 0,
    PRIMARY KEY (timestamp)
);

CREATE INDEX idx_equity_snapshots_ts ON equity_snapshots (timestamp DESC);
