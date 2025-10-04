use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub hyperliquid: HyperliquidConfig,
    #[serde(default)]
    pub backtest_scheduler: BacktestSchedulerConfig,
    #[serde(default)]
    pub token_selector: TokenSelectorConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperliquidConfig {
    pub api_url: String,
    pub ws_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestSchedulerConfig {
    pub enabled: bool,
    pub cron_schedule: String,
    pub fetch_universe_from_exchange: bool,
    pub token_universe: Option<Vec<String>>,
    pub backtest_window_days: i64,
    pub strategy_name: String,
    pub exchange: String,
    pub hyperliquid_api_url: String,
}

impl Default for BacktestSchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cron_schedule: "0 0 3 * * *".to_string(),
            fetch_universe_from_exchange: true,
            token_universe: None,
            backtest_window_days: 30,
            strategy_name: "quad_ma".to_string(),
            exchange: "hyperliquid".to_string(),
            hyperliquid_api_url: "https://api.hyperliquid.xyz".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSelectorConfig {
    pub min_sharpe_ratio: f64,
    pub min_win_rate: f64,
    pub max_drawdown: f64,
    pub min_num_trades: i32,
    pub lookback_hours: i64,
}

impl Default for TokenSelectorConfig {
    fn default() -> Self {
        Self {
            min_sharpe_ratio: 1.0,
            min_win_rate: 0.50,
            max_drawdown: 0.20,
            min_num_trades: 10,
            lookback_hours: 48,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
            },
            database: DatabaseConfig {
                url: "postgresql://localhost/algo_trade".to_string(),
                max_connections: 10,
            },
            hyperliquid: HyperliquidConfig {
                api_url: "https://api.hyperliquid.xyz".to_string(),
                ws_url: "wss://api.hyperliquid.xyz/ws".to_string(),
            },
            backtest_scheduler: BacktestSchedulerConfig::default(),
            token_selector: TokenSelectorConfig::default(),
        }
    }
}
