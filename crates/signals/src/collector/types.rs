//! Shared types for data collectors.
//!
//! Common configuration, events, and utilities used across all collectors.

use std::time::Duration;

/// Configuration for data collectors.
#[derive(Debug, Clone)]
pub struct CollectorConfig {
    /// Trading pair symbol (e.g., "btcusdt")
    pub symbol: String,
    /// Delay before reconnection attempts
    pub reconnect_delay: Duration,
    /// Maximum number of reconnection attempts before giving up (0 = unlimited)
    pub max_reconnect_attempts: u32,
    /// Exchange name
    pub exchange: String,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            symbol: "btcusdt".to_string(),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0, // Unlimited
            exchange: "binance".to_string(),
        }
    }
}

impl CollectorConfig {
    /// Creates a new collector config for a specific symbol.
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into().to_lowercase(),
            ..Default::default()
        }
    }

    /// Sets the reconnect delay.
    #[must_use]
    pub fn with_reconnect_delay(mut self, delay: Duration) -> Self {
        self.reconnect_delay = delay;
        self
    }

    /// Sets the maximum reconnect attempts.
    #[must_use]
    pub fn with_max_reconnect_attempts(mut self, max: u32) -> Self {
        self.max_reconnect_attempts = max;
        self
    }

    /// Sets the exchange name.
    #[must_use]
    pub fn with_exchange(mut self, exchange: impl Into<String>) -> Self {
        self.exchange = exchange.into();
        self
    }
}

/// Events emitted by collectors for monitoring.
#[derive(Debug, Clone)]
pub enum CollectorEvent {
    /// Successfully connected to data source
    Connected { source: String },
    /// Disconnected from data source
    Disconnected { source: String, reason: String },
    /// Error occurred during collection
    Error { source: String, error: String },
    /// Heartbeat for health monitoring
    Heartbeat {
        source: String,
        timestamp: chrono::DateTime<chrono::Utc>,
        records_collected: u64,
    },
    /// Reconnection attempt
    Reconnecting { source: String, attempt: u32 },
}

/// Statistics for a running collector.
#[derive(Debug, Clone, Default)]
pub struct CollectorStats {
    /// Total records collected since start
    pub records_collected: u64,
    /// Total errors encountered
    pub errors_encountered: u64,
    /// Number of reconnections
    pub reconnections: u32,
    /// Time of last successful record
    pub last_record_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl CollectorStats {
    /// Increments the record count.
    pub fn record_collected(&mut self) {
        self.records_collected += 1;
        self.last_record_time = Some(chrono::Utc::now());
    }

    /// Increments the error count.
    pub fn error_occurred(&mut self) {
        self.errors_encountered += 1;
    }

    /// Increments the reconnection count.
    pub fn reconnected(&mut self) {
        self.reconnections += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collector_config_default() {
        let config = CollectorConfig::default();

        assert_eq!(config.symbol, "btcusdt");
        assert_eq!(config.reconnect_delay, Duration::from_secs(5));
        assert_eq!(config.max_reconnect_attempts, 0);
        assert_eq!(config.exchange, "binance");
    }

    #[test]
    fn test_collector_config_new() {
        let config = CollectorConfig::new("ETHUSDT");

        // Symbol should be lowercased
        assert_eq!(config.symbol, "ethusdt");
    }

    #[test]
    fn test_collector_config_builder() {
        let config = CollectorConfig::new("solusdt")
            .with_reconnect_delay(Duration::from_secs(10))
            .with_max_reconnect_attempts(5)
            .with_exchange("hyperliquid");

        assert_eq!(config.symbol, "solusdt");
        assert_eq!(config.reconnect_delay, Duration::from_secs(10));
        assert_eq!(config.max_reconnect_attempts, 5);
        assert_eq!(config.exchange, "hyperliquid");
    }

    #[test]
    fn test_collector_stats_default() {
        let stats = CollectorStats::default();

        assert_eq!(stats.records_collected, 0);
        assert_eq!(stats.errors_encountered, 0);
        assert_eq!(stats.reconnections, 0);
        assert!(stats.last_record_time.is_none());
    }

    #[test]
    fn test_collector_stats_record_collected() {
        let mut stats = CollectorStats::default();

        stats.record_collected();
        assert_eq!(stats.records_collected, 1);
        assert!(stats.last_record_time.is_some());

        stats.record_collected();
        assert_eq!(stats.records_collected, 2);
    }

    #[test]
    fn test_collector_stats_error_occurred() {
        let mut stats = CollectorStats::default();

        stats.error_occurred();
        stats.error_occurred();
        assert_eq!(stats.errors_encountered, 2);
    }

    #[test]
    fn test_collector_stats_reconnected() {
        let mut stats = CollectorStats::default();

        stats.reconnected();
        stats.reconnected();
        stats.reconnected();
        assert_eq!(stats.reconnections, 3);
    }

    #[test]
    fn test_collector_event_variants() {
        // Test that all variants can be constructed
        let _connected = CollectorEvent::Connected {
            source: "test".to_string(),
        };

        let _disconnected = CollectorEvent::Disconnected {
            source: "test".to_string(),
            reason: "timeout".to_string(),
        };

        let _error = CollectorEvent::Error {
            source: "test".to_string(),
            error: "parse error".to_string(),
        };

        let _heartbeat = CollectorEvent::Heartbeat {
            source: "test".to_string(),
            timestamp: chrono::Utc::now(),
            records_collected: 100,
        };

        let _reconnecting = CollectorEvent::Reconnecting {
            source: "test".to_string(),
            attempt: 3,
        };
    }
}
