-- Bot configurations table
CREATE TABLE IF NOT EXISTS bot_configs (
    bot_id TEXT PRIMARY KEY NOT NULL,
    config_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Bot runtime state table
CREATE TABLE IF NOT EXISTS bot_runtime_state (
    bot_id TEXT PRIMARY KEY NOT NULL,
    state TEXT NOT NULL,
    started_at INTEGER,
    last_heartbeat INTEGER NOT NULL,
    FOREIGN KEY (bot_id) REFERENCES bot_configs(bot_id) ON DELETE CASCADE
);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_bot_configs_enabled ON bot_configs(enabled);
CREATE INDEX IF NOT EXISTS idx_bot_configs_created ON bot_configs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_bot_runtime_state_heartbeat ON bot_runtime_state(last_heartbeat DESC);
