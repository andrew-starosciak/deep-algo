//! News sentiment signal generator.
//!
//! Generates trading signals based on news events, sentiment, and urgency.
//! Uses category-weighted impact with time decay for recency bias.
//!
//! ## Key Concepts
//!
//! - **Category Weights**: Different news categories have different market impact
//! - **Time Decay**: Older news has less impact than recent news
//! - **Sentiment**: Bullish/bearish news affects direction
//! - **Urgency**: Higher urgency news has stronger signals

use algo_trade_core::{Direction, NewsEvent, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use std::collections::HashMap;

// ============================================
// Category Weights
// ============================================

/// Returns the default category weights for news impact.
///
/// Higher weights indicate more market-moving potential.
/// Categories are from the news collector:
/// - hack: 1.0 (highest impact, typically negative)
/// - regulation: 0.9
/// - whale: 0.8
/// - etf: 0.7
/// - exchange: 0.6
/// - technical: 0.5
/// - other: 0.3
#[must_use]
pub fn default_category_weights() -> HashMap<String, f64> {
    HashMap::from([
        ("hack".to_string(), 1.0),
        ("regulation".to_string(), 0.9),
        ("whale".to_string(), 0.8),
        ("etf".to_string(), 0.7),
        ("exchange".to_string(), 0.6),
        ("technical".to_string(), 0.5),
        ("other".to_string(), 0.3),
    ])
}

// ============================================
// Sentiment Parsing
// ============================================

/// Parses sentiment string into a numeric value.
///
/// Returns:
/// - 1.0 for positive/bullish
/// - -1.0 for negative/bearish
/// - 0.0 for neutral/unknown
///
/// # Arguments
/// * `sentiment` - Sentiment string (e.g., "positive", "negative", "neutral")
#[must_use]
pub fn parse_sentiment(sentiment: &str) -> f64 {
    match sentiment.to_lowercase().as_str() {
        "positive" | "bullish" => 1.0,
        "negative" | "bearish" => -1.0,
        "neutral" | "" => 0.0,
        _ => 0.0,
    }
}

// ============================================
// Time Decay
// ============================================

/// Calculates time decay factor for news impact.
///
/// Uses exponential decay: exp(-lambda * hours)
/// Where lambda is calibrated so that:
/// - At 0 minutes: 1.0
/// - At 15 minutes: ~0.5
/// - At 30 minutes: ~0.25
///
/// # Arguments
/// * `age_minutes` - Age of news in minutes
#[must_use]
pub fn calculate_time_decay(age_minutes: f64) -> f64 {
    // lambda = ln(2) / 15 = ~0.046 for half-life at 15 minutes
    let lambda = 0.693 / 15.0; // ln(2) / 15
    (-lambda * age_minutes).exp().max(0.0)
}

// ============================================
// News Impact Calculation
// ============================================

/// Calculates the aggregate news impact from a list of events.
///
/// Impact is calculated as:
/// `sum(category_weight * urgency * sentiment * time_decay)`
///
/// The result is in the range [-inf, +inf] where:
/// - Positive = bullish sentiment dominates
/// - Negative = bearish sentiment dominates
/// - Zero = neutral or no significant news
///
/// # Arguments
/// * `events` - List of news events to analyze
/// * `category_weights` - Weight for each news category
/// * `lookback` - Maximum age of news to consider
/// * `current_time` - Current timestamp for decay calculation
/// * `min_urgency` - Minimum urgency score to include (0.0-1.0)
///
/// # Example
/// ```
/// use algo_trade_core::NewsEvent;
/// use algo_trade_signals::generator::news_signal::{calculate_news_impact, default_category_weights};
/// use chrono::{Duration, Utc};
/// use rust_decimal_macros::dec;
///
/// let events = vec![
///     NewsEvent {
///         timestamp: Utc::now() - Duration::minutes(5),
///         source: "test".to_string(),
///         title: "Bitcoin ETF approved".to_string(),
///         sentiment: Some("positive".to_string()),
///         urgency_score: Some(0.9),
///         currencies: Some(vec!["BTC".to_string()]),
///     },
/// ];
///
/// let impact = calculate_news_impact(
///     &events,
///     &default_category_weights(),
///     Duration::minutes(30),
///     Utc::now(),
///     dec!(0.5),
/// );
///
/// assert!(impact > 0.0); // Positive sentiment = positive impact
/// ```
pub fn calculate_news_impact(
    events: &[NewsEvent],
    category_weights: &HashMap<String, f64>,
    lookback: Duration,
    current_time: DateTime<Utc>,
    min_urgency: Decimal,
) -> f64 {
    let min_urgency_f64: f64 = min_urgency.to_string().parse().unwrap_or(0.0);

    let lookback_start = current_time - lookback;

    let mut total_impact = 0.0;

    for event in events {
        // Filter by time
        if event.timestamp < lookback_start {
            continue;
        }

        // Filter by urgency
        let urgency = event.urgency_score.unwrap_or(0.0);
        if urgency < min_urgency_f64 {
            continue;
        }

        // Get sentiment
        let sentiment = event
            .sentiment
            .as_ref()
            .map(|s| parse_sentiment(s))
            .unwrap_or(0.0);

        // Skip neutral events
        if sentiment.abs() < 0.001 {
            continue;
        }

        // Get category weight (from title analysis or default)
        let category_weight = get_category_weight(&event.title, category_weights);

        // Calculate time decay
        let age = current_time - event.timestamp;
        let age_minutes = age.num_minutes().max(0) as f64 + (age.num_seconds() % 60) as f64 / 60.0;
        let time_decay = calculate_time_decay(age_minutes);

        // Calculate impact: category * urgency * sentiment * decay
        let impact = category_weight * urgency * sentiment * time_decay;
        total_impact += impact;
    }

    total_impact
}

/// Gets the category weight for a news title.
///
/// Analyzes the title for category keywords and returns the highest weight.
fn get_category_weight(title: &str, weights: &HashMap<String, f64>) -> f64 {
    let title_lower = title.to_lowercase();

    // Define keyword patterns for each category
    let category_keywords: &[(&str, &[&str])] = &[
        (
            "hack",
            &[
                "hack",
                "exploit",
                "vulnerability",
                "breach",
                "attack",
                "stolen",
            ],
        ),
        (
            "regulation",
            &[
                "sec",
                "regulation",
                "ban",
                "legal",
                "court",
                "lawsuit",
                "congress",
                "cftc",
            ],
        ),
        (
            "whale",
            &[
                "whale", "large", "million", "billion", "dormant", "massive", "huge",
            ],
        ),
        (
            "etf",
            &[
                "etf",
                "fund",
                "blackrock",
                "fidelity",
                "grayscale",
                "ishares",
            ],
        ),
        (
            "exchange",
            &[
                "binance", "coinbase", "kraken", "exchange", "listing", "delist",
            ],
        ),
        (
            "technical",
            &[
                "halving",
                "difficulty",
                "hashrate",
                "mining",
                "block",
                "mempool",
            ],
        ),
    ];

    let mut max_weight = weights.get("other").copied().unwrap_or(0.3);

    for (category, keywords) in category_keywords {
        if keywords.iter().any(|kw| title_lower.contains(kw)) {
            let weight = weights.get(*category).copied().unwrap_or(0.3);
            if weight > max_weight {
                max_weight = weight;
            }
        }
    }

    max_weight
}

// ============================================
// News Signal Generator
// ============================================

/// Signal generator based on news sentiment analysis.
///
/// Analyzes recent news events and generates trading signals based on:
/// - Aggregate sentiment (bullish vs bearish)
/// - News category importance (hacks > regulation > etf > etc.)
/// - Time decay (recent news weighted more heavily)
/// - Urgency scores (higher urgency = stronger signal)
#[derive(Debug, Clone)]
pub struct NewsSignal {
    /// Name of this signal
    name: String,
    /// Weight for composite signal aggregation
    weight: f64,
    /// Minimum urgency score to consider (0.0 to 1.0)
    pub min_urgency: Decimal,
    /// Lookback duration for considering news
    pub lookback_duration: Duration,
    /// Category weights (uses defaults if None)
    pub category_weights: Option<HashMap<String, f64>>,
    /// Threshold for directional signal (impact must exceed this)
    pub impact_threshold: f64,
}

impl Default for NewsSignal {
    fn default() -> Self {
        Self::new("news_sentiment")
    }
}

impl NewsSignal {
    /// Creates a new NewsSignal with default configuration.
    ///
    /// # Arguments
    /// * `name` - Name for this signal instance
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            weight: 1.0,
            min_urgency: Decimal::new(5, 1), // 0.5 default
            lookback_duration: Duration::minutes(30),
            category_weights: None,
            impact_threshold: 0.1,
        }
    }

    /// Sets the minimum urgency threshold.
    #[must_use]
    pub fn with_min_urgency(mut self, urgency: Decimal) -> Self {
        self.min_urgency = urgency.max(Decimal::ZERO).min(Decimal::ONE);
        self
    }

    /// Sets the lookback duration for news events.
    #[must_use]
    pub fn with_lookback(mut self, duration: Duration) -> Self {
        self.lookback_duration = duration;
        self
    }

    /// Sets custom category weights.
    #[must_use]
    pub fn with_category_weights(mut self, weights: HashMap<String, f64>) -> Self {
        self.category_weights = Some(weights);
        self
    }

    /// Sets the weight for composite signal aggregation.
    #[must_use]
    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight;
        self
    }

    /// Sets the impact threshold for generating directional signals.
    #[must_use]
    pub fn with_impact_threshold(mut self, threshold: f64) -> Self {
        self.impact_threshold = threshold.abs();
        self
    }

    /// Gets the effective category weights (custom or default).
    fn effective_weights(&self) -> HashMap<String, f64> {
        self.category_weights
            .clone()
            .unwrap_or_else(default_category_weights)
    }
}

#[async_trait]
impl SignalGenerator for NewsSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Get news events from context
        let events = match &ctx.news_events {
            Some(e) if !e.is_empty() => e,
            _ => {
                tracing::debug!("No news events in context, returning neutral signal");
                return Ok(SignalValue::neutral()
                    .with_metadata("event_count", 0.0)
                    .with_metadata("impact", 0.0));
            }
        };

        // Calculate aggregate impact
        let weights = self.effective_weights();
        let impact = calculate_news_impact(
            events,
            &weights,
            self.lookback_duration,
            ctx.timestamp,
            self.min_urgency,
        );

        // Count events within lookback and above urgency threshold
        let min_urgency_f64: f64 = self.min_urgency.to_string().parse().unwrap_or(0.0);
        let lookback_start = ctx.timestamp - self.lookback_duration;

        let relevant_events: Vec<_> = events
            .iter()
            .filter(|e| {
                e.timestamp >= lookback_start && e.urgency_score.unwrap_or(0.0) >= min_urgency_f64
            })
            .collect();

        let event_count = relevant_events.len();

        // Determine direction based on impact
        let (direction, strength) = if impact > self.impact_threshold {
            // Bullish signal
            let strength = (impact / 2.0).clamp(0.0, 1.0); // Scale impact to [0, 1]
            (Direction::Up, strength)
        } else if impact < -self.impact_threshold {
            // Bearish signal
            let strength = (-impact / 2.0).clamp(0.0, 1.0);
            (Direction::Down, strength)
        } else {
            // Neutral - impact below threshold
            (Direction::Neutral, 0.0)
        };

        // Build signal with metadata
        let signal = SignalValue::new(direction, strength, 0.0)?
            .with_metadata("event_count", event_count as f64)
            .with_metadata("impact", impact)
            .with_metadata("impact_threshold", self.impact_threshold)
            .with_metadata(
                "min_urgency",
                self.min_urgency.to_string().parse().unwrap_or(0.0),
            )
            .with_metadata(
                "lookback_minutes",
                self.lookback_duration.num_minutes() as f64,
            );

        Ok(signal)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn weight(&self) -> f64 {
        self.weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // ============================================
    // Helper Functions
    // ============================================

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    fn make_news_event(
        title: &str,
        sentiment: &str,
        urgency: f64,
        age_minutes: i64,
        base_time: DateTime<Utc>,
    ) -> NewsEvent {
        NewsEvent {
            timestamp: base_time - Duration::minutes(age_minutes),
            source: "test".to_string(),
            title: title.to_string(),
            sentiment: Some(sentiment.to_string()),
            urgency_score: Some(urgency),
            currencies: Some(vec!["BTC".to_string()]),
        }
    }

    // ============================================
    // Sentiment Parsing Tests
    // ============================================

    #[test]
    fn parse_sentiment_positive() {
        assert!((parse_sentiment("positive") - 1.0).abs() < 0.001);
        assert!((parse_sentiment("bullish") - 1.0).abs() < 0.001);
        assert!((parse_sentiment("POSITIVE") - 1.0).abs() < 0.001);
        assert!((parse_sentiment("Bullish") - 1.0).abs() < 0.001);
    }

    #[test]
    fn parse_sentiment_negative() {
        assert!((parse_sentiment("negative") - (-1.0)).abs() < 0.001);
        assert!((parse_sentiment("bearish") - (-1.0)).abs() < 0.001);
        assert!((parse_sentiment("NEGATIVE") - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn parse_sentiment_neutral() {
        assert!((parse_sentiment("neutral") - 0.0).abs() < 0.001);
        assert!((parse_sentiment("") - 0.0).abs() < 0.001);
        assert!((parse_sentiment("unknown") - 0.0).abs() < 0.001);
    }

    // ============================================
    // Time Decay Tests
    // ============================================

    #[test]
    fn time_decay_fresh_news() {
        let decay = calculate_time_decay(0.0);
        assert!(
            (decay - 1.0).abs() < 0.01,
            "Fresh news should have decay ~1.0"
        );
    }

    #[test]
    fn time_decay_at_15_minutes() {
        let decay = calculate_time_decay(15.0);
        // Should be approximately 0.5 (half-life at 15 minutes)
        assert!(
            decay > 0.45 && decay < 0.55,
            "Expected decay ~0.5 at 15 mins, got {}",
            decay
        );
    }

    #[test]
    fn time_decay_at_30_minutes() {
        let decay = calculate_time_decay(30.0);
        // Should be approximately 0.25 (two half-lives)
        assert!(
            decay > 0.20 && decay < 0.30,
            "Expected decay ~0.25 at 30 mins, got {}",
            decay
        );
    }

    #[test]
    fn time_decay_very_old() {
        let decay = calculate_time_decay(120.0); // 2 hours old
        assert!(decay < 0.01, "Very old news should have near-zero decay");
    }

    // ============================================
    // Category Weight Tests
    // ============================================

    #[test]
    fn default_category_weights_contains_all_categories() {
        let weights = default_category_weights();

        assert!(weights.contains_key("hack"));
        assert!(weights.contains_key("regulation"));
        assert!(weights.contains_key("whale"));
        assert!(weights.contains_key("etf"));
        assert!(weights.contains_key("exchange"));
        assert!(weights.contains_key("technical"));
        assert!(weights.contains_key("other"));
    }

    #[test]
    fn default_category_weights_ordering() {
        let weights = default_category_weights();

        assert!(weights["hack"] > weights["regulation"]);
        assert!(weights["regulation"] > weights["whale"]);
        assert!(weights["whale"] > weights["etf"]);
        assert!(weights["etf"] > weights["exchange"]);
        assert!(weights["exchange"] > weights["technical"]);
        assert!(weights["technical"] > weights["other"]);
    }

    #[test]
    fn get_category_weight_detects_hack() {
        let weights = default_category_weights();
        let weight = get_category_weight("Major exchange hacked, millions stolen", &weights);

        assert!(
            (weight - 1.0).abs() < 0.001,
            "Hack news should have weight 1.0"
        );
    }

    #[test]
    fn get_category_weight_detects_etf() {
        let weights = default_category_weights();
        let weight = get_category_weight("BlackRock Bitcoin ETF sees record inflows", &weights);

        assert!(
            (weight - 0.7).abs() < 0.001,
            "ETF news should have weight 0.7"
        );
    }

    #[test]
    fn get_category_weight_other() {
        let weights = default_category_weights();
        let weight = get_category_weight("Bitcoin price analysis for today", &weights);

        assert!(
            (weight - 0.3).abs() < 0.001,
            "Other news should have weight 0.3"
        );
    }

    // ============================================
    // News Impact Tests
    // ============================================

    #[test]
    fn news_impact_positive_for_bullish_news() {
        let now = sample_timestamp();
        let events = vec![make_news_event(
            "ETF approved by SEC",
            "positive",
            0.9,
            5,
            now,
        )];

        let impact = calculate_news_impact(
            &events,
            &default_category_weights(),
            Duration::minutes(30),
            now,
            dec!(0.5),
        );

        assert!(
            impact > 0.0,
            "Expected positive impact for bullish news, got {}",
            impact
        );
    }

    #[test]
    fn news_impact_negative_for_bearish_news() {
        let now = sample_timestamp();
        let events = vec![make_news_event(
            "Major exchange hacked",
            "negative",
            0.9,
            5,
            now,
        )];

        let impact = calculate_news_impact(
            &events,
            &default_category_weights(),
            Duration::minutes(30),
            now,
            dec!(0.5),
        );

        assert!(
            impact < 0.0,
            "Expected negative impact for bearish news, got {}",
            impact
        );
    }

    #[test]
    fn news_impact_higher_for_urgent_news() {
        let now = sample_timestamp();

        let low_urgency = vec![make_news_event("ETF update", "positive", 0.5, 5, now)];
        let high_urgency = vec![make_news_event("ETF update", "positive", 0.9, 5, now)];

        let low_impact = calculate_news_impact(
            &low_urgency,
            &default_category_weights(),
            Duration::minutes(30),
            now,
            dec!(0.0),
        );
        let high_impact = calculate_news_impact(
            &high_urgency,
            &default_category_weights(),
            Duration::minutes(30),
            now,
            dec!(0.0),
        );

        assert!(
            high_impact > low_impact,
            "High urgency should have more impact"
        );
    }

    #[test]
    fn news_impact_decays_with_time() {
        let now = sample_timestamp();

        let fresh_news = vec![make_news_event("ETF approved", "positive", 0.9, 1, now)];
        let old_news = vec![make_news_event("ETF approved", "positive", 0.9, 25, now)];

        let fresh_impact = calculate_news_impact(
            &fresh_news,
            &default_category_weights(),
            Duration::minutes(30),
            now,
            dec!(0.5),
        );
        let old_impact = calculate_news_impact(
            &old_news,
            &default_category_weights(),
            Duration::minutes(30),
            now,
            dec!(0.5),
        );

        assert!(
            fresh_impact > old_impact,
            "Fresh news should have more impact"
        );
    }

    #[test]
    fn news_impact_zero_with_no_events() {
        let impact = calculate_news_impact(
            &[],
            &default_category_weights(),
            Duration::minutes(30),
            sample_timestamp(),
            dec!(0.5),
        );

        assert!(impact.abs() < 0.001, "Expected zero impact with no events");
    }

    #[test]
    fn news_impact_filters_by_urgency() {
        let now = sample_timestamp();
        let events = vec![
            make_news_event("Low urgency news", "positive", 0.3, 5, now), // Below threshold
        ];

        let impact = calculate_news_impact(
            &events,
            &default_category_weights(),
            Duration::minutes(30),
            now,
            dec!(0.5), // Threshold is 0.5, event has 0.3
        );

        assert!(
            impact.abs() < 0.001,
            "Low urgency news should be filtered out"
        );
    }

    #[test]
    fn news_impact_filters_by_lookback() {
        let now = sample_timestamp();
        let events = vec![
            make_news_event("Old news", "positive", 0.9, 60, now), // 60 mins old
        ];

        let impact = calculate_news_impact(
            &events,
            &default_category_weights(),
            Duration::minutes(30), // Only look back 30 mins
            now,
            dec!(0.5),
        );

        assert!(
            impact.abs() < 0.001,
            "News outside lookback should be filtered"
        );
    }

    // ============================================
    // NewsSignal Generator Tests
    // ============================================

    #[tokio::test]
    async fn news_signal_returns_neutral_with_no_events() {
        let mut signal = NewsSignal::new("test");
        let ctx = SignalContext::new(sample_timestamp(), "BTCUSD");

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
        assert!((result.strength - 0.0).abs() < 0.001);
        assert_eq!(*result.metadata.get("event_count").unwrap(), 0.0);
    }

    #[tokio::test]
    async fn news_signal_bullish_on_positive_high_urgency() {
        let mut signal = NewsSignal::new("test")
            .with_min_urgency(dec!(0.5))
            .with_impact_threshold(0.01);

        let now = sample_timestamp();
        let events = vec![
            make_news_event("ETF approved by SEC", "positive", 0.9, 5, now),
            make_news_event("BlackRock buys more BTC", "positive", 0.8, 10, now),
        ];

        let ctx = SignalContext::new(now, "BTCUSD").with_news_events(events);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Up, "Expected bullish signal");
        assert!(result.strength > 0.0, "Expected positive strength");
    }

    #[tokio::test]
    async fn news_signal_bearish_on_negative_high_urgency() {
        let mut signal = NewsSignal::new("test")
            .with_min_urgency(dec!(0.5))
            .with_impact_threshold(0.01);

        let now = sample_timestamp();
        let events = vec![make_news_event(
            "Major exchange hacked",
            "negative",
            0.95,
            3,
            now,
        )];

        let ctx = SignalContext::new(now, "BTCUSD").with_news_events(events);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Down, "Expected bearish signal");
        assert!(result.strength > 0.0, "Expected positive strength");
    }

    #[tokio::test]
    async fn news_signal_ignores_old_events() {
        let mut signal = NewsSignal::new("test")
            .with_lookback(Duration::minutes(30))
            .with_min_urgency(dec!(0.5));

        let now = sample_timestamp();
        let events = vec![
            make_news_event("Old positive news", "positive", 0.9, 60, now), // 60 mins old
        ];

        let ctx = SignalContext::new(now, "BTCUSD").with_news_events(events);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(
            result.direction,
            Direction::Neutral,
            "Old news should be ignored"
        );
    }

    #[tokio::test]
    async fn news_signal_ignores_low_urgency() {
        let mut signal = NewsSignal::new("test").with_min_urgency(dec!(0.7)); // High threshold

        let now = sample_timestamp();
        let events = vec![
            make_news_event("Low urgency positive", "positive", 0.5, 5, now), // Below threshold
        ];

        let ctx = SignalContext::new(now, "BTCUSD").with_news_events(events);

        let result = signal.compute(&ctx).await.unwrap();

        // Event count should be 0 since it's below urgency threshold
        assert_eq!(*result.metadata.get("event_count").unwrap(), 0.0);
    }

    #[tokio::test]
    async fn news_signal_metadata_includes_event_count() {
        let mut signal = NewsSignal::new("test").with_min_urgency(dec!(0.5));

        let now = sample_timestamp();
        let events = vec![
            make_news_event("News 1", "positive", 0.9, 5, now),
            make_news_event("News 2", "negative", 0.8, 10, now),
            make_news_event("News 3", "neutral", 0.7, 15, now),
        ];

        let ctx = SignalContext::new(now, "BTCUSD").with_news_events(events);

        let result = signal.compute(&ctx).await.unwrap();

        // Should have 3 events above urgency threshold
        assert_eq!(*result.metadata.get("event_count").unwrap(), 3.0);
        assert!(result.metadata.contains_key("impact"));
        assert!(result.metadata.contains_key("lookback_minutes"));
    }

    #[tokio::test]
    async fn news_signal_respects_impact_threshold() {
        let now = sample_timestamp();

        // Signal with high threshold
        let mut high_threshold = NewsSignal::new("test").with_impact_threshold(10.0); // Very high threshold

        let events = vec![make_news_event("Small news", "positive", 0.6, 5, now)];

        let ctx = SignalContext::new(now, "BTCUSD").with_news_events(events);

        let result = high_threshold.compute(&ctx).await.unwrap();

        assert_eq!(
            result.direction,
            Direction::Neutral,
            "Impact below threshold should be neutral"
        );
    }

    // ============================================
    // Builder Pattern Tests
    // ============================================

    #[test]
    fn builder_with_min_urgency() {
        let signal = NewsSignal::new("test").with_min_urgency(dec!(0.7));

        assert_eq!(signal.min_urgency, dec!(0.7));
    }

    #[test]
    fn builder_with_min_urgency_clamps() {
        let too_high = NewsSignal::new("test").with_min_urgency(dec!(1.5));
        assert_eq!(too_high.min_urgency, dec!(1));

        let too_low = NewsSignal::new("test").with_min_urgency(dec!(-0.5));
        assert_eq!(too_low.min_urgency, dec!(0));
    }

    #[test]
    fn builder_with_lookback() {
        let signal = NewsSignal::new("test").with_lookback(Duration::hours(1));

        assert_eq!(signal.lookback_duration, Duration::hours(1));
    }

    #[test]
    fn builder_with_category_weights() {
        let custom_weights = HashMap::from([("hack".to_string(), 2.0), ("other".to_string(), 0.1)]);

        let signal = NewsSignal::new("test").with_category_weights(custom_weights.clone());

        assert!(signal.category_weights.is_some());
        assert_eq!(signal.effective_weights().get("hack").copied(), Some(2.0));
    }

    #[test]
    fn builder_with_weight() {
        let signal = NewsSignal::new("test").with_weight(2.5);

        assert!((signal.weight() - 2.5).abs() < 0.001);
    }

    #[test]
    fn builder_with_impact_threshold() {
        let signal = NewsSignal::new("test").with_impact_threshold(0.5);

        assert!((signal.impact_threshold - 0.5).abs() < 0.001);
    }

    #[test]
    fn signal_name_is_correct() {
        let signal = NewsSignal::new("my_news_signal");
        assert_eq!(signal.name(), "my_news_signal");
    }

    #[test]
    fn default_signal_has_sensible_values() {
        let signal = NewsSignal::default();

        assert_eq!(signal.name(), "news_sentiment");
        assert_eq!(signal.min_urgency, dec!(0.5));
        assert_eq!(signal.lookback_duration, Duration::minutes(30));
        assert!(signal.category_weights.is_none());
        assert!((signal.impact_threshold - 0.1).abs() < 0.001);
    }
}
