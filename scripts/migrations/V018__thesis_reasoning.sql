-- V018: Capture analyst and critic reasoning on theses
-- The bull/bear debate (analyst) and counter-case (critic) are valuable
-- for post-trade review and pattern analysis on winners vs losers.

ALTER TABLE theses ADD COLUMN IF NOT EXISTS analyst_reasoning TEXT;
ALTER TABLE theses ADD COLUMN IF NOT EXISTS critic_reasoning TEXT;
