-- V014: Position sync support
-- Adds IB contract ID for matching positions between IB and our DB.

ALTER TABLE options_positions ADD COLUMN IF NOT EXISTS ib_con_id BIGINT;

CREATE INDEX IF NOT EXISTS idx_options_positions_con_id
    ON options_positions(ib_con_id) WHERE ib_con_id IS NOT NULL;
