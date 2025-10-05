use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum BotCommand {
    Start,
    Stop,
    Pause,
    Resume,
    UpdateConfig(BotConfig),
    GetStatus(oneshot::Sender<BotStatus>),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
    pub enabled: bool,
    pub interval: String,
    pub ws_url: String,
    pub api_url: String,
    pub warmup_periods: usize,
    pub strategy_config: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotStatus {
    pub bot_id: String,
    pub state: BotState,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BotState {
    Stopped,
    Running,
    Paused,
    Error,
}
