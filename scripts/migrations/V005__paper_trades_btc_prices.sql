-- V005: Add BTC price tracking columns to paper_trades
-- Captures BTC price at window start, entry time, and settlement for analysis.

-- BTC price at the start of the 15-minute window
ALTER TABLE paper_trades
ADD COLUMN IF NOT EXISTS btc_price_window_start DECIMAL(20, 8);

-- BTC price at the moment of trade entry
ALTER TABLE paper_trades
ADD COLUMN IF NOT EXISTS btc_price_at_entry DECIMAL(20, 8);

-- BTC price at window end (settlement)
ALTER TABLE paper_trades
ADD COLUMN IF NOT EXISTS btc_price_window_end DECIMAL(20, 8);

-- Comments for documentation
COMMENT ON COLUMN paper_trades.btc_price_window_start IS 'BTC price at the start of the 15-min window (:00/:15/:30/:45)';
COMMENT ON COLUMN paper_trades.btc_price_at_entry IS 'BTC price at the moment the trade was placed';
COMMENT ON COLUMN paper_trades.btc_price_window_end IS 'BTC price at window end for settlement (set during settlement)';
