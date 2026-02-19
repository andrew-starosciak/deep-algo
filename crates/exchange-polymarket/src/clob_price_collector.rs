//! CLOB price snapshot collector.
//!
//! Polls the Gamma API every 5 seconds to capture current CLOB prices
//! for all configured coins' 15-minute Up/Down markets.

use algo_trade_data::ClobPriceSnapshotRecord;
use chrono::Utc;
use tokio::sync::mpsc;

use crate::gamma::GammaClient;
use crate::models::Coin;

/// Collector that fetches CLOB prices per-coin to guarantee correct coin attribution.
///
/// Iterates through each configured coin individually so the coin slug is
/// always known (unlike batch fetching where ordering is non-deterministic).
pub struct ClobPricePerCoinCollector {
    gamma: GammaClient,
    coins: Vec<Coin>,
    tx: mpsc::Sender<ClobPriceSnapshotRecord>,
    session_id: Option<String>,
}

impl ClobPricePerCoinCollector {
    /// Creates a new per-coin collector.
    pub fn new(
        gamma: GammaClient,
        coins: Vec<Coin>,
        tx: mpsc::Sender<ClobPriceSnapshotRecord>,
    ) -> Self {
        Self {
            gamma,
            coins,
            tx,
            session_id: None,
        }
    }

    /// Sets an optional session ID for tagging snapshots.
    #[must_use]
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Runs the collector loop, polling every 5 seconds.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));

        loop {
            interval.tick().await;

            let now = Utc::now();

            for coin in &self.coins {
                match self.gamma.get_current_15min_market(*coin).await {
                    Ok(market) => {
                        let up_token = match market.up_token() {
                            Some(t) => t,
                            None => continue,
                        };
                        let down_token = match market.down_token() {
                            Some(t) => t,
                            None => continue,
                        };

                        let record = ClobPriceSnapshotRecord {
                            timestamp: now,
                            coin: coin.slug_prefix().to_string(),
                            up_price: up_token.price,
                            down_price: down_token.price,
                            up_token_id: up_token.token_id.clone(),
                            down_token_id: down_token.token_id.clone(),
                            session_id: self.session_id.clone(),
                        };

                        if self.tx.send(record).await.is_err() {
                            tracing::info!("CLOB price collector channel closed, stopping");
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            coin = coin.slug_prefix(),
                            error = %e,
                            "Failed to fetch CLOB price for coin"
                        );
                    }
                }
            }

            tracing::debug!(coins = self.coins.len(), "Collected CLOB price snapshots");
        }
    }
}
