//! Cached microstructure signals for synchronous access from strategies.
//!
//! This module provides thread-safe storage for microstructure signals that are
//! updated asynchronously by the `MicrostructureOrchestrator` and read synchronously
//! by `EnhancedStrategy` implementations.

use algo_trade_core::signal::{Direction, SignalValue};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;

use algo_trade_core::events::SignalDirection;

/// Thread-safe handle to cached microstructure signals.
///
/// Use this type to share signal state between the orchestrator (writer)
/// and strategies (readers).
pub type SharedMicroSignals = Arc<RwLock<CachedMicroSignals>>;

/// Cached microstructure signals for sync access from strategies.
///
/// Updated asynchronously by background collector task.
/// All signal values are normalized to `SignalValue` with direction and strength.
#[derive(Debug, Clone)]
pub struct CachedMicroSignals {
    /// Order book imbalance signal (positive = bid heavy, negative = ask heavy)
    pub order_book_imbalance: SignalValue,
    /// Funding rate signal (high positive = potential short squeeze)
    pub funding_rate: SignalValue,
    /// Liquidation cascade signal (high strength = active cascade)
    pub liquidation_cascade: SignalValue,
    /// News/sentiment signal
    pub news: SignalValue,
    /// Composite signal combining all microstructure inputs
    pub composite: SignalValue,
    /// Timestamp of last signal update
    pub last_updated: DateTime<Utc>,
}

impl Default for CachedMicroSignals {
    fn default() -> Self {
        Self {
            order_book_imbalance: SignalValue::neutral(),
            funding_rate: SignalValue::neutral(),
            liquidation_cascade: SignalValue::neutral(),
            news: SignalValue::neutral(),
            composite: SignalValue::neutral(),
            last_updated: Utc::now(),
        }
    }
}

impl CachedMicroSignals {
    /// Creates a new `CachedMicroSignals` with default neutral values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if any signal indicates high market stress.
    ///
    /// High stress is defined as:
    /// - Liquidation cascade strength > 0.7, OR
    /// - Funding rate strength > 0.8
    #[must_use]
    pub fn is_high_stress(&self) -> bool {
        let liquidation_stress = self.liquidation_cascade.strength > 0.7;
        let funding_extreme = self.funding_rate.strength > 0.8;
        liquidation_stress || funding_extreme
    }

    /// Returns the dominant direction across all signals using weighted voting.
    ///
    /// Signals vote for their direction weighted by strength. The winning
    /// direction must have accumulated weight > 0.5 to be considered dominant.
    #[must_use]
    pub fn consensus_direction(&self) -> Direction {
        let mut up_weight = 0.0;
        let mut down_weight = 0.0;

        for signal in [
            &self.order_book_imbalance,
            &self.funding_rate,
            &self.liquidation_cascade,
        ] {
            match signal.direction {
                Direction::Up => up_weight += signal.strength,
                Direction::Down => down_weight += signal.strength,
                Direction::Neutral => {}
            }
        }

        if up_weight > down_weight && up_weight > 0.5 {
            Direction::Up
        } else if down_weight > up_weight && down_weight > 0.5 {
            Direction::Down
        } else {
            Direction::Neutral
        }
    }

    /// Check if microstructure supports a given strategy direction.
    ///
    /// Returns true if:
    /// - Strategy direction is Exit (exits always allowed)
    /// - Microstructure consensus is Neutral (does not block)
    /// - Microstructure direction matches strategy direction (Long/Up or Short/Down)
    ///
    /// Returns false if microstructure direction conflicts with strategy direction.
    #[must_use]
    pub fn supports_direction(&self, direction: &SignalDirection) -> bool {
        let micro_dir = self.consensus_direction();
        matches!(
            (direction, micro_dir),
            (SignalDirection::Long, Direction::Up)
                | (SignalDirection::Short, Direction::Down)
                | (_, Direction::Neutral)
                | (SignalDirection::Exit, _)
        )
    }

    /// Returns the age of the cached signals in seconds.
    #[must_use]
    pub fn age_seconds(&self) -> i64 {
        (Utc::now() - self.last_updated).num_seconds()
    }

    /// Returns true if signals are considered stale (older than threshold).
    #[must_use]
    pub fn is_stale(&self, max_age_seconds: i64) -> bool {
        self.age_seconds() > max_age_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================
    // Default/Constructor Tests
    // ============================================

    #[test]
    fn default_creates_neutral_signals() {
        let signals = CachedMicroSignals::default();

        assert_eq!(signals.order_book_imbalance.direction, Direction::Neutral);
        assert!((signals.order_book_imbalance.strength - 0.0).abs() < f64::EPSILON);
        assert_eq!(signals.funding_rate.direction, Direction::Neutral);
        assert_eq!(signals.liquidation_cascade.direction, Direction::Neutral);
        assert_eq!(signals.news.direction, Direction::Neutral);
        assert_eq!(signals.composite.direction, Direction::Neutral);
    }

    #[test]
    fn new_is_same_as_default() {
        let new_signals = CachedMicroSignals::new();
        let default_signals = CachedMicroSignals::default();

        assert_eq!(
            new_signals.order_book_imbalance.direction,
            default_signals.order_book_imbalance.direction
        );
        assert_eq!(
            new_signals.funding_rate.direction,
            default_signals.funding_rate.direction
        );
    }

    // ============================================
    // is_high_stress Tests
    // ============================================

    #[test]
    fn is_high_stress_false_when_all_neutral() {
        let signals = CachedMicroSignals::default();
        assert!(!signals.is_high_stress());
    }

    #[test]
    fn is_high_stress_true_when_liquidation_above_threshold() {
        let mut signals = CachedMicroSignals::default();
        signals.liquidation_cascade = SignalValue::new(Direction::Down, 0.75, 0.8).unwrap();

        assert!(signals.is_high_stress());
    }

    #[test]
    fn is_high_stress_true_when_funding_above_threshold() {
        let mut signals = CachedMicroSignals::default();
        signals.funding_rate = SignalValue::new(Direction::Up, 0.85, 0.9).unwrap();

        assert!(signals.is_high_stress());
    }

    #[test]
    fn is_high_stress_false_when_just_below_thresholds() {
        let mut signals = CachedMicroSignals::default();
        signals.liquidation_cascade = SignalValue::new(Direction::Down, 0.69, 0.8).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Up, 0.79, 0.9).unwrap();

        assert!(!signals.is_high_stress());
    }

    #[test]
    fn is_high_stress_true_when_at_liquidation_threshold() {
        let mut signals = CachedMicroSignals::default();
        signals.liquidation_cascade = SignalValue::new(Direction::Down, 0.71, 0.8).unwrap();

        assert!(signals.is_high_stress());
    }

    // ============================================
    // consensus_direction Tests
    // ============================================

    #[test]
    fn consensus_direction_neutral_when_all_neutral() {
        let signals = CachedMicroSignals::default();
        assert_eq!(signals.consensus_direction(), Direction::Neutral);
    }

    #[test]
    fn consensus_direction_up_when_majority_bullish() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Up, 0.6, 0.7).unwrap();
        signals.liquidation_cascade = SignalValue::new(Direction::Down, 0.3, 0.5).unwrap();

        // up_weight = 0.8 + 0.6 = 1.4, down_weight = 0.3
        assert_eq!(signals.consensus_direction(), Direction::Up);
    }

    #[test]
    fn consensus_direction_down_when_majority_bearish() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Down, 0.7, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Down, 0.6, 0.7).unwrap();
        signals.liquidation_cascade = SignalValue::new(Direction::Up, 0.2, 0.5).unwrap();

        // down_weight = 0.7 + 0.6 = 1.3, up_weight = 0.2
        assert_eq!(signals.consensus_direction(), Direction::Down);
    }

    #[test]
    fn consensus_direction_neutral_when_weights_below_threshold() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Up, 0.3, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Down, 0.2, 0.7).unwrap();
        signals.liquidation_cascade = SignalValue::neutral();

        // up_weight = 0.3, down_weight = 0.2 - both below 0.5
        assert_eq!(signals.consensus_direction(), Direction::Neutral);
    }

    #[test]
    fn consensus_direction_neutral_when_tied() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Up, 0.6, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Down, 0.6, 0.7).unwrap();
        signals.liquidation_cascade = SignalValue::neutral();

        // Tied at 0.6 each - should return neutral (neither wins)
        assert_eq!(signals.consensus_direction(), Direction::Neutral);
    }

    #[test]
    fn consensus_direction_ignores_neutral_signals() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Neutral, 0.9, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Up, 0.6, 0.7).unwrap();
        signals.liquidation_cascade = SignalValue::neutral();

        // Neutral signals don't contribute to voting
        assert_eq!(signals.consensus_direction(), Direction::Up);
    }

    // ============================================
    // supports_direction Tests
    // ============================================

    #[test]
    fn supports_direction_exit_always_allowed() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Down, 0.9, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Down, 0.9, 0.9).unwrap();

        // Even with strong bearish consensus, exit is allowed
        assert!(signals.supports_direction(&SignalDirection::Exit));
    }

    #[test]
    fn supports_direction_long_when_consensus_up() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Up, 0.6, 0.7).unwrap();

        assert!(signals.supports_direction(&SignalDirection::Long));
    }

    #[test]
    fn supports_direction_short_when_consensus_down() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Down, 0.8, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Down, 0.6, 0.7).unwrap();

        assert!(signals.supports_direction(&SignalDirection::Short));
    }

    #[test]
    fn supports_direction_long_blocked_when_consensus_down() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Down, 0.8, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Down, 0.6, 0.7).unwrap();

        // Consensus is Down, Long should not be supported
        assert!(!signals.supports_direction(&SignalDirection::Long));
    }

    #[test]
    fn supports_direction_short_blocked_when_consensus_up() {
        let mut signals = CachedMicroSignals::default();
        signals.order_book_imbalance = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();
        signals.funding_rate = SignalValue::new(Direction::Up, 0.6, 0.7).unwrap();

        // Consensus is Up, Short should not be supported
        assert!(!signals.supports_direction(&SignalDirection::Short));
    }

    #[test]
    fn supports_direction_any_when_consensus_neutral() {
        let signals = CachedMicroSignals::default();

        // Neutral consensus allows any direction
        assert!(signals.supports_direction(&SignalDirection::Long));
        assert!(signals.supports_direction(&SignalDirection::Short));
        assert!(signals.supports_direction(&SignalDirection::Exit));
    }

    // ============================================
    // age_seconds and is_stale Tests
    // ============================================

    #[test]
    fn age_seconds_is_small_for_fresh_signals() {
        let signals = CachedMicroSignals::default();
        // Just created, should be < 1 second old
        assert!(signals.age_seconds() < 2);
    }

    #[test]
    fn is_stale_false_for_fresh_signals() {
        let signals = CachedMicroSignals::default();
        assert!(!signals.is_stale(60)); // 60 second threshold
    }

    #[test]
    fn is_stale_true_for_old_signals() {
        let mut signals = CachedMicroSignals::default();
        signals.last_updated = Utc::now() - chrono::Duration::seconds(120);

        assert!(signals.is_stale(60)); // 60 second threshold
    }

    // ============================================
    // Clone Tests
    // ============================================

    #[test]
    fn clone_produces_independent_copy() {
        let mut original = CachedMicroSignals::default();
        original.order_book_imbalance = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();

        let cloned = original.clone();

        // Modify original
        original.order_book_imbalance = SignalValue::new(Direction::Down, 0.5, 0.5).unwrap();

        // Clone should be unchanged
        assert_eq!(cloned.order_book_imbalance.direction, Direction::Up);
    }

    // ============================================
    // SharedMicroSignals (Arc<RwLock>) Tests
    // ============================================

    #[tokio::test]
    async fn shared_signals_can_be_read_concurrently() {
        let signals: SharedMicroSignals = Arc::new(RwLock::new(CachedMicroSignals::default()));

        let s1 = signals.clone();
        let s2 = signals.clone();

        // Both reads should succeed concurrently
        let handle1 = tokio::spawn(async move {
            let guard = s1.read().await;
            guard.consensus_direction()
        });

        let handle2 = tokio::spawn(async move {
            let guard = s2.read().await;
            guard.is_high_stress()
        });

        let (dir, stress) = tokio::join!(handle1, handle2);
        assert_eq!(dir.unwrap(), Direction::Neutral);
        assert!(!stress.unwrap());
    }

    #[tokio::test]
    async fn shared_signals_can_be_written() {
        let signals: SharedMicroSignals = Arc::new(RwLock::new(CachedMicroSignals::default()));

        // Write new values
        {
            let mut guard = signals.write().await;
            guard.order_book_imbalance = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();
        }

        // Read and verify
        {
            let guard = signals.read().await;
            assert_eq!(guard.order_book_imbalance.direction, Direction::Up);
        }
    }
}
