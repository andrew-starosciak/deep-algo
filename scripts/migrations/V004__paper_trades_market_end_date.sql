-- V004: Add market_end_date to paper_trades
-- This column stores the actual market end time for proper settlement timing.
-- Previously, settlement used trade.timestamp + 15min which was incorrect.

-- Add market_end_date column
ALTER TABLE paper_trades
ADD COLUMN IF NOT EXISTS market_end_date TIMESTAMPTZ;

-- Add index for settlement queries
CREATE INDEX IF NOT EXISTS idx_paper_trades_settlement
ON paper_trades (market_end_date, status)
WHERE status = 'pending';

-- Update comment
COMMENT ON COLUMN paper_trades.market_end_date IS 'Actual market end time from Polymarket for settlement timing';
