//! CryptoPanic news collector.
//!
//! Polls CryptoPanic API for BTC-related news and calculates urgency scores
//! based on category and recency.

use crate::collector::types::{CollectorEvent, CollectorStats};
use algo_trade_data::{NewsEventRecord, NewsSentiment};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, Instant};

/// CryptoPanic API base URL.
pub const CRYPTOPANIC_API_URL: &str = "https://cryptopanic.com/api/v1";

/// Configuration for the news collector.
#[derive(Debug, Clone)]
pub struct NewsCollectorConfig {
    /// API authentication token
    pub api_key: String,
    /// Polling interval
    pub poll_interval: Duration,
    /// Filter for currency (default: BTC)
    pub currencies: Vec<String>,
    /// Filter type (hot, rising, bullish, bearish, etc.)
    pub filter: Option<String>,
    /// Maximum news age to include (older news is skipped)
    pub max_age: ChronoDuration,
}

impl NewsCollectorConfig {
    /// Creates a new config with API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            poll_interval: Duration::from_secs(60),
            currencies: vec!["BTC".to_string()],
            filter: Some("hot".to_string()),
            max_age: ChronoDuration::hours(24),
        }
    }

    /// Sets the polling interval.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Sets the currencies to filter.
    #[must_use]
    pub fn with_currencies(mut self, currencies: Vec<String>) -> Self {
        self.currencies = currencies;
        self
    }

    /// Sets the filter type.
    #[must_use]
    pub fn with_filter(mut self, filter: Option<String>) -> Self {
        self.filter = filter;
        self
    }

    /// Sets the maximum news age.
    #[must_use]
    pub fn with_max_age(mut self, age: ChronoDuration) -> Self {
        self.max_age = age;
        self
    }
}

/// News category with associated base urgency score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NewsCategory {
    /// Hack, exploit, vulnerability, breach
    Hack,
    /// SEC, regulation, ban, legal, court
    Regulation,
    /// Whale, large, million, billion, dormant
    Whale,
    /// ETF, fund, BlackRock, Fidelity
    Etf,
    /// Binance, Coinbase, exchange, listing
    Exchange,
    /// Halving, difficulty, hashrate
    Technical,
    /// Default/other
    Other,
}

impl NewsCategory {
    /// Returns the base urgency score for this category.
    ///
    /// Higher scores indicate more market-moving potential.
    #[must_use]
    pub fn base_urgency(&self) -> f64 {
        match self {
            NewsCategory::Hack => 1.0,
            NewsCategory::Regulation => 0.9,
            NewsCategory::Whale => 0.8,
            NewsCategory::Etf => 0.7,
            NewsCategory::Exchange => 0.6,
            NewsCategory::Technical => 0.5,
            NewsCategory::Other => 0.3,
        }
    }

    /// Returns the category name as a string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            NewsCategory::Hack => "hack",
            NewsCategory::Regulation => "regulation",
            NewsCategory::Whale => "whale",
            NewsCategory::Etf => "etf",
            NewsCategory::Exchange => "exchange",
            NewsCategory::Technical => "technical",
            NewsCategory::Other => "other",
        }
    }
}

/// Categorizes a news title into categories based on keywords.
///
/// Returns all matching categories, with the highest-priority category first.
pub fn categorize_news(title: &str) -> Vec<NewsCategory> {
    let title_lower = title.to_lowercase();
    let mut categories = Vec::new();

    // Check each category's keywords
    let hack_keywords = ["hack", "exploit", "vulnerability", "breach", "attack", "stolen"];
    let regulation_keywords = ["sec", "regulation", "ban", "legal", "court", "lawsuit", "congress", "cftc"];
    let whale_keywords = ["whale", "large", "million", "billion", "dormant", "massive", "huge"];
    let etf_keywords = ["etf", "fund", "blackrock", "fidelity", "grayscale", "ishares"];
    let exchange_keywords = ["binance", "coinbase", "kraken", "exchange", "listing", "delist"];
    let technical_keywords = ["halving", "difficulty", "hashrate", "mining", "block", "mempool"];

    if hack_keywords.iter().any(|kw| title_lower.contains(kw)) {
        categories.push(NewsCategory::Hack);
    }
    if regulation_keywords.iter().any(|kw| title_lower.contains(kw)) {
        categories.push(NewsCategory::Regulation);
    }
    if whale_keywords.iter().any(|kw| title_lower.contains(kw)) {
        categories.push(NewsCategory::Whale);
    }
    if etf_keywords.iter().any(|kw| title_lower.contains(kw)) {
        categories.push(NewsCategory::Etf);
    }
    if exchange_keywords.iter().any(|kw| title_lower.contains(kw)) {
        categories.push(NewsCategory::Exchange);
    }
    if technical_keywords.iter().any(|kw| title_lower.contains(kw)) {
        categories.push(NewsCategory::Technical);
    }

    // Sort by urgency (highest first)
    categories.sort_by(|a, b| {
        b.base_urgency()
            .partial_cmp(&a.base_urgency())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if categories.is_empty() {
        categories.push(NewsCategory::Other);
    }

    categories
}

/// Calculates the urgency score for news based on category and age.
///
/// Urgency = base_urgency * exp(-0.1 * hours_old)
///
/// This means urgency decays exponentially with time:
/// - At 0 hours: 100% of base urgency
/// - At 7 hours: ~50% of base urgency
/// - At 23 hours: ~10% of base urgency
pub fn calculate_urgency(categories: &[NewsCategory], published_at: DateTime<Utc>, now: DateTime<Utc>) -> Decimal {
    if categories.is_empty() {
        return Decimal::ZERO;
    }

    // Get highest priority category (first in sorted list)
    let base_urgency = categories[0].base_urgency();

    // Calculate time decay
    let age = now - published_at;
    let hours_old = age.num_hours().max(0) as f64 + (age.num_minutes() % 60) as f64 / 60.0;

    // Exponential decay: exp(-0.1 * hours)
    let decay = (-0.1 * hours_old).exp();

    let urgency = base_urgency * decay;

    // Convert to Decimal, clamp to [0, 1]
    Decimal::try_from(urgency.clamp(0.0, 1.0)).unwrap_or(Decimal::ZERO)
}

/// Determines sentiment from vote counts.
pub fn determine_sentiment(positive: i32, negative: i32) -> NewsSentiment {
    let total = positive + negative;
    if total == 0 {
        return NewsSentiment::Neutral;
    }

    let ratio = positive as f64 / total as f64;
    if ratio > 0.6 {
        NewsSentiment::Positive
    } else if ratio < 0.4 {
        NewsSentiment::Negative
    } else {
        NewsSentiment::Neutral
    }
}

/// CryptoPanic news collector.
pub struct NewsCollector {
    /// Configuration
    config: NewsCollectorConfig,
    /// HTTP client
    http: reqwest::Client,
    /// Output channel
    tx: mpsc::Sender<NewsEventRecord>,
    /// Optional event channel for monitoring
    event_tx: Option<mpsc::Sender<CollectorEvent>>,
    /// Statistics
    stats: CollectorStats,
    /// Seen news IDs (to avoid duplicates)
    seen_ids: HashSet<String>,
    /// Base URL (configurable for testing)
    base_url: String,
}

impl NewsCollector {
    /// Creates a new news collector.
    pub fn new(config: NewsCollectorConfig, tx: mpsc::Sender<NewsEventRecord>) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            tx,
            event_tx: None,
            stats: CollectorStats::default(),
            seen_ids: HashSet::new(),
            base_url: CRYPTOPANIC_API_URL.to_string(),
        }
    }

    /// Sets an event channel for monitoring.
    #[must_use]
    pub fn with_event_channel(mut self, tx: mpsc::Sender<CollectorEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Sets a custom base URL (useful for testing).
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Returns a reference to current statistics.
    pub fn stats(&self) -> &CollectorStats {
        &self.stats
    }

    /// Builds the API URL for fetching news.
    fn build_url(&self) -> String {
        let mut url = format!(
            "{}/posts/?auth_token={}",
            self.base_url, self.config.api_key
        );

        if !self.config.currencies.is_empty() {
            url.push_str(&format!("&currencies={}", self.config.currencies.join(",")));
        }

        if let Some(ref filter) = self.config.filter {
            url.push_str(&format!("&filter={}", filter));
        }

        url
    }

    /// Fetches news from the API.
    pub async fn fetch_news(&self) -> Result<Vec<CryptoPanicPost>> {
        let url = self.build_url();
        tracing::debug!("Fetching news from: {}", url);

        let response = self.http.get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("API error {}: {}", status, text));
        }

        let api_response: CryptoPanicResponse = response.json().await?;
        Ok(api_response.results.unwrap_or_default())
    }

    /// Processes a single news post into a record.
    pub fn process_post(&self, post: &CryptoPanicPost, now: DateTime<Utc>) -> Option<NewsEventRecord> {
        // Parse published time
        let published_at = DateTime::parse_from_rfc3339(&post.published_at)
            .map(|dt| dt.with_timezone(&Utc))
            .ok()?;

        // Check age
        let age = now - published_at;
        if age > self.config.max_age {
            return None;
        }

        // Categorize
        let categories = categorize_news(&post.title);
        let category_strs: Vec<String> = categories.iter().map(|c| c.as_str().to_string()).collect();

        // Calculate urgency
        let urgency = calculate_urgency(&categories, published_at, now);

        // Determine sentiment from votes
        let sentiment = post.votes.as_ref().map(|v| {
            determine_sentiment(v.positive, v.negative)
        }).unwrap_or(NewsSentiment::Neutral);

        // Extract currencies
        let currencies: Vec<String> = post.currencies
            .as_ref()
            .map(|c| c.iter().map(|curr| curr.code.clone()).collect())
            .unwrap_or_default();

        // Build record
        Some(
            NewsEventRecord::new(published_at, "cryptopanic".to_string(), post.title.clone())
                .with_url(post.url.clone())
                .with_categories(category_strs)
                .with_currencies(currencies)
                .with_sentiment(sentiment, urgency)
                .with_raw_data(serde_json::json!({
                    "id": post.id,
                    "kind": post.kind,
                    "source": post.source,
                    "votes": post.votes,
                }))
        )
    }

    /// Polls for news and emits new records.
    pub async fn poll(&mut self) -> Result<usize> {
        let posts = self.fetch_news().await?;
        let now = Utc::now();
        let mut new_count = 0;

        for post in posts {
            // Skip if already seen
            if self.seen_ids.contains(&post.id) {
                continue;
            }

            // Process post
            if let Some(record) = self.process_post(&post, now) {
                // Send record
                if self.tx.send(record).await.is_err() {
                    tracing::warn!("News record channel closed");
                    break;
                }

                self.seen_ids.insert(post.id.clone());
                self.stats.record_collected();
                new_count += 1;
            }
        }

        Ok(new_count)
    }

    /// Runs the collector indefinitely.
    pub async fn run(&mut self) -> Result<()> {
        self.emit_event(CollectorEvent::Connected {
            source: "news:cryptopanic".to_string(),
        })
        .await;

        let mut poll_interval = interval(self.config.poll_interval);
        let mut last_heartbeat = Instant::now();

        loop {
            poll_interval.tick().await;

            match self.poll().await {
                Ok(count) => {
                    tracing::debug!("Fetched {} new news items", count);
                }
                Err(e) => {
                    tracing::error!("News poll failed: {}", e);
                    self.stats.error_occurred();
                    self.emit_event(CollectorEvent::Error {
                        source: "news:cryptopanic".to_string(),
                        error: e.to_string(),
                    })
                    .await;
                }
            }

            // Heartbeat every 5 minutes
            if last_heartbeat.elapsed() > Duration::from_secs(300) {
                self.emit_event(CollectorEvent::Heartbeat {
                    source: "news:cryptopanic".to_string(),
                    timestamp: Utc::now(),
                    records_collected: self.stats.records_collected,
                })
                .await;
                last_heartbeat = Instant::now();
            }

            // Check if channel is still open
            if self.tx.is_closed() {
                tracing::info!("Output channel closed, stopping news collector");
                break;
            }
        }

        self.emit_event(CollectorEvent::Disconnected {
            source: "news:cryptopanic".to_string(),
            reason: "Channel closed".to_string(),
        })
        .await;

        Ok(())
    }

    /// Emits an event if the event channel is configured.
    async fn emit_event(&self, event: CollectorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event).await;
        }
    }

    /// Clears the seen IDs cache.
    pub fn clear_seen_cache(&mut self) {
        self.seen_ids.clear();
    }
}

// ========== CryptoPanic API Response Types ==========

/// CryptoPanic API response.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct CryptoPanicResponse {
    /// Total count of results
    pub count: Option<i32>,
    /// Next page URL (for pagination)
    pub next: Option<String>,
    /// Previous page URL (for pagination)
    pub previous: Option<String>,
    /// Results array
    pub results: Option<Vec<CryptoPanicPost>>,
}

/// A single news post from CryptoPanic.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptoPanicPost {
    /// Unique post ID
    pub id: String,
    /// Post kind (news, media, etc.)
    pub kind: String,
    /// Post title/headline
    pub title: String,
    /// URL to the post
    pub url: String,
    /// Published timestamp (RFC3339)
    pub published_at: String,
    /// Source information
    pub source: Option<CryptoPanicSource>,
    /// Vote counts
    pub votes: Option<CryptoPanicVotes>,
    /// Currencies mentioned
    pub currencies: Option<Vec<CryptoPanicCurrency>>,
}

/// Source information.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct CryptoPanicSource {
    pub title: Option<String>,
    pub region: Option<String>,
    pub domain: Option<String>,
}

/// Vote counts.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct CryptoPanicVotes {
    pub positive: i32,
    pub negative: i32,
    pub important: i32,
    pub liked: i32,
    pub disliked: i32,
    pub lol: i32,
    pub toxic: i32,
    pub saved: i32,
    pub comments: i32,
}

/// Currency information.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptoPanicCurrency {
    pub code: String,
    pub title: String,
    pub slug: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    // ========== NewsCollectorConfig Tests ==========

    #[test]
    fn test_config_new() {
        let config = NewsCollectorConfig::new("test-api-key");

        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.poll_interval, Duration::from_secs(60));
        assert_eq!(config.currencies, vec!["BTC".to_string()]);
        assert_eq!(config.filter, Some("hot".to_string()));
    }

    #[test]
    fn test_config_with_poll_interval() {
        let config = NewsCollectorConfig::new("key")
            .with_poll_interval(Duration::from_secs(30));

        assert_eq!(config.poll_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_config_with_currencies() {
        let config = NewsCollectorConfig::new("key")
            .with_currencies(vec!["BTC".to_string(), "ETH".to_string()]);

        assert_eq!(config.currencies.len(), 2);
    }

    #[test]
    fn test_config_with_filter() {
        let config = NewsCollectorConfig::new("key")
            .with_filter(Some("rising".to_string()));

        assert_eq!(config.filter, Some("rising".to_string()));
    }

    #[test]
    fn test_config_with_max_age() {
        let config = NewsCollectorConfig::new("key")
            .with_max_age(ChronoDuration::hours(12));

        assert_eq!(config.max_age, ChronoDuration::hours(12));
    }

    // ========== NewsCategory Tests ==========

    #[test]
    fn test_category_base_urgency_ordering() {
        // Hack should be highest priority
        assert!(NewsCategory::Hack.base_urgency() > NewsCategory::Regulation.base_urgency());
        assert!(NewsCategory::Regulation.base_urgency() > NewsCategory::Whale.base_urgency());
        assert!(NewsCategory::Whale.base_urgency() > NewsCategory::Etf.base_urgency());
        assert!(NewsCategory::Etf.base_urgency() > NewsCategory::Exchange.base_urgency());
        assert!(NewsCategory::Exchange.base_urgency() > NewsCategory::Technical.base_urgency());
        assert!(NewsCategory::Technical.base_urgency() > NewsCategory::Other.base_urgency());
    }

    #[test]
    fn test_category_as_str() {
        assert_eq!(NewsCategory::Hack.as_str(), "hack");
        assert_eq!(NewsCategory::Regulation.as_str(), "regulation");
        assert_eq!(NewsCategory::Whale.as_str(), "whale");
        assert_eq!(NewsCategory::Etf.as_str(), "etf");
        assert_eq!(NewsCategory::Exchange.as_str(), "exchange");
        assert_eq!(NewsCategory::Technical.as_str(), "technical");
        assert_eq!(NewsCategory::Other.as_str(), "other");
    }

    // ========== categorize_news Tests ==========

    #[test]
    fn test_categorize_hack() {
        let categories = categorize_news("Major exchange hacked, millions stolen");
        assert_eq!(categories[0], NewsCategory::Hack);
    }

    #[test]
    fn test_categorize_exploit() {
        let categories = categorize_news("DeFi exploit drains $50M");
        assert_eq!(categories[0], NewsCategory::Hack);
    }

    #[test]
    fn test_categorize_regulation() {
        let categories = categorize_news("SEC approves new crypto regulations");
        assert_eq!(categories[0], NewsCategory::Regulation);
    }

    #[test]
    fn test_categorize_legal() {
        let categories = categorize_news("Court rules in favor of crypto lawsuit");
        assert_eq!(categories[0], NewsCategory::Regulation);
    }

    #[test]
    fn test_categorize_whale() {
        let categories = categorize_news("Whale moves $500 million in Bitcoin");
        assert_eq!(categories[0], NewsCategory::Whale);
    }

    #[test]
    fn test_categorize_dormant() {
        let categories = categorize_news("Dormant wallet awakens after 10 years");
        assert_eq!(categories[0], NewsCategory::Whale);
    }

    #[test]
    fn test_categorize_etf() {
        let categories = categorize_news("BlackRock Bitcoin ETF sees record inflows");
        assert_eq!(categories[0], NewsCategory::Etf);
    }

    #[test]
    fn test_categorize_fidelity() {
        let categories = categorize_news("Fidelity launches new crypto fund");
        assert_eq!(categories[0], NewsCategory::Etf);
    }

    #[test]
    fn test_categorize_exchange() {
        let categories = categorize_news("Coinbase announces new listing");
        assert_eq!(categories[0], NewsCategory::Exchange);
    }

    #[test]
    fn test_categorize_technical() {
        let categories = categorize_news("Bitcoin halving countdown begins");
        assert_eq!(categories[0], NewsCategory::Technical);
    }

    #[test]
    fn test_categorize_hashrate() {
        let categories = categorize_news("Hashrate hits all-time high");
        assert_eq!(categories[0], NewsCategory::Technical);
    }

    #[test]
    fn test_categorize_other() {
        let categories = categorize_news("Bitcoin price analysis for today");
        assert_eq!(categories[0], NewsCategory::Other);
    }

    #[test]
    fn test_categorize_multiple_categories() {
        let categories = categorize_news("SEC investigates Binance hack");

        // Should have both Hack and Regulation
        assert!(categories.contains(&NewsCategory::Hack));
        assert!(categories.contains(&NewsCategory::Regulation));
        assert!(categories.contains(&NewsCategory::Exchange));

        // Hack should be first (highest priority)
        assert_eq!(categories[0], NewsCategory::Hack);
    }

    #[test]
    fn test_categorize_case_insensitive() {
        let categories = categorize_news("MAJOR HACK DISCOVERED");
        assert_eq!(categories[0], NewsCategory::Hack);
    }

    // ========== calculate_urgency Tests ==========

    #[test]
    fn test_urgency_fresh_news() {
        let now = sample_timestamp();
        let published_at = now; // Just published

        let categories = vec![NewsCategory::Hack];
        let urgency = calculate_urgency(&categories, published_at, now);

        // Should be close to 1.0 for fresh hack news
        assert!(urgency >= dec!(0.99));
    }

    #[test]
    fn test_urgency_decays_with_time() {
        let now = sample_timestamp();
        let published_7h_ago = now - ChronoDuration::hours(7);

        let categories = vec![NewsCategory::Hack];
        let urgency = calculate_urgency(&categories, published_7h_ago, now);

        // At 7 hours, decay = exp(-0.7) ~= 0.496
        // So urgency ~= 1.0 * 0.496 = 0.496
        assert!(urgency > dec!(0.45) && urgency < dec!(0.55));
    }

    #[test]
    fn test_urgency_very_old_news() {
        let now = sample_timestamp();
        let published_24h_ago = now - ChronoDuration::hours(24);

        let categories = vec![NewsCategory::Hack];
        let urgency = calculate_urgency(&categories, published_24h_ago, now);

        // At 24 hours, decay = exp(-2.4) ~= 0.09
        assert!(urgency < dec!(0.15));
    }

    #[test]
    fn test_urgency_lower_category() {
        let now = sample_timestamp();
        let published_at = now;

        let categories = vec![NewsCategory::Technical];
        let urgency = calculate_urgency(&categories, published_at, now);

        // Technical has base urgency of 0.5
        assert!(urgency >= dec!(0.49) && urgency <= dec!(0.51));
    }

    #[test]
    fn test_urgency_empty_categories() {
        let now = sample_timestamp();
        let urgency = calculate_urgency(&[], now, now);
        assert_eq!(urgency, Decimal::ZERO);
    }

    #[test]
    fn test_urgency_uses_highest_priority() {
        let now = sample_timestamp();
        let published_at = now;

        // Multiple categories - should use hack (highest)
        let categories = vec![NewsCategory::Hack, NewsCategory::Technical];
        let urgency = calculate_urgency(&categories, published_at, now);

        assert!(urgency >= dec!(0.99)); // Hack base is 1.0
    }

    // ========== determine_sentiment Tests ==========

    #[test]
    fn test_sentiment_positive() {
        let sentiment = determine_sentiment(10, 2);
        assert_eq!(sentiment, NewsSentiment::Positive);
    }

    #[test]
    fn test_sentiment_negative() {
        let sentiment = determine_sentiment(2, 10);
        assert_eq!(sentiment, NewsSentiment::Negative);
    }

    #[test]
    fn test_sentiment_neutral_balanced() {
        let sentiment = determine_sentiment(5, 5);
        assert_eq!(sentiment, NewsSentiment::Neutral);
    }

    #[test]
    fn test_sentiment_neutral_no_votes() {
        let sentiment = determine_sentiment(0, 0);
        assert_eq!(sentiment, NewsSentiment::Neutral);
    }

    #[test]
    fn test_sentiment_edge_cases() {
        // 60% positive - should be neutral (threshold is >60%)
        let sentiment_60 = determine_sentiment(6, 4);
        assert_eq!(sentiment_60, NewsSentiment::Neutral);

        // 61% positive - should be positive
        let sentiment_61 = determine_sentiment(61, 39);
        assert_eq!(sentiment_61, NewsSentiment::Positive);

        // 39% positive - should be negative
        let sentiment_39 = determine_sentiment(39, 61);
        assert_eq!(sentiment_39, NewsSentiment::Negative);
    }

    // ========== NewsCollector Tests ==========

    #[tokio::test]
    async fn test_collector_creation() {
        let config = NewsCollectorConfig::new("test-key");
        let (tx, _rx) = mpsc::channel(100);

        let collector = NewsCollector::new(config, tx);

        assert_eq!(collector.stats().records_collected, 0);
    }

    #[tokio::test]
    async fn test_collector_build_url() {
        let config = NewsCollectorConfig::new("mykey")
            .with_currencies(vec!["BTC".to_string()])
            .with_filter(Some("hot".to_string()));
        let (tx, _rx) = mpsc::channel(100);

        let collector = NewsCollector::new(config, tx);
        let url = collector.build_url();

        assert!(url.contains("auth_token=mykey"));
        assert!(url.contains("currencies=BTC"));
        assert!(url.contains("filter=hot"));
    }

    #[tokio::test]
    async fn test_collector_build_url_multiple_currencies() {
        let config = NewsCollectorConfig::new("key")
            .with_currencies(vec!["BTC".to_string(), "ETH".to_string()]);
        let (tx, _rx) = mpsc::channel(100);

        let collector = NewsCollector::new(config, tx);
        let url = collector.build_url();

        assert!(url.contains("currencies=BTC,ETH"));
    }

    #[test]
    fn test_process_post_valid() {
        let config = NewsCollectorConfig::new("key");
        let (tx, _rx) = mpsc::channel::<NewsEventRecord>(100);
        let collector = NewsCollector::new(config, tx);

        let post = CryptoPanicPost {
            id: "123".to_string(),
            kind: "news".to_string(),
            title: "Major Bitcoin hack discovered".to_string(),
            url: "https://example.com/news".to_string(),
            published_at: "2025-01-29T12:00:00Z".to_string(),
            source: None,
            votes: Some(CryptoPanicVotes {
                positive: 10,
                negative: 2,
                important: 5,
                liked: 8,
                disliked: 1,
                lol: 0,
                toxic: 0,
                saved: 3,
                comments: 5,
            }),
            currencies: Some(vec![CryptoPanicCurrency {
                code: "BTC".to_string(),
                title: "Bitcoin".to_string(),
                slug: "bitcoin".to_string(),
            }]),
        };

        let now = Utc.with_ymd_and_hms(2025, 1, 29, 12, 30, 0).unwrap();
        let record = collector.process_post(&post, now).unwrap();

        assert_eq!(record.title, "Major Bitcoin hack discovered");
        assert_eq!(record.source, "cryptopanic");
        assert!(record.categories.as_ref().unwrap().contains(&"hack".to_string()));
        assert!(record.currencies.as_ref().unwrap().contains(&"BTC".to_string()));
        assert_eq!(record.sentiment, Some("positive".to_string()));
        assert!(record.urgency_score.is_some());
    }

    #[test]
    fn test_process_post_too_old() {
        let config = NewsCollectorConfig::new("key")
            .with_max_age(ChronoDuration::hours(1));
        let (tx, _rx) = mpsc::channel::<NewsEventRecord>(100);
        let collector = NewsCollector::new(config, tx);

        let post = CryptoPanicPost {
            id: "123".to_string(),
            kind: "news".to_string(),
            title: "Old news".to_string(),
            url: "https://example.com".to_string(),
            published_at: "2025-01-29T10:00:00Z".to_string(), // 2 hours old
            source: None,
            votes: None,
            currencies: None,
        };

        let now = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = collector.process_post(&post, now);

        assert!(record.is_none());
    }

    #[test]
    fn test_process_post_invalid_timestamp() {
        let config = NewsCollectorConfig::new("key");
        let (tx, _rx) = mpsc::channel::<NewsEventRecord>(100);
        let collector = NewsCollector::new(config, tx);

        let post = CryptoPanicPost {
            id: "123".to_string(),
            kind: "news".to_string(),
            title: "Test".to_string(),
            url: "https://example.com".to_string(),
            published_at: "invalid-date".to_string(),
            source: None,
            votes: None,
            currencies: None,
        };

        let now = Utc::now();
        let record = collector.process_post(&post, now);

        assert!(record.is_none());
    }

    #[tokio::test]
    async fn test_collector_deduplication() {
        let config = NewsCollectorConfig::new("key");
        let (tx, _rx) = mpsc::channel(100);

        let mut collector = NewsCollector::new(config, tx);

        // Add an ID to seen set
        collector.seen_ids.insert("123".to_string());

        // Check it's seen
        assert!(collector.seen_ids.contains("123"));

        // Clear cache
        collector.clear_seen_cache();
        assert!(collector.seen_ids.is_empty());
    }

    // ========== API Response Parsing Tests ==========

    #[test]
    fn test_parse_cryptopanic_response() {
        let json = r#"{
            "count": 10,
            "next": "https://api.example.com/next",
            "previous": null,
            "results": [
                {
                    "id": "12345",
                    "kind": "news",
                    "title": "Bitcoin hits new high",
                    "url": "https://example.com/news",
                    "published_at": "2025-01-29T12:00:00Z",
                    "source": {
                        "title": "CryptoNews",
                        "region": "en",
                        "domain": "cryptonews.com"
                    },
                    "votes": {
                        "positive": 10,
                        "negative": 2,
                        "important": 5,
                        "liked": 8,
                        "disliked": 1,
                        "lol": 0,
                        "toxic": 0,
                        "saved": 3,
                        "comments": 5
                    },
                    "currencies": [
                        {"code": "BTC", "title": "Bitcoin", "slug": "bitcoin"}
                    ]
                }
            ]
        }"#;

        let response: CryptoPanicResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.count, Some(10));
        assert!(response.next.is_some());
        assert!(response.previous.is_none());

        let results = response.results.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "12345");
        assert_eq!(results[0].title, "Bitcoin hits new high");
    }

    #[test]
    fn test_parse_cryptopanic_post_minimal() {
        let json = r#"{
            "id": "123",
            "kind": "news",
            "title": "Test",
            "url": "https://test.com",
            "published_at": "2025-01-29T12:00:00Z"
        }"#;

        let post: CryptoPanicPost = serde_json::from_str(json).unwrap();

        assert_eq!(post.id, "123");
        assert!(post.source.is_none());
        assert!(post.votes.is_none());
        assert!(post.currencies.is_none());
    }
}
