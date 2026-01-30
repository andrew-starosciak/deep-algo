//! News event data model.
//!
//! Captures news with urgency scoring for sentiment-based signals.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// A news event with sentiment and urgency analysis.
///
/// Used for news-based trading signals. Urgency score indicates
/// the potential market impact timing.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NewsEventRecord {
    /// Timestamp when news was published/received
    pub timestamp: DateTime<Utc>,
    /// News source (e.g., "cryptopanic", "twitter")
    pub source: String,
    /// News headline/title
    pub title: String,
    /// URL to the full article
    pub url: Option<String>,
    /// Categories/tags for the news
    pub categories: Option<Vec<String>>,
    /// Cryptocurrency tickers mentioned (e.g., ["BTC", "ETH"])
    pub currencies: Option<Vec<String>>,
    /// Sentiment classification: "positive", "negative", "neutral"
    pub sentiment: Option<String>,
    /// Urgency score from 0.0 to 1.0 (higher = more urgent)
    pub urgency_score: Option<Decimal>,
    /// Raw data from the news source
    pub raw_data: Option<JsonValue>,
}

impl NewsEventRecord {
    /// Creates a new news event record.
    pub fn new(timestamp: DateTime<Utc>, source: String, title: String) -> Self {
        Self {
            timestamp,
            source,
            title,
            url: None,
            categories: None,
            currencies: None,
            sentiment: None,
            urgency_score: None,
            raw_data: None,
        }
    }

    /// Builder method to add URL.
    pub fn with_url(mut self, url: String) -> Self {
        self.url = Some(url);
        self
    }

    /// Builder method to add categories.
    pub fn with_categories(mut self, categories: Vec<String>) -> Self {
        self.categories = Some(categories);
        self
    }

    /// Builder method to add currencies.
    pub fn with_currencies(mut self, currencies: Vec<String>) -> Self {
        self.currencies = Some(currencies);
        self
    }

    /// Builder method to add sentiment analysis.
    pub fn with_sentiment(mut self, sentiment: NewsSentiment, urgency: Decimal) -> Self {
        self.sentiment = Some(sentiment.as_str().to_string());
        self.urgency_score = Some(urgency);
        self
    }

    /// Builder method to add raw data.
    pub fn with_raw_data(mut self, data: JsonValue) -> Self {
        self.raw_data = Some(data);
        self
    }

    /// Returns true if this news is about a specific currency.
    #[must_use]
    pub fn mentions_currency(&self, currency: &str) -> bool {
        self.currencies
            .as_ref()
            .map(|c| c.iter().any(|curr| curr.eq_ignore_ascii_case(currency)))
            .unwrap_or(false)
    }

    /// Returns the parsed sentiment enum.
    #[must_use]
    pub fn parsed_sentiment(&self) -> Option<NewsSentiment> {
        self.sentiment.as_ref().and_then(|s| match s.as_str() {
            "positive" => Some(NewsSentiment::Positive),
            "negative" => Some(NewsSentiment::Negative),
            "neutral" => Some(NewsSentiment::Neutral),
            _ => None,
        })
    }

    /// Returns true if this is high-urgency news.
    #[must_use]
    pub fn is_high_urgency(&self, threshold: Decimal) -> bool {
        self.urgency_score.map(|s| s >= threshold).unwrap_or(false)
    }

    /// Returns true if this is positive sentiment news.
    #[must_use]
    pub fn is_positive(&self) -> bool {
        self.parsed_sentiment() == Some(NewsSentiment::Positive)
    }

    /// Returns true if this is negative sentiment news.
    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.parsed_sentiment() == Some(NewsSentiment::Negative)
    }

    /// Returns the signal direction based on sentiment.
    /// Positive news suggests bullish, negative suggests bearish.
    #[must_use]
    pub fn signal_direction(&self) -> Option<NewsSignalDirection> {
        match self.parsed_sentiment()? {
            NewsSentiment::Positive => Some(NewsSignalDirection::Bullish),
            NewsSentiment::Negative => Some(NewsSignalDirection::Bearish),
            NewsSentiment::Neutral => None,
        }
    }

    /// Calculates a weighted signal strength combining sentiment and urgency.
    /// Returns a value from -1.0 (strong bearish) to 1.0 (strong bullish).
    #[must_use]
    pub fn signal_strength(&self) -> Option<Decimal> {
        let sentiment = self.parsed_sentiment()?;
        let urgency = self.urgency_score.unwrap_or(Decimal::ZERO);

        match sentiment {
            NewsSentiment::Positive => Some(urgency),
            NewsSentiment::Negative => Some(-urgency),
            NewsSentiment::Neutral => Some(Decimal::ZERO),
        }
    }
}

/// News sentiment classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsSentiment {
    Positive,
    Negative,
    Neutral,
}

impl NewsSentiment {
    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            NewsSentiment::Positive => "positive",
            NewsSentiment::Negative => "negative",
            NewsSentiment::Neutral => "neutral",
        }
    }
}

/// Direction signal from news analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsSignalDirection {
    Bullish,
    Bearish,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use serde_json::json;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn test_news_event_creation() {
        let record = NewsEventRecord {
            timestamp: sample_timestamp(),
            source: "cryptopanic".to_string(),
            title: "Bitcoin hits new ATH".to_string(),
            url: Some("https://example.com".to_string()),
            categories: Some(vec!["price".to_string()]),
            currencies: Some(vec!["BTC".to_string()]),
            sentiment: Some("positive".to_string()),
            urgency_score: Some(dec!(0.85)),
            raw_data: Some(json!({})),
        };

        assert_eq!(record.source, "cryptopanic");
        assert_eq!(record.title, "Bitcoin hits new ATH");
    }

    #[test]
    fn test_news_event_new() {
        let record = NewsEventRecord::new(
            sample_timestamp(),
            "cryptopanic".to_string(),
            "Test headline".to_string(),
        );

        assert_eq!(record.source, "cryptopanic");
        assert_eq!(record.url, None);
        assert_eq!(record.sentiment, None);
    }

    #[test]
    fn test_builder_pattern() {
        let record = NewsEventRecord::new(
            sample_timestamp(),
            "cryptopanic".to_string(),
            "Bitcoin hits new ATH".to_string(),
        )
        .with_url("https://example.com".to_string())
        .with_categories(vec!["price".to_string()])
        .with_currencies(vec!["BTC".to_string(), "ETH".to_string()])
        .with_sentiment(NewsSentiment::Positive, dec!(0.85))
        .with_raw_data(json!({"votes": {"positive": 10}}));

        assert_eq!(record.url, Some("https://example.com".to_string()));
        assert_eq!(record.categories, Some(vec!["price".to_string()]));
        assert_eq!(
            record.currencies,
            Some(vec!["BTC".to_string(), "ETH".to_string()])
        );
        assert_eq!(record.sentiment, Some("positive".to_string()));
        assert_eq!(record.urgency_score, Some(dec!(0.85)));
        assert!(record.raw_data.is_some());
    }

    #[test]
    fn test_mentions_currency() {
        let record = NewsEventRecord::new(
            sample_timestamp(),
            "cryptopanic".to_string(),
            "Test".to_string(),
        )
        .with_currencies(vec!["BTC".to_string(), "ETH".to_string()]);

        assert!(record.mentions_currency("BTC"));
        assert!(record.mentions_currency("btc")); // case insensitive
        assert!(record.mentions_currency("ETH"));
        assert!(!record.mentions_currency("SOL"));
    }

    #[test]
    fn test_mentions_currency_none() {
        let record = NewsEventRecord::new(
            sample_timestamp(),
            "cryptopanic".to_string(),
            "Test".to_string(),
        );

        assert!(!record.mentions_currency("BTC"));
    }

    #[test]
    fn test_parsed_sentiment() {
        let positive = NewsEventRecord::new(
            sample_timestamp(),
            "cryptopanic".to_string(),
            "Test".to_string(),
        )
        .with_sentiment(NewsSentiment::Positive, dec!(0.5));

        assert_eq!(positive.parsed_sentiment(), Some(NewsSentiment::Positive));

        let negative = NewsEventRecord {
            timestamp: sample_timestamp(),
            source: "test".to_string(),
            title: "Test".to_string(),
            url: None,
            categories: None,
            currencies: None,
            sentiment: Some("negative".to_string()),
            urgency_score: None,
            raw_data: None,
        };

        assert_eq!(negative.parsed_sentiment(), Some(NewsSentiment::Negative));
    }

    #[test]
    fn test_is_high_urgency() {
        let record = NewsEventRecord::new(
            sample_timestamp(),
            "cryptopanic".to_string(),
            "Test".to_string(),
        )
        .with_sentiment(NewsSentiment::Positive, dec!(0.85));

        assert!(record.is_high_urgency(dec!(0.7)));
        assert!(!record.is_high_urgency(dec!(0.9)));
    }

    #[test]
    fn test_is_positive_negative() {
        let positive =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Positive, dec!(0.5));

        assert!(positive.is_positive());
        assert!(!positive.is_negative());

        let negative =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Negative, dec!(0.5));

        assert!(!negative.is_positive());
        assert!(negative.is_negative());
    }

    #[test]
    fn test_signal_direction() {
        let positive =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Positive, dec!(0.5));

        assert_eq!(
            positive.signal_direction(),
            Some(NewsSignalDirection::Bullish)
        );

        let negative =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Negative, dec!(0.5));

        assert_eq!(
            negative.signal_direction(),
            Some(NewsSignalDirection::Bearish)
        );

        let neutral =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Neutral, dec!(0.5));

        assert_eq!(neutral.signal_direction(), None);
    }

    #[test]
    fn test_signal_strength() {
        let positive =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Positive, dec!(0.85));

        assert_eq!(positive.signal_strength(), Some(dec!(0.85)));

        let negative =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Negative, dec!(0.75));

        assert_eq!(negative.signal_strength(), Some(dec!(-0.75)));

        let neutral =
            NewsEventRecord::new(sample_timestamp(), "test".to_string(), "Test".to_string())
                .with_sentiment(NewsSentiment::Neutral, dec!(0.5));

        assert_eq!(neutral.signal_strength(), Some(Decimal::ZERO));
    }

    #[test]
    fn test_sentiment_as_str() {
        assert_eq!(NewsSentiment::Positive.as_str(), "positive");
        assert_eq!(NewsSentiment::Negative.as_str(), "negative");
        assert_eq!(NewsSentiment::Neutral.as_str(), "neutral");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let record = NewsEventRecord {
            timestamp: sample_timestamp(),
            source: "cryptopanic".to_string(),
            title: "Bitcoin hits new ATH".to_string(),
            url: Some("https://example.com".to_string()),
            categories: Some(vec!["price".to_string()]),
            currencies: Some(vec!["BTC".to_string()]),
            sentiment: Some("positive".to_string()),
            urgency_score: Some(dec!(0.85)),
            raw_data: Some(json!({"test": true})),
        };

        let json_str = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: NewsEventRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(record.source, deserialized.source);
        assert_eq!(record.title, deserialized.title);
        assert_eq!(record.sentiment, deserialized.sentiment);
        assert_eq!(record.urgency_score, deserialized.urgency_score);
    }
}
