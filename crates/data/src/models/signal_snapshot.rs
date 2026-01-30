//! Signal snapshot data model.
//!
//! Captures signal generator outputs for statistical validation,
//! debugging, and forward return analysis.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// A snapshot of a signal generator's output.
///
/// Used for logging signal outputs and correlating with forward returns
/// to validate signal predictive power.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SignalSnapshotRecord {
    /// Auto-generated ID (optional for new records)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[sqlx(default)]
    pub id: Option<i64>,
    /// Timestamp when the signal was computed
    pub timestamp: DateTime<Utc>,
    /// Name of the signal generator
    pub signal_name: String,
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange name (e.g., "binance")
    pub exchange: String,
    /// Signal direction: "up", "down", "neutral"
    pub direction: String,
    /// Signal strength from 0.0 to 1.0
    pub strength: Decimal,
    /// Statistical confidence from 0.0 to 1.0
    pub confidence: Decimal,
    /// Optional metadata for debugging and analysis
    #[serde(default)]
    pub metadata: JsonValue,
    /// 15-minute forward return (filled after settlement)
    pub forward_return_15m: Option<Decimal>,
    /// Created timestamp (database default)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[sqlx(default)]
    pub created_at: Option<DateTime<Utc>>,
}

impl SignalSnapshotRecord {
    /// Creates a new signal snapshot record.
    ///
    /// # Arguments
    /// * `timestamp` - When the signal was computed
    /// * `signal_name` - Name of the signal generator
    /// * `symbol` - Trading pair symbol
    /// * `exchange` - Exchange name
    /// * `direction` - Signal direction (up/down/neutral)
    /// * `strength` - Signal strength [0, 1]
    /// * `confidence` - Statistical confidence [0, 1]
    pub fn new(
        timestamp: DateTime<Utc>,
        signal_name: impl Into<String>,
        symbol: impl Into<String>,
        exchange: impl Into<String>,
        direction: SignalDirection,
        strength: Decimal,
        confidence: Decimal,
    ) -> Self {
        Self {
            id: None,
            timestamp,
            signal_name: signal_name.into(),
            symbol: symbol.into(),
            exchange: exchange.into(),
            direction: direction.as_str().to_string(),
            strength,
            confidence,
            metadata: JsonValue::Object(serde_json::Map::new()),
            forward_return_15m: None,
            created_at: None,
        }
    }

    /// Builder method to add metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: JsonValue) -> Self {
        self.metadata = metadata;
        self
    }

    /// Builder method to add a single metadata key-value pair.
    #[must_use]
    pub fn with_metadata_entry(
        mut self,
        key: impl Into<String>,
        value: impl Into<JsonValue>,
    ) -> Self {
        if let JsonValue::Object(ref mut map) = self.metadata {
            map.insert(key.into(), value.into());
        }
        self
    }

    /// Sets the forward return (used during validation).
    pub fn set_forward_return(&mut self, return_pct: Decimal) {
        self.forward_return_15m = Some(return_pct);
    }

    /// Returns true if this signal has been validated (has forward return).
    #[must_use]
    pub fn is_validated(&self) -> bool {
        self.forward_return_15m.is_some()
    }

    /// Returns the parsed direction.
    #[must_use]
    pub fn parsed_direction(&self) -> Option<SignalDirection> {
        SignalDirection::parse(&self.direction)
    }

    /// Returns true if the signal correctly predicted the forward return direction.
    ///
    /// Returns None if not validated or direction is neutral.
    #[must_use]
    pub fn is_correct_prediction(&self) -> Option<bool> {
        let forward_return = self.forward_return_15m?;
        let direction = self.parsed_direction()?;

        match direction {
            SignalDirection::Up => Some(forward_return > Decimal::ZERO),
            SignalDirection::Down => Some(forward_return < Decimal::ZERO),
            SignalDirection::Neutral => None,
        }
    }
}

/// Signal direction enum for type safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDirection {
    /// Bullish signal - expect price to go up
    Up,
    /// Bearish signal - expect price to go down
    Down,
    /// No directional bias
    Neutral,
}

impl SignalDirection {
    /// Returns the string representation.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            SignalDirection::Up => "up",
            SignalDirection::Down => "down",
            SignalDirection::Neutral => "neutral",
        }
    }

    /// Parses from string (non-failing version).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "neutral" => Some(Self::Neutral),
            _ => None,
        }
    }
}

impl std::str::FromStr for SignalDirection {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| anyhow::anyhow!("Invalid signal direction: {}", s))
    }
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

    // ============================================
    // SignalSnapshotRecord Tests
    // ============================================

    #[test]
    fn signal_snapshot_record_new_creates_valid_record() {
        let record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "order_book_imbalance",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.75),
            dec!(0.85),
        );

        assert_eq!(record.signal_name, "order_book_imbalance");
        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange, "binance");
        assert_eq!(record.direction, "up");
        assert_eq!(record.strength, dec!(0.75));
        assert_eq!(record.confidence, dec!(0.85));
        assert!(record.id.is_none());
        assert!(record.forward_return_15m.is_none());
    }

    #[test]
    fn signal_snapshot_record_with_metadata_works() {
        let record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test_signal",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.5),
            dec!(0.5),
        )
        .with_metadata(json!({
            "imbalance": 0.15,
            "volume_ratio": 1.5
        }));

        assert!(record.metadata.get("imbalance").is_some());
        assert!(record.metadata.get("volume_ratio").is_some());
    }

    #[test]
    fn signal_snapshot_record_with_metadata_entry_works() {
        let record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test_signal",
            "BTCUSDT",
            "binance",
            SignalDirection::Down,
            dec!(0.6),
            dec!(0.7),
        )
        .with_metadata_entry("zscore", json!(-2.1))
        .with_metadata_entry("percentile", json!(0.05));

        let zscore = record.metadata.get("zscore").unwrap().as_f64().unwrap();
        assert!((zscore - (-2.1)).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_snapshot_record_serializes_correctly() {
        let record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test_signal",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.75),
            dec!(0.85),
        )
        .with_metadata(json!({"test": true}));

        let json_str = serde_json::to_string(&record).expect("serialization failed");

        assert!(json_str.contains("\"signal_name\":\"test_signal\""));
        assert!(json_str.contains("\"direction\":\"up\""));
        assert!(json_str.contains("\"strength\":\"0.75\""));
        // id should be skipped since it's None
        assert!(!json_str.contains("\"id\":null"));
    }

    #[test]
    fn signal_snapshot_record_deserializes_correctly() {
        let json_str = r#"{
            "timestamp": "2025-01-29T12:00:00Z",
            "signal_name": "test_signal",
            "symbol": "BTCUSDT",
            "exchange": "binance",
            "direction": "down",
            "strength": "0.65",
            "confidence": "0.80",
            "metadata": {"imbalance": -0.2},
            "forward_return_15m": "0.0015"
        }"#;

        let record: SignalSnapshotRecord =
            serde_json::from_str(json_str).expect("deserialization failed");

        assert_eq!(record.signal_name, "test_signal");
        assert_eq!(record.direction, "down");
        assert_eq!(record.strength, dec!(0.65));
        assert_eq!(record.forward_return_15m, Some(dec!(0.0015)));
    }

    #[test]
    fn signal_snapshot_record_is_validated_works() {
        let mut record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.5),
            dec!(0.5),
        );

        assert!(!record.is_validated());

        record.set_forward_return(dec!(0.001));
        assert!(record.is_validated());
    }

    #[test]
    fn signal_snapshot_record_parsed_direction_works() {
        let up_record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.5),
            dec!(0.5),
        );
        assert_eq!(up_record.parsed_direction(), Some(SignalDirection::Up));

        let down_record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test",
            "BTCUSDT",
            "binance",
            SignalDirection::Down,
            dec!(0.5),
            dec!(0.5),
        );
        assert_eq!(down_record.parsed_direction(), Some(SignalDirection::Down));
    }

    #[test]
    fn signal_snapshot_record_is_correct_prediction_up() {
        let mut record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.8),
            dec!(0.9),
        );

        // Not validated yet
        assert!(record.is_correct_prediction().is_none());

        // Correct prediction (up signal, positive return)
        record.set_forward_return(dec!(0.01));
        assert_eq!(record.is_correct_prediction(), Some(true));

        // Incorrect prediction (up signal, negative return)
        record.set_forward_return(dec!(-0.01));
        assert_eq!(record.is_correct_prediction(), Some(false));
    }

    #[test]
    fn signal_snapshot_record_is_correct_prediction_down() {
        let mut record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test",
            "BTCUSDT",
            "binance",
            SignalDirection::Down,
            dec!(0.8),
            dec!(0.9),
        );

        // Correct prediction (down signal, negative return)
        record.set_forward_return(dec!(-0.01));
        assert_eq!(record.is_correct_prediction(), Some(true));

        // Incorrect prediction (down signal, positive return)
        record.set_forward_return(dec!(0.01));
        assert_eq!(record.is_correct_prediction(), Some(false));
    }

    #[test]
    fn signal_snapshot_record_is_correct_prediction_neutral() {
        let mut record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "test",
            "BTCUSDT",
            "binance",
            SignalDirection::Neutral,
            dec!(0.0),
            dec!(0.5),
        );

        record.set_forward_return(dec!(0.01));
        // Neutral signals don't have correct/incorrect predictions
        assert!(record.is_correct_prediction().is_none());
    }

    // ============================================
    // SignalDirection Tests
    // ============================================

    #[test]
    fn signal_direction_as_str_correct() {
        assert_eq!(SignalDirection::Up.as_str(), "up");
        assert_eq!(SignalDirection::Down.as_str(), "down");
        assert_eq!(SignalDirection::Neutral.as_str(), "neutral");
    }

    #[test]
    fn signal_direction_parse_correct() {
        assert_eq!(SignalDirection::parse("up"), Some(SignalDirection::Up));
        assert_eq!(SignalDirection::parse("UP"), Some(SignalDirection::Up));
        assert_eq!(SignalDirection::parse("Up"), Some(SignalDirection::Up));
        assert_eq!(SignalDirection::parse("down"), Some(SignalDirection::Down));
        assert_eq!(
            SignalDirection::parse("neutral"),
            Some(SignalDirection::Neutral)
        );
        assert_eq!(SignalDirection::parse("invalid"), None);
    }

    #[test]
    fn signal_direction_from_str_trait() {
        use std::str::FromStr;
        assert!(SignalDirection::from_str("up").is_ok());
        assert!(SignalDirection::from_str("invalid").is_err());
    }

    #[test]
    fn signal_direction_equality() {
        assert_eq!(SignalDirection::Up, SignalDirection::Up);
        assert_ne!(SignalDirection::Up, SignalDirection::Down);
    }
}
