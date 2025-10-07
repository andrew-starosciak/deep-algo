use crate::commands::BotConfig;
use anyhow::Result;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

/// `SQLite` database for persistent bot configuration storage.
///
/// Provides async CRUD operations for bot configs and runtime state tracking.
/// Uses connection pooling for concurrent access.
#[derive(Clone)]
pub struct BotDatabase {
    pool: SqlitePool,
}

impl BotDatabase {
    /// Creates a new database connection pool.
    ///
    /// # Arguments
    ///
    /// * `database_url` - `SQLite` database path (e.g., `<sqlite://bots.db>`)
    ///
    /// # Errors
    ///
    /// Returns error if connection fails or migrations fail.
    pub async fn new(database_url: &str) -> Result<Self> {
        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await?;

        Ok(Self { pool })
    }

    /// Creates an in-memory database for testing.
    ///
    /// # Errors
    ///
    /// Returns error if connection fails.
    #[cfg(test)]
    pub async fn new_in_memory() -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await?;

        Ok(Self { pool })
    }

    /// Inserts or updates a bot configuration in the database.
    ///
    /// # Arguments
    ///
    /// * `config` - Bot configuration to persist
    ///
    /// # Errors
    ///
    /// Returns error if serialization or database operation fails.
    pub async fn insert_bot(&self, config: &BotConfig) -> Result<()> {
        let config_json = serde_json::to_string(config)?;
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            r"
            INSERT INTO bot_configs (bot_id, config_json, enabled, created_at, updated_at)
            VALUES (?1, ?2, 1, ?3, ?3)
            ON CONFLICT(bot_id) DO UPDATE SET
                config_json = excluded.config_json,
                updated_at = excluded.updated_at
            "
        )
        .bind(&config.bot_id)
        .bind(config_json)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Loads all bot configurations from the database.
    ///
    /// # Errors
    ///
    /// Returns error if database query or deserialization fails.
    pub async fn load_all_bots(&self) -> Result<Vec<BotConfig>> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT config_json FROM bot_configs ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;

        let mut configs = Vec::new();
        for (config_json,) in rows {
            let config: BotConfig = serde_json::from_str(&config_json)?;
            configs.push(config);
        }

        Ok(configs)
    }

    /// Updates the runtime state of a bot.
    ///
    /// # Arguments
    ///
    /// * `bot_id` - Bot identifier
    /// * `state` - Bot state (Stopped/Running/Paused/Error)
    /// * `started_at` - Optional start timestamp
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails.
    pub async fn update_bot_state(
        &self,
        bot_id: &str,
        state: &str,
        started_at: Option<i64>,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            r"
            INSERT INTO bot_runtime_state (bot_id, state, started_at, last_heartbeat)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(bot_id) DO UPDATE SET
                state = excluded.state,
                started_at = excluded.started_at,
                last_heartbeat = excluded.last_heartbeat
            "
        )
        .bind(bot_id)
        .bind(state)
        .bind(started_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Deletes a bot configuration from the database.
    ///
    /// # Arguments
    ///
    /// * `bot_id` - Bot identifier to delete
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails.
    pub async fn delete_bot(&self, bot_id: &str) -> Result<()> {
        // Delete runtime state first (foreign key constraint)
        sqlx::query("DELETE FROM bot_runtime_state WHERE bot_id = ?1")
            .bind(bot_id)
            .execute(&self.pool)
            .await?;

        // Delete config
        sqlx::query("DELETE FROM bot_configs WHERE bot_id = ?1")
            .bind(bot_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Loads only enabled bot configurations (for auto-restore).
    ///
    /// # Errors
    ///
    /// Returns error if database query or deserialization fails.
    pub async fn get_enabled_bots(&self) -> Result<Vec<BotConfig>> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT config_json FROM bot_configs WHERE enabled = 1 ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;

        let mut configs = Vec::new();
        for (config_json,) in rows {
            let config: BotConfig = serde_json::from_str(&config_json)?;
            configs.push(config);
        }

        Ok(configs)
    }
}
