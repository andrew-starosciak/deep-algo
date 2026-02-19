-- V016: Thesis outcome tracking
-- Denormalize outcomes onto theses directly so we can show historical
-- thesis performance without a 3-table JOIN on every read.

ALTER TABLE theses ADD COLUMN IF NOT EXISTS outcome_realized_pnl DECIMAL;
ALTER TABLE theses ADD COLUMN IF NOT EXISTS outcome_close_reason TEXT;
ALTER TABLE theses ADD COLUMN IF NOT EXISTS outcome_closed_at TIMESTAMPTZ;
ALTER TABLE theses ADD COLUMN IF NOT EXISTS outcome_position_id BIGINT
    REFERENCES options_positions(id);
