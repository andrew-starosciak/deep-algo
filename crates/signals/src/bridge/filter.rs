//! Microstructure filter for strategy signal enhancement.
//!
//! This module provides the decision logic for filtering, modifying, or overriding
//! strategy signals based on microstructure conditions.

use algo_trade_core::events::{SignalDirection, SignalEvent};
use algo_trade_core::signal::Direction;

use super::CachedMicroSignals;

/// Configuration for microstructure filtering behavior.
///
/// All features are independently configurable, allowing fine-grained control
/// over how microstructure signals affect strategy decisions.
#[derive(Debug, Clone)]
pub struct MicrostructureFilterConfig {
    /// Block entries when microstructure direction conflicts with strategy
    pub entry_filter_enabled: bool,
    /// Minimum composite signal strength to apply entry filter (0.0 to 1.0)
    pub entry_filter_threshold: f64,

    /// Force exit on extreme microstructure conditions
    pub exit_trigger_enabled: bool,
    /// Liquidation cascade strength threshold to trigger exit
    pub exit_liquidation_threshold: f64,
    /// Funding rate strength threshold to trigger exit
    pub exit_funding_threshold: f64,

    /// Adjust position size based on market stress
    pub sizing_adjustment_enabled: bool,
    /// Multiply signal strength by this factor under high stress (0.0 to 1.0)
    pub stress_size_multiplier: f64,

    /// Delay entry until order book supports direction
    pub entry_timing_enabled: bool,
    /// Minimum order book imbalance in favor of direction
    pub timing_support_threshold: f64,
}

impl Default for MicrostructureFilterConfig {
    fn default() -> Self {
        Self {
            entry_filter_enabled: true,
            entry_filter_threshold: 0.6,
            exit_trigger_enabled: true,
            exit_liquidation_threshold: 0.8,
            exit_funding_threshold: 0.9,
            sizing_adjustment_enabled: true,
            stress_size_multiplier: 0.5,
            entry_timing_enabled: false,
            timing_support_threshold: 0.3,
        }
    }
}

impl MicrostructureFilterConfig {
    /// Creates a config with all features disabled.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            entry_filter_enabled: false,
            exit_trigger_enabled: false,
            sizing_adjustment_enabled: false,
            entry_timing_enabled: false,
            ..Self::default()
        }
    }

    /// Creates a conservative config with tighter thresholds.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            entry_filter_enabled: true,
            entry_filter_threshold: 0.4,
            exit_trigger_enabled: true,
            exit_liquidation_threshold: 0.6,
            exit_funding_threshold: 0.7,
            sizing_adjustment_enabled: true,
            stress_size_multiplier: 0.25,
            entry_timing_enabled: true,
            timing_support_threshold: 0.4,
        }
    }
}

/// Result of applying microstructure filter to a strategy signal.
#[derive(Debug, Clone)]
pub enum FilterResult {
    /// Allow signal to pass through unchanged
    Allow(SignalEvent),
    /// Block the signal entirely
    Block {
        /// Reason for blocking
        reason: String,
    },
    /// Modify the signal (e.g., reduce strength for sizing)
    Modify(SignalEvent),
    /// Override with forced exit
    ForceExit {
        /// Reason for forced exit
        reason: String,
        /// Exit signal to execute
        signal: SignalEvent,
    },
}

impl FilterResult {
    /// Returns true if the result allows a signal to pass (Allow, Modify, or ForceExit).
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        !matches!(self, Self::Block { .. })
    }

    /// Returns the signal if one exists (Allow, Modify, or ForceExit).
    #[must_use]
    pub fn signal(&self) -> Option<&SignalEvent> {
        match self {
            Self::Allow(s) | Self::Modify(s) | Self::ForceExit { signal: s, .. } => Some(s),
            Self::Block { .. } => None,
        }
    }
}

/// Applies microstructure signals to filter/modify strategy signals.
///
/// The filter evaluates signals in this order:
/// 1. Exit triggers (checked first, can override any signal)
/// 2. Entry filter (blocks conflicting entries)
/// 3. Entry timing (waits for order book support)
/// 4. Sizing adjustment (reduces strength under stress)
pub struct MicrostructureFilter {
    config: MicrostructureFilterConfig,
}

impl MicrostructureFilter {
    /// Creates a new filter with the given configuration.
    #[must_use]
    pub fn new(config: MicrostructureFilterConfig) -> Self {
        Self { config }
    }

    /// Returns a reference to the filter configuration.
    #[must_use]
    pub fn config(&self) -> &MicrostructureFilterConfig {
        &self.config
    }

    /// Returns a mutable reference to the filter configuration.
    pub fn config_mut(&mut self) -> &mut MicrostructureFilterConfig {
        &mut self.config
    }

    /// Apply filter to a strategy signal based on current microstructure state.
    ///
    /// Evaluation order:
    /// 1. Exit triggers (highest priority)
    /// 2. Entry filter
    /// 3. Entry timing
    /// 4. Sizing adjustment
    #[must_use]
    pub fn apply(&self, signal: SignalEvent, micro: &CachedMicroSignals) -> FilterResult {
        // 1. Check for forced exit conditions first (highest priority)
        if self.config.exit_trigger_enabled {
            if let Some(exit_result) = self.check_exit_trigger(&signal, micro) {
                return exit_result;
            }
        }

        // 2. Check entry filter (only for non-exit signals)
        if self.config.entry_filter_enabled && signal.direction != SignalDirection::Exit {
            if let Some(block_result) = self.check_entry_filter(&signal, micro) {
                return block_result;
            }
        }

        // 3. Check entry timing (only for non-exit signals)
        if self.config.entry_timing_enabled
            && signal.direction != SignalDirection::Exit
            && !self.check_entry_timing(&signal, micro)
        {
            return FilterResult::Block {
                reason: "Waiting for order book support".to_string(),
            };
        }

        // 4. Apply sizing adjustment if enabled
        if self.config.sizing_adjustment_enabled {
            if let Some(modified) = self.apply_sizing_adjustment(signal.clone(), micro) {
                return FilterResult::Modify(modified);
            }
        }

        FilterResult::Allow(signal)
    }

    /// Check if exit trigger conditions are met.
    fn check_exit_trigger(
        &self,
        signal: &SignalEvent,
        micro: &CachedMicroSignals,
    ) -> Option<FilterResult> {
        // Only trigger exits for open positions (Long/Short)
        if signal.direction == SignalDirection::Exit {
            return None;
        }

        // Check liquidation cascade against position direction
        let liq = &micro.liquidation_cascade;
        if liq.strength >= self.config.exit_liquidation_threshold {
            let should_exit = matches!(
                (&signal.direction, liq.direction),
                (SignalDirection::Long, Direction::Down) | (SignalDirection::Short, Direction::Up)
            );

            if should_exit {
                return Some(FilterResult::ForceExit {
                    reason: format!(
                        "Liquidation cascade against position (strength: {:.2})",
                        liq.strength
                    ),
                    signal: SignalEvent {
                        direction: SignalDirection::Exit,
                        strength: 1.0,
                        symbol: signal.symbol.clone(),
                        price: signal.price,
                        timestamp: signal.timestamp,
                    },
                });
            }
        }

        // Check extreme funding rate
        let funding = &micro.funding_rate;
        if funding.strength >= self.config.exit_funding_threshold {
            let should_exit = matches!(
                (&signal.direction, funding.direction),
                (SignalDirection::Long, Direction::Down) | (SignalDirection::Short, Direction::Up)
            );

            if should_exit {
                return Some(FilterResult::ForceExit {
                    reason: format!(
                        "Extreme funding rate against position (strength: {:.2})",
                        funding.strength
                    ),
                    signal: SignalEvent {
                        direction: SignalDirection::Exit,
                        strength: 1.0,
                        symbol: signal.symbol.clone(),
                        price: signal.price,
                        timestamp: signal.timestamp,
                    },
                });
            }
        }

        None
    }

    /// Check if entry filter should block the signal.
    fn check_entry_filter(
        &self,
        signal: &SignalEvent,
        micro: &CachedMicroSignals,
    ) -> Option<FilterResult> {
        let composite = &micro.composite;

        // Only apply filter if composite signal is strong enough
        if composite.strength < self.config.entry_filter_threshold {
            return None;
        }

        // Check for directional conflict
        let conflicts = matches!(
            (&signal.direction, composite.direction),
            (SignalDirection::Long, Direction::Down) | (SignalDirection::Short, Direction::Up)
        );

        if conflicts {
            Some(FilterResult::Block {
                reason: format!(
                    "Microstructure signal ({:?}, strength: {:.2}) conflicts with {} entry",
                    composite.direction,
                    composite.strength,
                    match signal.direction {
                        SignalDirection::Long => "long",
                        SignalDirection::Short => "short",
                        SignalDirection::Exit => "exit",
                    }
                ),
            })
        } else {
            None
        }
    }

    /// Check if entry timing conditions are met.
    fn check_entry_timing(&self, signal: &SignalEvent, micro: &CachedMicroSignals) -> bool {
        let ob = &micro.order_book_imbalance;

        match (&signal.direction, ob.direction) {
            // Long entry requires bullish order book
            (SignalDirection::Long, Direction::Up) => {
                ob.strength >= self.config.timing_support_threshold
            }
            // Short entry requires bearish order book
            (SignalDirection::Short, Direction::Down) => {
                ob.strength >= self.config.timing_support_threshold
            }
            // Neutral order book allows entry
            (SignalDirection::Long, Direction::Neutral)
            | (SignalDirection::Short, Direction::Neutral) => true,
            // Conflicting order book blocks entry
            _ => false,
        }
    }

    /// Apply sizing adjustment under high stress conditions.
    fn apply_sizing_adjustment(
        &self,
        mut signal: SignalEvent,
        micro: &CachedMicroSignals,
    ) -> Option<SignalEvent> {
        if micro.is_high_stress() {
            signal.strength *= self.config.stress_size_multiplier;
            Some(signal)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use algo_trade_core::signal::SignalValue;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    // ============================================
    // Helper Functions
    // ============================================

    fn make_signal(direction: SignalDirection, strength: f64) -> SignalEvent {
        SignalEvent {
            symbol: "BTCUSD".to_string(),
            direction,
            strength,
            price: dec!(50000),
            timestamp: Utc::now(),
        }
    }

    fn make_micro_signals() -> CachedMicroSignals {
        CachedMicroSignals::default()
    }

    // ============================================
    // MicrostructureFilterConfig Tests
    // ============================================

    #[test]
    fn config_default_has_expected_values() {
        let config = MicrostructureFilterConfig::default();

        assert!(config.entry_filter_enabled);
        assert!((config.entry_filter_threshold - 0.6).abs() < f64::EPSILON);
        assert!(config.exit_trigger_enabled);
        assert!((config.exit_liquidation_threshold - 0.8).abs() < f64::EPSILON);
        assert!(config.sizing_adjustment_enabled);
        assert!(!config.entry_timing_enabled); // Off by default
    }

    #[test]
    fn config_disabled_has_all_features_off() {
        let config = MicrostructureFilterConfig::disabled();

        assert!(!config.entry_filter_enabled);
        assert!(!config.exit_trigger_enabled);
        assert!(!config.sizing_adjustment_enabled);
        assert!(!config.entry_timing_enabled);
    }

    #[test]
    fn config_conservative_has_tighter_thresholds() {
        let config = MicrostructureFilterConfig::conservative();

        assert!(config.entry_filter_threshold < 0.6);
        assert!(config.exit_liquidation_threshold < 0.8);
        assert!(config.stress_size_multiplier < 0.5);
    }

    // ============================================
    // FilterResult Tests
    // ============================================

    #[test]
    fn filter_result_is_allowed_true_for_allow() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let result = FilterResult::Allow(signal);
        assert!(result.is_allowed());
    }

    #[test]
    fn filter_result_is_allowed_true_for_modify() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let result = FilterResult::Modify(signal);
        assert!(result.is_allowed());
    }

    #[test]
    fn filter_result_is_allowed_true_for_force_exit() {
        let signal = make_signal(SignalDirection::Exit, 1.0);
        let result = FilterResult::ForceExit {
            reason: "test".to_string(),
            signal,
        };
        assert!(result.is_allowed());
    }

    #[test]
    fn filter_result_is_allowed_false_for_block() {
        let result = FilterResult::Block {
            reason: "test".to_string(),
        };
        assert!(!result.is_allowed());
    }

    #[test]
    fn filter_result_signal_returns_some_for_allow() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let result = FilterResult::Allow(signal.clone());
        assert!(result.signal().is_some());
        assert_eq!(result.signal().unwrap().direction, SignalDirection::Long);
    }

    #[test]
    fn filter_result_signal_returns_none_for_block() {
        let result = FilterResult::Block {
            reason: "test".to_string(),
        };
        assert!(result.signal().is_none());
    }

    // ============================================
    // Entry Filter Tests
    // ============================================

    #[test]
    fn entry_filter_allows_when_no_conflict() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        micro.composite = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Long entry with Up composite should be allowed
        assert!(result.is_allowed());
    }

    #[test]
    fn entry_filter_blocks_conflicting_long() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        micro.composite = SignalValue::new(Direction::Down, 0.8, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Long entry with strong Down composite should be blocked
        assert!(!result.is_allowed());
        if let FilterResult::Block { reason } = result {
            assert!(reason.contains("conflicts"));
            assert!(reason.contains("long"));
        } else {
            panic!("Expected Block result");
        }
    }

    #[test]
    fn entry_filter_blocks_conflicting_short() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        micro.composite = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Short, 0.7);
        let result = filter.apply(signal, &micro);

        // Short entry with strong Up composite should be blocked
        assert!(!result.is_allowed());
    }

    #[test]
    fn entry_filter_allows_below_threshold() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Strength below threshold (0.6)
        micro.composite = SignalValue::new(Direction::Down, 0.5, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Should allow because composite strength is below threshold
        assert!(result.is_allowed());
    }

    #[test]
    fn entry_filter_always_allows_exit() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        micro.composite = SignalValue::new(Direction::Down, 0.9, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Exit, 1.0);
        let result = filter.apply(signal, &micro);

        // Exit signals are never blocked by entry filter
        assert!(result.is_allowed());
    }

    #[test]
    fn entry_filter_disabled_allows_everything() {
        let config = MicrostructureFilterConfig::disabled();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        micro.composite = SignalValue::new(Direction::Down, 0.99, 0.99).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // With filter disabled, conflicting signal should pass
        assert!(result.is_allowed());
    }

    // ============================================
    // Exit Trigger Tests
    // ============================================

    #[test]
    fn exit_trigger_forces_exit_on_liquidation_cascade() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Strong liquidation cascade against longs (Down direction)
        micro.liquidation_cascade = SignalValue::new(Direction::Down, 0.85, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        if let FilterResult::ForceExit { reason, signal } = result {
            assert!(reason.contains("Liquidation cascade"));
            assert_eq!(signal.direction, SignalDirection::Exit);
            assert!((signal.strength - 1.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected ForceExit result");
        }
    }

    #[test]
    fn exit_trigger_forces_exit_on_funding_extreme() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Extreme funding rate against shorts (Up direction = potential short squeeze)
        micro.funding_rate = SignalValue::new(Direction::Up, 0.95, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Short, 0.7);
        let result = filter.apply(signal, &micro);

        if let FilterResult::ForceExit { reason, signal } = result {
            assert!(reason.contains("funding rate"));
            assert_eq!(signal.direction, SignalDirection::Exit);
        } else {
            panic!("Expected ForceExit result");
        }
    }

    #[test]
    fn exit_trigger_no_effect_when_directions_aligned() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Strong liquidation cascade Down, but we're short (aligned)
        micro.liquidation_cascade = SignalValue::new(Direction::Down, 0.85, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Short, 0.7);
        let result = filter.apply(signal, &micro);

        // Should NOT force exit because cascade supports short position
        assert!(!matches!(result, FilterResult::ForceExit { .. }));
    }

    #[test]
    fn exit_trigger_below_threshold_no_effect() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Liquidation cascade below threshold (0.8)
        micro.liquidation_cascade = SignalValue::new(Direction::Down, 0.75, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Should not trigger exit
        assert!(!matches!(result, FilterResult::ForceExit { .. }));
    }

    #[test]
    fn exit_trigger_does_not_affect_exit_signals() {
        let mut config = MicrostructureFilterConfig::default();
        // Disable sizing adjustment to isolate exit trigger behavior
        config.sizing_adjustment_enabled = false;
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        micro.liquidation_cascade = SignalValue::new(Direction::Down, 0.95, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Exit, 1.0);
        let result = filter.apply(signal, &micro);

        // Exit signals should pass through without being turned into ForceExit or Block
        assert!(matches!(result, FilterResult::Allow(_)));
    }

    // ============================================
    // Sizing Adjustment Tests
    // ============================================

    #[test]
    fn sizing_adjustment_reduces_strength_under_stress() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // High stress condition
        micro.liquidation_cascade = SignalValue::new(Direction::Up, 0.75, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.8);
        let result = filter.apply(signal, &micro);

        if let FilterResult::Modify(modified) = result {
            // Default stress_size_multiplier is 0.5
            let expected = 0.8 * 0.5;
            assert!((modified.strength - expected).abs() < f64::EPSILON);
        } else {
            panic!("Expected Modify result, got {:?}", result);
        }
    }

    #[test]
    fn sizing_adjustment_no_effect_when_not_stressed() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let micro = make_micro_signals(); // Default = no stress

        let signal = make_signal(SignalDirection::Long, 0.8);
        let result = filter.apply(signal, &micro);

        // Should allow unchanged
        if let FilterResult::Allow(s) = result {
            assert!((s.strength - 0.8).abs() < f64::EPSILON);
        } else {
            panic!("Expected Allow result");
        }
    }

    #[test]
    fn sizing_adjustment_disabled_no_effect() {
        let config = MicrostructureFilterConfig::disabled();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        micro.liquidation_cascade = SignalValue::new(Direction::Up, 0.9, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.8);
        let result = filter.apply(signal, &micro);

        // With sizing disabled, strength should be unchanged
        if let FilterResult::Allow(s) = result {
            assert!((s.strength - 0.8).abs() < f64::EPSILON);
        } else {
            panic!("Expected Allow result");
        }
    }

    // ============================================
    // Entry Timing Tests
    // ============================================

    #[test]
    fn entry_timing_blocks_when_order_book_conflicts() {
        let mut config = MicrostructureFilterConfig::default();
        config.entry_timing_enabled = true;
        config.entry_filter_enabled = false; // Isolate timing test
        config.sizing_adjustment_enabled = false;
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Order book shows bearish imbalance
        micro.order_book_imbalance = SignalValue::new(Direction::Down, 0.6, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Long entry blocked due to bearish order book
        if let FilterResult::Block { reason } = result {
            assert!(reason.contains("order book"));
        } else {
            panic!("Expected Block result");
        }
    }

    #[test]
    fn entry_timing_allows_when_order_book_supports() {
        let mut config = MicrostructureFilterConfig::default();
        config.entry_timing_enabled = true;
        config.entry_filter_enabled = false;
        config.sizing_adjustment_enabled = false;
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Order book supports long entry
        micro.order_book_imbalance = SignalValue::new(Direction::Up, 0.5, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        assert!(result.is_allowed());
    }

    #[test]
    fn entry_timing_allows_neutral_order_book() {
        let mut config = MicrostructureFilterConfig::default();
        config.entry_timing_enabled = true;
        config.entry_filter_enabled = false;
        config.sizing_adjustment_enabled = false;
        let filter = MicrostructureFilter::new(config);

        let micro = make_micro_signals(); // Neutral order book

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Neutral order book allows entry
        assert!(result.is_allowed());
    }

    #[test]
    fn entry_timing_requires_minimum_strength() {
        let mut config = MicrostructureFilterConfig::default();
        config.entry_timing_enabled = true;
        config.timing_support_threshold = 0.5;
        config.entry_filter_enabled = false;
        config.sizing_adjustment_enabled = false;
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Order book direction is correct but strength below threshold
        micro.order_book_imbalance = SignalValue::new(Direction::Up, 0.3, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Should block because order book strength < threshold
        if let FilterResult::Block { reason } = result {
            assert!(reason.contains("order book"));
        } else {
            panic!("Expected Block result");
        }
    }

    // ============================================
    // Priority Order Tests
    // ============================================

    #[test]
    fn exit_trigger_has_priority_over_entry_filter() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Both exit trigger and entry filter conditions
        micro.liquidation_cascade = SignalValue::new(Direction::Down, 0.9, 0.9).unwrap();
        micro.composite = SignalValue::new(Direction::Down, 0.8, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Exit trigger should fire, not entry filter block
        assert!(matches!(result, FilterResult::ForceExit { .. }));
    }

    #[test]
    fn entry_filter_has_priority_over_sizing() {
        let config = MicrostructureFilterConfig::default();
        let filter = MicrostructureFilter::new(config);

        let mut micro = make_micro_signals();
        // Entry filter condition (conflict)
        micro.composite = SignalValue::new(Direction::Down, 0.8, 0.9).unwrap();
        // Also high stress
        micro.liquidation_cascade = SignalValue::new(Direction::Up, 0.75, 0.9).unwrap();

        let signal = make_signal(SignalDirection::Long, 0.7);
        let result = filter.apply(signal, &micro);

        // Should be blocked, not modified
        assert!(matches!(result, FilterResult::Block { .. }));
    }

    // ============================================
    // Config Mutation Tests
    // ============================================

    #[test]
    fn config_mut_allows_modification() {
        let config = MicrostructureFilterConfig::default();
        let mut filter = MicrostructureFilter::new(config);

        filter.config_mut().entry_filter_threshold = 0.9;

        assert!((filter.config().entry_filter_threshold - 0.9).abs() < f64::EPSILON);
    }
}
