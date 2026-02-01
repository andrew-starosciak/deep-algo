use anyhow::{Context, Result};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum BotCommand {
    Start,
    Stop,
    Pause,
    Resume,
    UpdateConfig(Box<BotConfig>),
    GetStatus(oneshot::Sender<BotStatus>),
    Shutdown,
}

/// Execution mode for bot trading
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    /// Live trading with real money (requires wallet configuration)
    #[default]
    Live,
    /// Paper trading with simulated fills (no real money, safe for testing)
    Paper,
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

    // Trading parameters
    #[serde(default = "default_initial_capital")]
    pub initial_capital: Decimal,
    #[serde(default = "default_risk_per_trade")]
    pub risk_per_trade_pct: f64,
    #[serde(default = "default_max_position")]
    pub max_position_pct: f64,

    // Hyperliquid-specific
    #[serde(default = "default_leverage")]
    pub leverage: u8,
    #[serde(default)]
    pub margin_mode: MarginMode,

    // Execution mode
    #[serde(default)]
    pub execution_mode: ExecutionMode,
    #[serde(default = "default_paper_slippage")]
    pub paper_slippage_bps: f64,
    #[serde(default = "default_paper_commission")]
    pub paper_commission_rate: f64,

    // Wallet configuration (loaded from env vars at runtime, not serialized)
    #[serde(skip)]
    pub wallet: Option<WalletConfig>,

    // Microstructure bridge configuration
    /// Enable microstructure signal filtering for strategy
    #[serde(default)]
    pub microstructure_enabled: bool,

    /// Entry filter: block entries when microstructure conflicts (0.0 to 1.0)
    #[serde(default = "default_entry_filter_threshold")]
    pub microstructure_entry_filter_threshold: f64,

    /// Exit trigger: force exit on liquidation cascade (0.0 to 1.0)
    #[serde(default = "default_exit_liquidation_threshold")]
    pub microstructure_exit_liquidation_threshold: f64,

    /// Exit trigger: force exit on extreme funding (0.0 to 1.0)
    #[serde(default = "default_exit_funding_threshold")]
    pub microstructure_exit_funding_threshold: f64,

    /// Sizing adjustment: reduce size under stress (0.0 to 1.0)
    #[serde(default = "default_stress_size_multiplier")]
    pub microstructure_stress_size_multiplier: f64,

    /// Enable entry timing based on order book support
    #[serde(default)]
    pub microstructure_entry_timing_enabled: bool,

    /// Entry timing: minimum order book support threshold (0.0 to 1.0)
    #[serde(default = "default_timing_support_threshold")]
    pub microstructure_timing_support_threshold: f64,
}

const fn default_entry_filter_threshold() -> f64 {
    0.6
}

const fn default_exit_liquidation_threshold() -> f64 {
    0.8
}

const fn default_exit_funding_threshold() -> f64 {
    0.9
}

const fn default_stress_size_multiplier() -> f64 {
    0.5
}

const fn default_timing_support_threshold() -> f64 {
    0.3
}

fn default_initial_capital() -> Decimal {
    Decimal::from(10000)
}

const fn default_risk_per_trade() -> f64 {
    0.05 // 5%
}

const fn default_max_position() -> f64 {
    0.20 // 20%
}

const fn default_leverage() -> u8 {
    1 // Conservative default
}

const fn default_paper_slippage() -> f64 {
    10.0 // 10 basis points (0.1%) - conservative estimate
}

const fn default_paper_commission() -> f64 {
    0.00025 // 0.025% - Hyperliquid taker fee
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

/// Wallet configuration for Hyperliquid trading
#[derive(Debug, Clone)]
pub struct WalletConfig {
    /// Master account address (42-character hex with 0x prefix)
    pub account_address: String,

    /// API wallet private key (66-character hex with 0x prefix)
    /// SECURITY: Loaded from env var, not stored in config file
    pub api_wallet_private_key: Option<String>,

    /// Nonce counter (atomic, per-wallet)
    pub nonce_counter: Arc<AtomicU64>,
}

impl WalletConfig {
    /// Load wallet from environment variables
    ///
    /// Expected env vars:
    /// - `HYPERLIQUID_ACCOUNT_ADDRESS`: Master account address
    /// - `HYPERLIQUID_API_WALLET_KEY`: API wallet private key
    ///
    /// # Errors
    /// Returns error if environment variables are missing or invalid format
    pub fn from_env() -> Result<Self> {
        let account_address = std::env::var("HYPERLIQUID_ACCOUNT_ADDRESS")
            .context("Missing HYPERLIQUID_ACCOUNT_ADDRESS env var")?;

        let api_wallet_private_key = std::env::var("HYPERLIQUID_API_WALLET_KEY")
            .context("Missing HYPERLIQUID_API_WALLET_KEY env var")?;

        // Validate address format (42-char hex)
        if !account_address.starts_with("0x") || account_address.len() != 42 {
            anyhow::bail!("Invalid account address format: must be 0x-prefixed 42-char hex");
        }

        // Validate private key format (66-char hex: 0x + 64 hex chars)
        if !api_wallet_private_key.starts_with("0x") || api_wallet_private_key.len() != 66 {
            anyhow::bail!("Invalid private key format: must be 0x-prefixed 66-char hex");
        }

        let timestamp = chrono::Utc::now().timestamp_millis();
        let nonce = u64::try_from(timestamp).context("Timestamp must be positive")?;

        Ok(Self {
            account_address,
            api_wallet_private_key: Some(api_wallet_private_key),
            nonce_counter: Arc::new(AtomicU64::new(nonce)),
        })
    }

    /// Get next nonce (atomic increment)
    #[must_use]
    pub fn next_nonce(&self) -> u64 {
        self.nonce_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Margin mode for trading
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum MarginMode {
    /// Cross margin (shares collateral across all positions)
    #[default]
    Cross,
    /// Isolated margin (collateral constrained to single asset)
    Isolated,
}
