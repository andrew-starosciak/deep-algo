//! Polymarket odds polling collector.
//!
//! Polls Polymarket CLOB for BTC-related market prices and emits
//! `PolymarketOddsRecord` for storage and signal processing.
//!
//! Supports both traditional CLOB markets and 15-minute Up/Down markets
//! via the Gamma API.

use crate::client::PolymarketClient;
use crate::gamma::GammaClient;
use crate::models::{Coin, Market};
use algo_trade_data::PolymarketOddsRecord;
use anyhow::Result;
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, Instant};

/// Configuration for the odds collector.
#[derive(Debug, Clone)]
pub struct OddsCollectorConfig {
    /// Polling interval for price updates
    pub poll_interval: Duration,
    /// How often to rediscover markets
    pub discovery_interval: Duration,
    /// Minimum liquidity for markets to track
    pub min_liquidity: Decimal,
    /// Maximum number of markets to track
    pub max_markets: usize,
    /// Coins to track for 15-minute markets (empty = disabled)
    pub coins_15min: Vec<Coin>,
    /// Whether to use Gamma API for 15-minute market discovery
    pub enable_15min_discovery: bool,
}

impl Default for OddsCollectorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30),
            discovery_interval: Duration::from_secs(3600), // 1 hour
            min_liquidity: Decimal::ZERO,
            max_markets: 50,
            coins_15min: vec![Coin::Btc], // Default to BTC only
            enable_15min_discovery: true,
        }
    }
}

impl OddsCollectorConfig {
    /// Creates a config with custom poll interval.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Creates a config with custom discovery interval.
    #[must_use]
    pub fn with_discovery_interval(mut self, interval: Duration) -> Self {
        self.discovery_interval = interval;
        self
    }

    /// Creates a config with minimum liquidity requirement.
    #[must_use]
    pub fn with_min_liquidity(mut self, liquidity: Decimal) -> Self {
        self.min_liquidity = liquidity;
        self
    }

    /// Creates a config with max markets limit.
    #[must_use]
    pub fn with_max_markets(mut self, max: usize) -> Self {
        self.max_markets = max;
        self
    }

    /// Sets coins for 15-minute market discovery.
    #[must_use]
    pub fn with_15min_coins(mut self, coins: Vec<Coin>) -> Self {
        self.coins_15min = coins;
        self
    }

    /// Enables or disables 15-minute market discovery.
    #[must_use]
    pub fn with_15min_discovery(mut self, enable: bool) -> Self {
        self.enable_15min_discovery = enable;
        self
    }
}

/// Statistics for the odds collector.
#[derive(Debug, Clone, Default)]
pub struct OddsCollectorStats {
    /// Total number of poll cycles completed
    pub poll_cycles: u64,
    /// Total number of records emitted
    pub records_emitted: u64,
    /// Current number of tracked markets
    pub tracked_markets: usize,
    /// Number of errors encountered
    pub errors: u64,
    /// Number of market discoveries performed
    pub discoveries: u64,
    /// Last successful poll timestamp
    pub last_poll: Option<chrono::DateTime<Utc>>,
}

impl OddsCollectorStats {
    /// Records a successful poll cycle.
    pub fn record_poll(&mut self, records: usize) {
        self.poll_cycles += 1;
        self.records_emitted += records as u64;
        self.last_poll = Some(Utc::now());
    }

    /// Records an error.
    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    /// Records a market discovery.
    pub fn record_discovery(&mut self, markets: usize) {
        self.discoveries += 1;
        self.tracked_markets = markets;
    }
}

/// Event types emitted by the collector.
#[derive(Debug, Clone)]
pub enum OddsCollectorEvent {
    /// Collector started
    Started,
    /// Markets discovered
    MarketsDiscovered { count: usize },
    /// Poll cycle completed
    PollCompleted { records: usize },
    /// Error occurred
    Error { message: String },
    /// Collector stopped
    Stopped,
}

/// Polymarket odds polling collector.
///
/// Continuously polls Polymarket for BTC-related market prices
/// and emits records through a channel.
///
/// Supports both traditional CLOB markets and 15-minute Up/Down markets
/// via the Gamma API.
pub struct OddsCollector {
    /// Polymarket CLOB API client
    client: PolymarketClient,
    /// Gamma API client for 15-minute markets
    gamma_client: GammaClient,
    /// Configuration
    config: OddsCollectorConfig,
    /// Output channel for odds records
    tx: mpsc::Sender<PolymarketOddsRecord>,
    /// Optional event channel for monitoring
    event_tx: Option<mpsc::Sender<OddsCollectorEvent>>,
    /// Currently tracked markets
    tracked_markets: Vec<Market>,
    /// Statistics
    stats: OddsCollectorStats,
}

impl OddsCollector {
    /// Creates a new odds collector.
    pub fn new(
        client: PolymarketClient,
        config: OddsCollectorConfig,
        tx: mpsc::Sender<PolymarketOddsRecord>,
    ) -> Self {
        Self {
            client,
            gamma_client: GammaClient::new(),
            config,
            tx,
            event_tx: None,
            tracked_markets: Vec::new(),
            stats: OddsCollectorStats::default(),
        }
    }

    /// Sets a custom Gamma client (useful for testing).
    #[must_use]
    pub fn with_gamma_client(mut self, gamma_client: GammaClient) -> Self {
        self.gamma_client = gamma_client;
        self
    }

    /// Sets an event channel for monitoring.
    #[must_use]
    pub fn with_event_channel(mut self, tx: mpsc::Sender<OddsCollectorEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Returns a reference to current statistics.
    pub fn stats(&self) -> &OddsCollectorStats {
        &self.stats
    }

    /// Returns the list of currently tracked markets.
    pub fn tracked_markets(&self) -> &[Market] {
        &self.tracked_markets
    }

    /// Discovers 15-minute markets using Gamma API.
    async fn discover_15min_markets(&self) -> Vec<Market> {
        if !self.config.enable_15min_discovery || self.config.coins_15min.is_empty() {
            return vec![];
        }

        tracing::info!(
            coins = ?self.config.coins_15min.iter().map(|c| c.slug_prefix()).collect::<Vec<_>>(),
            "Discovering 15-minute markets via Gamma API"
        );

        let markets = self
            .gamma_client
            .get_15min_markets_for_coins(&self.config.coins_15min)
            .await;

        tracing::info!(count = markets.len(), "Discovered 15-minute markets");

        markets
    }

    /// Discovers all markets (both CLOB and 15-minute).
    pub async fn discover_markets(&mut self) -> Result<usize> {
        tracing::info!("Discovering markets...");

        // Get 15-minute markets from Gamma API (prioritize these)
        let markets_15min = self.discover_15min_markets().await;
        let count_15min = markets_15min.len();

        // Get general BTC markets from CLOB API
        let markets_clob = if self.config.min_liquidity > Decimal::ZERO {
            self.client
                .discover_tradeable_btc_markets(self.config.min_liquidity)
                .await
                .unwrap_or_default()
        } else {
            self.client.discover_btc_markets().await.unwrap_or_default()
        };
        let count_clob = markets_clob.len();

        // Combine: 15-minute markets first, then CLOB markets
        let mut all_markets = markets_15min;
        all_markets.extend(markets_clob);

        // Deduplicate by condition ID
        let all_markets = deduplicate_markets(all_markets);

        // Limit number of tracked markets
        let markets: Vec<Market> = all_markets
            .into_iter()
            .take(self.config.max_markets)
            .collect();

        let count = markets.len();
        self.tracked_markets = markets;
        self.stats.record_discovery(count);

        self.emit_event(OddsCollectorEvent::MarketsDiscovered { count })
            .await;

        tracing::info!(
            total = count,
            from_15min = count_15min,
            from_clob = count_clob,
            "Market discovery complete"
        );
        Ok(count)
    }

    /// Polls current prices for all tracked markets and emits records.
    ///
    /// For 15-minute markets, fetches fresh prices from the Gamma API each cycle.
    /// For CLOB markets, uses cached prices from discovery.
    pub async fn poll_prices(&mut self) -> Result<usize> {
        let timestamp = Utc::now();
        let mut records_emitted = 0;

        // Fetch fresh 15-minute market prices from Gamma API
        if self.config.enable_15min_discovery && !self.config.coins_15min.is_empty() {
            let fresh_markets = self
                .gamma_client
                .get_15min_markets_for_coins(&self.config.coins_15min)
                .await;

            for market in fresh_markets {
                let (up_price, down_price) = match (market.up_price(), market.down_price()) {
                    (Some(up), Some(down)) => (up, down),
                    _ => continue,
                };

                let record = PolymarketOddsRecord::new(
                    timestamp,
                    market.condition_id.clone(),
                    market.question.clone(),
                    up_price,
                    down_price,
                )
                .with_metadata(market.volume_24h, market.liquidity, market.end_date);

                if self.tx.send(record).await.is_err() {
                    tracing::warn!("Odds record channel closed");
                    break;
                }

                records_emitted += 1;

                tracing::debug!(
                    market_id = %market.condition_id,
                    up_price = %up_price,
                    down_price = %down_price,
                    "Polled fresh 15-min market price"
                );
            }
        }

        // For non-15min markets, use cached prices from tracked_markets
        for market in &self.tracked_markets {
            // Skip 15-min markets (already handled above with fresh prices)
            if market.is_15min_market() {
                continue;
            }

            let (yes_price, no_price) = match (market.yes_price(), market.no_price()) {
                (Some(y), Some(n)) => (y, n),
                _ => continue,
            };

            let record = PolymarketOddsRecord::new(
                timestamp,
                market.condition_id.clone(),
                market.question.clone(),
                yes_price,
                no_price,
            )
            .with_metadata(market.volume_24h, market.liquidity, market.end_date);

            if self.tx.send(record).await.is_err() {
                tracing::warn!("Odds record channel closed");
                break;
            }

            records_emitted += 1;
        }

        self.stats.record_poll(records_emitted);
        self.emit_event(OddsCollectorEvent::PollCompleted {
            records: records_emitted,
        })
        .await;

        Ok(records_emitted)
    }

    /// Runs the collector indefinitely.
    ///
    /// This method polls prices at regular intervals and periodically
    /// rediscovers markets.
    pub async fn run(&mut self) -> Result<()> {
        self.emit_event(OddsCollectorEvent::Started).await;

        // Initial market discovery
        if let Err(e) = self.discover_markets().await {
            tracing::error!("Initial market discovery failed: {}", e);
            self.stats.record_error();
            self.emit_event(OddsCollectorEvent::Error {
                message: e.to_string(),
            })
            .await;
        }

        let mut poll_interval = interval(self.config.poll_interval);
        let mut last_discovery = Instant::now();

        loop {
            poll_interval.tick().await;

            // Check if we need to rediscover markets
            if last_discovery.elapsed() >= self.config.discovery_interval {
                if let Err(e) = self.discover_markets().await {
                    tracing::error!("Market discovery failed: {}", e);
                    self.stats.record_error();
                    self.emit_event(OddsCollectorEvent::Error {
                        message: e.to_string(),
                    })
                    .await;
                }
                last_discovery = Instant::now();
            }

            // Poll prices
            match self.poll_prices().await {
                Ok(count) => {
                    tracing::debug!("Polled {} market prices", count);
                }
                Err(e) => {
                    tracing::error!("Price poll failed: {}", e);
                    self.stats.record_error();
                    self.emit_event(OddsCollectorEvent::Error {
                        message: e.to_string(),
                    })
                    .await;
                }
            }

            // Check if channel is still open
            if self.tx.is_closed() {
                tracing::info!("Output channel closed, stopping collector");
                break;
            }
        }

        self.emit_event(OddsCollectorEvent::Stopped).await;
        Ok(())
    }

    /// Emits an event if the event channel is configured.
    async fn emit_event(&self, event: OddsCollectorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event).await;
        }
    }
}

/// Filters a list of markets to only include BTC-related ones.
pub fn filter_btc_markets(markets: Vec<Market>) -> Vec<Market> {
    markets.into_iter().filter(|m| m.is_btc_related()).collect()
}

/// Deduplicates markets by condition ID, keeping the first occurrence.
pub fn deduplicate_markets(markets: Vec<Market>) -> Vec<Market> {
    let mut seen = HashSet::new();
    markets
        .into_iter()
        .filter(|m| seen.insert(m.condition_id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamma::GammaClient;
    use crate::models::Token;
    use rust_decimal_macros::dec;

    fn sample_btc_market() -> Market {
        Market {
            condition_id: "btc-100k".to_string(),
            question: "Will Bitcoin hit $100k?".to_string(),
            description: None,
            end_date: None,
            tokens: vec![
                Token {
                    token_id: "yes-1".to_string(),
                    outcome: "Yes".to_string(),
                    price: dec!(0.65),
                    winner: None,
                },
                Token {
                    token_id: "no-1".to_string(),
                    outcome: "No".to_string(),
                    price: dec!(0.35),
                    winner: None,
                },
            ],
            active: true,
            tags: None,
            volume_24h: Some(dec!(50000)),
            liquidity: Some(dec!(100000)),
        }
    }

    fn sample_eth_market() -> Market {
        Market {
            condition_id: "eth-10k".to_string(),
            question: "Will Ethereum hit $10k?".to_string(),
            description: None,
            end_date: None,
            tokens: vec![],
            active: true,
            tags: None,
            volume_24h: None,
            liquidity: None,
        }
    }

    fn sample_15min_market() -> Market {
        Market {
            condition_id: "btc-15min-up".to_string(),
            question: "Bitcoin Up or Down - 15 min".to_string(),
            description: None,
            end_date: None,
            tokens: vec![
                Token {
                    token_id: "up-token".to_string(),
                    outcome: "Up".to_string(),
                    price: dec!(0.55),
                    winner: None,
                },
                Token {
                    token_id: "down-token".to_string(),
                    outcome: "Down".to_string(),
                    price: dec!(0.45),
                    winner: None,
                },
            ],
            active: true,
            tags: Some(vec!["15min".to_string()]),
            volume_24h: None,
            liquidity: Some(dec!(10000)),
        }
    }

    // ========== OddsCollectorConfig Tests ==========

    #[test]
    fn test_config_default() {
        let config = OddsCollectorConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(30));
        assert_eq!(config.discovery_interval, Duration::from_secs(3600));
        assert_eq!(config.min_liquidity, Decimal::ZERO);
        assert_eq!(config.max_markets, 50);
        assert_eq!(config.coins_15min, vec![Coin::Btc]);
        assert!(config.enable_15min_discovery);
    }

    #[test]
    fn test_config_with_15min_coins() {
        let config =
            OddsCollectorConfig::default().with_15min_coins(vec![Coin::Btc, Coin::Eth, Coin::Sol]);
        assert_eq!(config.coins_15min.len(), 3);
        assert!(config.coins_15min.contains(&Coin::Btc));
        assert!(config.coins_15min.contains(&Coin::Eth));
        assert!(config.coins_15min.contains(&Coin::Sol));
    }

    #[test]
    fn test_config_with_15min_discovery_disabled() {
        let config = OddsCollectorConfig::default().with_15min_discovery(false);
        assert!(!config.enable_15min_discovery);
    }

    #[test]
    fn test_config_with_poll_interval() {
        let config = OddsCollectorConfig::default().with_poll_interval(Duration::from_secs(10));
        assert_eq!(config.poll_interval, Duration::from_secs(10));
    }

    #[test]
    fn test_config_with_discovery_interval() {
        let config =
            OddsCollectorConfig::default().with_discovery_interval(Duration::from_secs(1800));
        assert_eq!(config.discovery_interval, Duration::from_secs(1800));
    }

    #[test]
    fn test_config_with_min_liquidity() {
        let config = OddsCollectorConfig::default().with_min_liquidity(dec!(10000));
        assert_eq!(config.min_liquidity, dec!(10000));
    }

    #[test]
    fn test_config_with_max_markets() {
        let config = OddsCollectorConfig::default().with_max_markets(100);
        assert_eq!(config.max_markets, 100);
    }

    #[test]
    fn test_config_builder_chain() {
        let config = OddsCollectorConfig::default()
            .with_poll_interval(Duration::from_secs(15))
            .with_min_liquidity(dec!(5000))
            .with_max_markets(25);

        assert_eq!(config.poll_interval, Duration::from_secs(15));
        assert_eq!(config.min_liquidity, dec!(5000));
        assert_eq!(config.max_markets, 25);
    }

    // ========== OddsCollectorStats Tests ==========

    #[test]
    fn test_stats_default() {
        let stats = OddsCollectorStats::default();
        assert_eq!(stats.poll_cycles, 0);
        assert_eq!(stats.records_emitted, 0);
        assert_eq!(stats.tracked_markets, 0);
        assert_eq!(stats.errors, 0);
        assert!(stats.last_poll.is_none());
    }

    #[test]
    fn test_stats_record_poll() {
        let mut stats = OddsCollectorStats::default();
        stats.record_poll(5);

        assert_eq!(stats.poll_cycles, 1);
        assert_eq!(stats.records_emitted, 5);
        assert!(stats.last_poll.is_some());
    }

    #[test]
    fn test_stats_record_multiple_polls() {
        let mut stats = OddsCollectorStats::default();
        stats.record_poll(5);
        stats.record_poll(3);
        stats.record_poll(7);

        assert_eq!(stats.poll_cycles, 3);
        assert_eq!(stats.records_emitted, 15);
    }

    #[test]
    fn test_stats_record_error() {
        let mut stats = OddsCollectorStats::default();
        stats.record_error();
        stats.record_error();

        assert_eq!(stats.errors, 2);
    }

    #[test]
    fn test_stats_record_discovery() {
        let mut stats = OddsCollectorStats::default();
        stats.record_discovery(10);

        assert_eq!(stats.discoveries, 1);
        assert_eq!(stats.tracked_markets, 10);

        stats.record_discovery(5);
        assert_eq!(stats.discoveries, 2);
        assert_eq!(stats.tracked_markets, 5);
    }

    // ========== OddsCollector Tests ==========

    #[tokio::test]
    async fn test_collector_creation() {
        let client = PolymarketClient::new();
        let config = OddsCollectorConfig::default();
        let (tx, _rx) = mpsc::channel(100);

        let collector = OddsCollector::new(client, config.clone(), tx);

        assert!(collector.tracked_markets().is_empty());
        assert_eq!(collector.stats().poll_cycles, 0);
    }

    #[tokio::test]
    async fn test_collector_with_event_channel() {
        let client = PolymarketClient::new();
        let config = OddsCollectorConfig::default();
        let (tx, _rx) = mpsc::channel(100);
        let (event_tx, _event_rx) = mpsc::channel(100);

        let _collector = OddsCollector::new(client, config, tx).with_event_channel(event_tx);
    }

    #[tokio::test]
    async fn test_poll_prices_empty_markets() {
        let client = PolymarketClient::new();
        // Disable 15-min discovery for this test (tests cached markets only)
        let config = OddsCollectorConfig::default().with_15min_discovery(false);
        let (tx, _rx) = mpsc::channel(100);

        let mut collector = OddsCollector::new(client, config, tx);
        let count = collector.poll_prices().await.unwrap();

        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_poll_prices_with_markets() {
        let client = PolymarketClient::new();
        // Disable 15-min discovery for this test (tests cached markets only)
        let config = OddsCollectorConfig::default().with_15min_discovery(false);
        let (tx, mut rx) = mpsc::channel(100);

        let mut collector = OddsCollector::new(client, config, tx);
        collector.tracked_markets = vec![sample_btc_market()];

        let count = collector.poll_prices().await.unwrap();

        assert_eq!(count, 1);
        assert_eq!(collector.stats().records_emitted, 1);

        // Verify record was sent
        let record = rx.recv().await.unwrap();
        assert_eq!(record.market_id, "btc-100k");
        assert_eq!(record.outcome_yes_price, dec!(0.65));
        assert_eq!(record.outcome_no_price, dec!(0.35));
    }

    #[tokio::test]
    async fn test_poll_prices_skips_markets_without_prices() {
        let client = PolymarketClient::new();
        // Disable 15-min discovery for this test (tests cached markets only)
        let config = OddsCollectorConfig::default().with_15min_discovery(false);
        let (tx, _rx) = mpsc::channel(100);

        let mut collector = OddsCollector::new(client, config, tx);
        collector.tracked_markets = vec![
            sample_btc_market(),
            sample_eth_market(), // No tokens/prices
        ];

        let count = collector.poll_prices().await.unwrap();

        // Only BTC market should be polled (ETH has no prices)
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_poll_prices_with_15min_markets_mocked() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        // Mock Gamma API response
        Mock::given(method("GET"))
            .and(path("/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "slug": "btc-updown-15m-test",
                    "markets": [{
                        "conditionId": "0xtest-15min",
                        "outcomePrices": "[\"0.55\", \"0.45\"]",
                        "clobTokenIds": "[\"up-token\", \"down-token\"]",
                        "liquidity": "10000",
                        "question": "BTC 15-min test",
                        "active": true
                    }]
                }
            ])))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new();
        let config = OddsCollectorConfig::default().with_15min_coins(vec![Coin::Btc]);
        let (tx, mut rx) = mpsc::channel(100);

        let gamma_client = GammaClient::new().with_base_url(mock_server.uri());
        let mut collector = OddsCollector::new(client, config, tx).with_gamma_client(gamma_client);

        let count = collector.poll_prices().await.unwrap();

        assert_eq!(count, 1);

        // Verify record was sent with Up/Down prices
        let record = rx.recv().await.unwrap();
        assert_eq!(record.market_id, "0xtest-15min");
        assert_eq!(record.outcome_yes_price, dec!(0.55));
        assert_eq!(record.outcome_no_price, dec!(0.45));
    }

    #[tokio::test]
    async fn test_poll_prices_mixed_markets_mocked() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        // Mock Gamma API response for 15-min market
        Mock::given(method("GET"))
            .and(path("/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "slug": "btc-updown-15m-test",
                    "markets": [{
                        "conditionId": "0xtest-15min",
                        "outcomePrices": "[\"0.60\", \"0.40\"]",
                        "clobTokenIds": "[\"up\", \"down\"]",
                        "active": true
                    }]
                }
            ])))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new();
        let config = OddsCollectorConfig::default().with_15min_coins(vec![Coin::Btc]);
        let (tx, mut rx) = mpsc::channel(100);

        let gamma_client = GammaClient::new().with_base_url(mock_server.uri());
        let mut collector = OddsCollector::new(client, config, tx).with_gamma_client(gamma_client);
        // Add a cached CLOB market
        collector.tracked_markets = vec![sample_btc_market()];

        let count = collector.poll_prices().await.unwrap();

        // 1 from Gamma API (15-min) + 1 from cached (CLOB)
        assert_eq!(count, 2);

        // First record: 15-min market (fetched fresh)
        let record1 = rx.recv().await.unwrap();
        assert_eq!(record1.market_id, "0xtest-15min");
        assert_eq!(record1.outcome_yes_price, dec!(0.60));

        // Second record: CLOB market (cached)
        let record2 = rx.recv().await.unwrap();
        assert_eq!(record2.market_id, "btc-100k");
        assert_eq!(record2.outcome_yes_price, dec!(0.65));
    }

    #[tokio::test]
    async fn test_poll_prices_handles_closed_channel() {
        let client = PolymarketClient::new();
        // Disable 15-min discovery for this test
        let config = OddsCollectorConfig::default().with_15min_discovery(false);
        let (tx, rx) = mpsc::channel(1);

        let mut collector = OddsCollector::new(client, config, tx);
        collector.tracked_markets = vec![sample_btc_market(), sample_btc_market()];

        // Drop receiver to close channel
        drop(rx);

        // Should handle gracefully
        let _count = collector.poll_prices().await.unwrap();
    }

    // ========== Helper Function Tests ==========

    #[test]
    fn test_filter_btc_markets() {
        let markets = vec![
            sample_btc_market(),
            sample_eth_market(),
            Market {
                condition_id: "btc-crash".to_string(),
                question: "Will BTC crash 50%?".to_string(),
                description: None,
                end_date: None,
                tokens: vec![],
                active: true,
                tags: None,
                volume_24h: None,
                liquidity: None,
            },
        ];

        let btc_markets = filter_btc_markets(markets);

        assert_eq!(btc_markets.len(), 2);
        assert!(btc_markets.iter().all(|m| m.is_btc_related()));
    }

    #[test]
    fn test_filter_btc_markets_empty() {
        let markets = vec![sample_eth_market()];
        let btc_markets = filter_btc_markets(markets);
        assert!(btc_markets.is_empty());
    }

    #[test]
    fn test_deduplicate_markets() {
        let markets = vec![
            sample_btc_market(),
            sample_btc_market(), // Duplicate
            sample_eth_market(),
        ];

        let unique = deduplicate_markets(markets);

        assert_eq!(unique.len(), 2);
        assert_eq!(unique[0].condition_id, "btc-100k");
        assert_eq!(unique[1].condition_id, "eth-10k");
    }

    #[test]
    fn test_deduplicate_markets_preserves_order() {
        let m1 = Market {
            condition_id: "a".to_string(),
            question: "First".to_string(),
            description: None,
            end_date: None,
            tokens: vec![],
            active: true,
            tags: None,
            volume_24h: None,
            liquidity: None,
        };

        let m2 = Market {
            condition_id: "b".to_string(),
            question: "Second".to_string(),
            description: None,
            end_date: None,
            tokens: vec![],
            active: true,
            tags: None,
            volume_24h: None,
            liquidity: None,
        };

        let m3 = Market {
            condition_id: "a".to_string(), // Duplicate of m1
            question: "First (copy)".to_string(),
            description: None,
            end_date: None,
            tokens: vec![],
            active: true,
            tags: None,
            volume_24h: None,
            liquidity: None,
        };

        let markets = vec![m1, m2, m3];
        let unique = deduplicate_markets(markets);

        assert_eq!(unique.len(), 2);
        assert_eq!(unique[0].question, "First"); // First occurrence kept
        assert_eq!(unique[1].condition_id, "b");
    }

    // ========== Event Tests ==========

    #[tokio::test]
    async fn test_collector_emits_events() {
        let client = PolymarketClient::new();
        // Disable 15-min discovery for this test
        let config = OddsCollectorConfig::default().with_15min_discovery(false);
        let (tx, _rx) = mpsc::channel(100);
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let mut collector = OddsCollector::new(client, config, tx).with_event_channel(event_tx);

        collector.tracked_markets = vec![sample_btc_market()];

        // Poll should emit event
        collector.poll_prices().await.unwrap();

        // Check for PollCompleted event
        let event = event_rx.recv().await.unwrap();
        match event {
            OddsCollectorEvent::PollCompleted { records } => {
                assert_eq!(records, 1);
            }
            _ => panic!("Expected PollCompleted event"),
        }
    }
}
