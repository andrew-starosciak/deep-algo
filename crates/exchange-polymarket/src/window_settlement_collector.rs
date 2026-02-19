//! Window settlement collector.
//!
//! Polls the Gamma API every 30 seconds to check for resolved 15-minute
//! market windows and records the settlement outcome.

use algo_trade_data::WindowSettlementRecord;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashSet;
use tokio::sync::mpsc;

use crate::gamma::GammaClient;
use crate::models::Coin;

/// Collector that polls for resolved 15-minute window settlements.
pub struct WindowSettlementCollector {
    gamma: GammaClient,
    coins: Vec<Coin>,
    tx: mpsc::Sender<WindowSettlementRecord>,
    session_id: Option<String>,
    /// Set of (coin_slug, window_start_ts) already settled — avoids duplicates.
    settled: HashSet<(String, i64)>,
}

impl WindowSettlementCollector {
    /// Creates a new settlement collector.
    pub fn new(
        gamma: GammaClient,
        coins: Vec<Coin>,
        tx: mpsc::Sender<WindowSettlementRecord>,
    ) -> Self {
        Self {
            gamma,
            coins,
            tx,
            session_id: None,
            settled: HashSet::new(),
        }
    }

    /// Sets an optional session ID for tagging records.
    #[must_use]
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Runs the collector loop, polling every 30 seconds.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            interval.tick().await;

            let now = Utc::now();
            self.check_settlements(now).await;
            self.prune_old_entries(now);
        }
    }

    /// Checks all recent closed windows for settlements.
    async fn check_settlements(&mut self, now: DateTime<Utc>) {
        // Check the last 2 hours of windows (8 windows per coin)
        let window_count = 8;

        for coin in self.coins.clone() {
            for i in 1..=window_count {
                // Calculate window boundaries: each window is 15 minutes
                let window_end_approx = now - Duration::minutes(15 * (i - 1));
                let window_start_ts = GammaClient::calculate_window_timestamp(
                    window_end_approx - Duration::minutes(15),
                );
                let window_end_ts = window_start_ts + 900; // 15 minutes in seconds

                let window_start =
                    DateTime::from_timestamp(window_start_ts, 0).unwrap_or(now);
                let window_end =
                    DateTime::from_timestamp(window_end_ts, 0).unwrap_or(now);

                // Skip if window hasn't been closed long enough (need 2+ min for resolution)
                let min_settle_time = window_end + Duration::minutes(2);
                if now < min_settle_time {
                    continue;
                }

                // Skip if already settled
                let key = (coin.slug_prefix().to_string(), window_start_ts);
                if self.settled.contains(&key) {
                    continue;
                }

                // Query Gamma API for outcome
                match self.gamma.get_market_outcome(coin, window_end).await {
                    Ok(Some(outcome)) => {
                        // Get slug and condition_id for metadata
                        let slug =
                            GammaClient::generate_event_slug(coin, window_start_ts);
                        let condition_id = self
                            .gamma
                            .get_15min_event(coin, window_start)
                            .await
                            .ok()
                            .and_then(|event| {
                                event
                                    .markets
                                    .first()
                                    .map(|m| m.condition_id.clone())
                            });

                        let record = WindowSettlementRecord {
                            window_start,
                            coin: coin.slug_prefix().to_string(),
                            window_end,
                            outcome,
                            settlement_source: "gamma".to_string(),
                            gamma_slug: Some(slug),
                            condition_id,
                            settled_at: Utc::now(),
                            session_id: self.session_id.clone(),
                        };

                        if self.tx.send(record).await.is_err() {
                            tracing::info!(
                                "Settlement collector channel closed, stopping"
                            );
                            return;
                        }

                        self.settled.insert(key);

                        tracing::info!(
                            coin = coin.slug_prefix(),
                            window_start = %window_start,
                            "Recorded window settlement"
                        );
                    }
                    Ok(None) => {
                        // Not yet resolved — will retry next cycle
                        tracing::debug!(
                            coin = coin.slug_prefix(),
                            window_start = %window_start,
                            "Window not yet resolved"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            coin = coin.slug_prefix(),
                            window_start = %window_start,
                            error = %e,
                            "Failed to check settlement"
                        );
                    }
                }
            }
        }
    }

    /// Removes entries older than 3 hours from the settled set.
    fn prune_old_entries(&mut self, now: DateTime<Utc>) {
        let cutoff = now.timestamp() - 3 * 3600; // 3 hours ago
        self.settled.retain(|(_coin, ts)| *ts > cutoff);
    }
}
