//! IB Gateway/TWS client connection management.

use anyhow::{Context, Result};
use tracing::{info, warn};

/// IB client configuration.
#[derive(Debug, Clone)]
pub struct IBConfig {
    /// Gateway/TWS host (use 127.0.0.1, not localhost — TWS may block IPv6).
    pub host: String,
    /// Gateway port (4001 = live, 4002 = paper).
    pub port: u16,
    /// Client ID (unique per connection).
    pub client_id: i32,
}

impl Default for IBConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 4002, // Paper trading by default
            client_id: 100,
        }
    }
}

impl IBConfig {
    /// Paper trading configuration.
    pub fn paper() -> Self {
        Self::default()
    }

    /// Live trading configuration.
    pub fn live() -> Self {
        Self {
            port: 4001,
            ..Self::default()
        }
    }

    /// Connection URL for ibapi crate.
    pub fn connection_url(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Wrapper around ibapi::Client with reconnection and convenience methods.
pub struct IBClient {
    config: IBConfig,
    client: ibapi::Client,
}

impl IBClient {
    /// Connect to IB Gateway/TWS.
    pub async fn connect(config: IBConfig) -> Result<Self> {
        let url = config.connection_url();
        info!(url = %url, client_id = config.client_id, "Connecting to IB Gateway");

        let client = ibapi::Client::connect(&url, config.client_id)
            .await
            .context("Failed to connect to IB Gateway")?;

        info!("Connected to IB Gateway");
        Ok(Self { config, client })
    }

    /// Get a reference to the underlying ibapi client.
    pub fn inner(&self) -> &ibapi::Client {
        &self.client
    }

    /// Check if the connection is alive.
    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    /// Get the configuration.
    pub fn config(&self) -> &IBConfig {
        &self.config
    }
}
