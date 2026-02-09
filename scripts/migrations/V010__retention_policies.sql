-- V010: Add retention policies for unbounded tables
-- Prevents disk growth on long-running EC2 instances

-- cross_market_opportunities: high volume, no retention previously
SELECT add_retention_policy('cross_market_opportunities', INTERVAL '180 days', if_not_exists => TRUE);

-- cross_market_coin_stats / pair_stats / calibration: small but unbounded
SELECT add_retention_policy('cross_market_calibration', INTERVAL '180 days', if_not_exists => TRUE);

-- Phase 1 tables that had compression but no retention
SELECT add_retention_policy('orderbook_snapshots', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('funding_rates', INTERVAL '180 days', if_not_exists => TRUE);
SELECT add_retention_policy('liquidations', INTERVAL '180 days', if_not_exists => TRUE);
SELECT add_retention_policy('liquidation_aggregates', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('polymarket_odds', INTERVAL '180 days', if_not_exists => TRUE);
SELECT add_retention_policy('news_events', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('binary_trades', INTERVAL '365 days', if_not_exists => TRUE);
SELECT add_retention_policy('paper_trades', INTERVAL '180 days', if_not_exists => TRUE);
