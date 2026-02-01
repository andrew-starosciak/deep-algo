//! Strategy wrapper that enhances trading strategies with microstructure filtering.
//!
//! `EnhancedStrategy<S>` wraps any `Strategy` implementation and applies
//! microstructure signal filtering to its output.

use anyhow::Result;
use async_trait::async_trait;

use algo_trade_core::events::{MarketEvent, SignalDirection, SignalEvent};
use algo_trade_core::traits::Strategy;

use super::filter::{FilterResult, MicrostructureFilter, MicrostructureFilterConfig};
use super::CachedMicroSignals;
use super::SharedMicroSignals;

/// Wraps a base strategy with microstructure filtering.
///
/// The enhanced strategy:
/// 1. Reads cached microstructure signals (sync via RwLock read)
/// 2. Checks for forced exit conditions before processing events
/// 3. Applies entry filter, timing, and sizing to strategy signals
///
/// # Type Parameters
///
/// * `S` - The base strategy type that implements `Strategy`
///
/// # Example
///
/// ```ignore
/// use algo_trade_signals::bridge::{EnhancedStrategy, MicrostructureFilterConfig, SharedMicroSignals};
///
/// let signals: SharedMicroSignals = Arc::new(RwLock::new(CachedMicroSignals::default()));
///
/// let enhanced = EnhancedStrategy::new(
///     MaCrossoverStrategy::new("BTCUSD".to_string(), 10, 30),
///     signals,
///     MicrostructureFilterConfig::default(),
/// )
/// .with_entry_filter(0.6)
/// .with_exit_triggers(0.8, 0.9);
/// ```
pub struct EnhancedStrategy<S: Strategy> {
    inner: S,
    signals: SharedMicroSignals,
    filter: MicrostructureFilter,
    last_signal: Option<SignalEvent>,
}

impl<S: Strategy> EnhancedStrategy<S> {
    /// Creates a new enhanced strategy.
    ///
    /// # Arguments
    ///
    /// * `strategy` - The base strategy to wrap
    /// * `signals` - Shared microstructure signal cache
    /// * `config` - Filter configuration
    #[must_use]
    pub fn new(
        strategy: S,
        signals: SharedMicroSignals,
        config: MicrostructureFilterConfig,
    ) -> Self {
        Self {
            inner: strategy,
            signals,
            filter: MicrostructureFilter::new(config),
            last_signal: None,
        }
    }

    /// Enables entry filtering with the specified threshold.
    ///
    /// Entries will be blocked when microstructure composite signal
    /// conflicts with strategy direction and strength exceeds threshold.
    #[must_use]
    pub fn with_entry_filter(mut self, threshold: f64) -> Self {
        self.filter.config_mut().entry_filter_enabled = true;
        self.filter.config_mut().entry_filter_threshold = threshold;
        self
    }

    /// Enables exit triggers with the specified thresholds.
    ///
    /// Forced exits will be triggered when:
    /// - Liquidation cascade strength exceeds `liquidation_threshold` against position
    /// - Funding rate strength exceeds `funding_threshold` against position
    #[must_use]
    pub fn with_exit_triggers(
        mut self,
        liquidation_threshold: f64,
        funding_threshold: f64,
    ) -> Self {
        self.filter.config_mut().exit_trigger_enabled = true;
        self.filter.config_mut().exit_liquidation_threshold = liquidation_threshold;
        self.filter.config_mut().exit_funding_threshold = funding_threshold;
        self
    }

    /// Enables position sizing adjustment based on market stress.
    ///
    /// Signal strength will be multiplied by `stress_multiplier` under high stress.
    #[must_use]
    pub fn with_sizing_adjustment(mut self, stress_multiplier: f64) -> Self {
        self.filter.config_mut().sizing_adjustment_enabled = true;
        self.filter.config_mut().stress_size_multiplier = stress_multiplier;
        self
    }

    /// Enables entry timing based on order book support.
    ///
    /// Entries will wait until order book imbalance supports the direction
    /// with at least `support_threshold` strength.
    #[must_use]
    pub fn with_entry_timing(mut self, support_threshold: f64) -> Self {
        self.filter.config_mut().entry_timing_enabled = true;
        self.filter.config_mut().timing_support_threshold = support_threshold;
        self
    }

    /// Returns a reference to the inner strategy.
    #[must_use]
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Returns a mutable reference to the inner strategy.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Returns a reference to the filter.
    #[must_use]
    pub fn filter(&self) -> &MicrostructureFilter {
        &self.filter
    }

    /// Returns the last signal produced (before filtering).
    #[must_use]
    pub fn last_signal(&self) -> Option<&SignalEvent> {
        self.last_signal.as_ref()
    }

    /// Checks if a forced exit should be triggered based on current position.
    ///
    /// This is called before processing market events to proactively exit
    /// positions under extreme microstructure conditions.
    fn check_proactive_exit(&self, micro: &CachedMicroSignals) -> Option<FilterResult> {
        if !self.filter.config().exit_trigger_enabled {
            return None;
        }

        let Some(last) = &self.last_signal else {
            return None;
        };

        // Only check for open positions (not exits)
        if last.direction == SignalDirection::Exit {
            return None;
        }

        // Create a dummy signal to check exit conditions
        let check_signal = SignalEvent {
            direction: last.direction.clone(),
            symbol: last.symbol.clone(),
            price: last.price,
            strength: last.strength,
            timestamp: last.timestamp,
        };

        let result = self.filter.apply(check_signal, micro);

        match result {
            FilterResult::ForceExit { .. } => Some(result),
            _ => None,
        }
    }
}

#[async_trait]
impl<S: Strategy> Strategy for EnhancedStrategy<S> {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        // Read cached microstructure signals
        let micro = {
            let guard = self.signals.read().await;
            guard.clone()
        };

        // Check for proactive exit based on microstructure
        if let Some(FilterResult::ForceExit { reason, signal }) = self.check_proactive_exit(&micro)
        {
            tracing::info!(
                "Microstructure forced exit: {} (symbol: {})",
                reason,
                signal.symbol
            );
            self.last_signal = Some(signal.clone());
            return Ok(Some(signal));
        }

        // Process event through inner strategy
        let maybe_signal = self.inner.on_market_event(event).await?;

        let Some(signal) = maybe_signal else {
            return Ok(None);
        };

        // Save original strength for logging
        let original_strength = signal.strength;

        // Apply microstructure filter
        match self.filter.apply(signal, &micro) {
            FilterResult::Allow(s) => {
                self.last_signal = Some(s.clone());
                Ok(Some(s))
            }
            FilterResult::Block { reason } => {
                tracing::debug!("Signal blocked by microstructure filter: {}", reason);
                Ok(None)
            }
            FilterResult::Modify(s) => {
                tracing::debug!(
                    "Signal modified by microstructure filter: strength {} -> {}",
                    original_strength,
                    s.strength
                );
                self.last_signal = Some(s.clone());
                Ok(Some(s))
            }
            FilterResult::ForceExit { reason, signal } => {
                tracing::info!("Microstructure forced exit: {}", reason);
                self.last_signal = Some(signal.clone());
                Ok(Some(signal))
            }
        }
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use algo_trade_core::signal::{Direction, SignalValue};
    use chrono::Utc;
    use rust_decimal_macros::dec;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    // ============================================
    // Mock Strategy for Testing
    // ============================================

    struct MockStrategy {
        name: &'static str,
        next_signal: Option<SignalEvent>,
        events_received: Vec<MarketEvent>,
    }

    impl MockStrategy {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                next_signal: None,
                events_received: Vec::new(),
            }
        }

        fn with_signal(mut self, signal: SignalEvent) -> Self {
            self.next_signal = Some(signal);
            self
        }
    }

    #[async_trait]
    impl Strategy for MockStrategy {
        async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
            self.events_received.push(event.clone());
            Ok(self.next_signal.take())
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

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

    fn make_bar_event() -> MarketEvent {
        MarketEvent::Bar {
            symbol: "BTCUSD".to_string(),
            open: dec!(50000),
            high: dec!(50100),
            low: dec!(49900),
            close: dec!(50050),
            volume: dec!(100),
            timestamp: Utc::now(),
        }
    }

    fn make_shared_signals() -> SharedMicroSignals {
        Arc::new(RwLock::new(CachedMicroSignals::default()))
    }

    // ============================================
    // Constructor Tests
    // ============================================

    #[test]
    fn new_creates_enhanced_strategy() {
        let strategy = MockStrategy::new("test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::default();

        let enhanced = EnhancedStrategy::new(strategy, signals, config);

        assert_eq!(enhanced.name(), "test");
        assert!(enhanced.last_signal().is_none());
    }

    #[test]
    fn inner_returns_reference_to_base_strategy() {
        let strategy = MockStrategy::new("inner_test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::default();

        let enhanced = EnhancedStrategy::new(strategy, signals, config);

        assert_eq!(enhanced.inner().name, "inner_test");
    }

    // ============================================
    // Builder Pattern Tests
    // ============================================

    #[test]
    fn with_entry_filter_configures_filter() {
        let strategy = MockStrategy::new("test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let enhanced = EnhancedStrategy::new(strategy, signals, config).with_entry_filter(0.7);

        assert!(enhanced.filter().config().entry_filter_enabled);
        assert!((enhanced.filter().config().entry_filter_threshold - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn with_exit_triggers_configures_filter() {
        let strategy = MockStrategy::new("test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let enhanced =
            EnhancedStrategy::new(strategy, signals, config).with_exit_triggers(0.75, 0.85);

        assert!(enhanced.filter().config().exit_trigger_enabled);
        assert!(
            (enhanced.filter().config().exit_liquidation_threshold - 0.75).abs() < f64::EPSILON
        );
        assert!((enhanced.filter().config().exit_funding_threshold - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn with_sizing_adjustment_configures_filter() {
        let strategy = MockStrategy::new("test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let enhanced = EnhancedStrategy::new(strategy, signals, config).with_sizing_adjustment(0.3);

        assert!(enhanced.filter().config().sizing_adjustment_enabled);
        assert!((enhanced.filter().config().stress_size_multiplier - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn with_entry_timing_configures_filter() {
        let strategy = MockStrategy::new("test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let enhanced = EnhancedStrategy::new(strategy, signals, config).with_entry_timing(0.4);

        assert!(enhanced.filter().config().entry_timing_enabled);
        assert!((enhanced.filter().config().timing_support_threshold - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn builder_methods_can_be_chained() {
        let strategy = MockStrategy::new("test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let enhanced = EnhancedStrategy::new(strategy, signals, config)
            .with_entry_filter(0.5)
            .with_exit_triggers(0.7, 0.8)
            .with_sizing_adjustment(0.4)
            .with_entry_timing(0.3);

        assert!(enhanced.filter().config().entry_filter_enabled);
        assert!(enhanced.filter().config().exit_trigger_enabled);
        assert!(enhanced.filter().config().sizing_adjustment_enabled);
        assert!(enhanced.filter().config().entry_timing_enabled);
    }

    // ============================================
    // Signal Passthrough Tests
    // ============================================

    #[tokio::test]
    async fn passthrough_when_no_filter_conditions() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal.clone());
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let mut enhanced = EnhancedStrategy::new(strategy, signals, config);
        let event = make_bar_event();

        let result = enhanced.on_market_event(&event).await.unwrap();

        assert!(result.is_some());
        let output = result.unwrap();
        assert_eq!(output.direction, SignalDirection::Long);
        assert!((output.strength - 0.8).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn passthrough_none_when_strategy_returns_none() {
        let strategy = MockStrategy::new("test"); // No signal set
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::default();

        let mut enhanced = EnhancedStrategy::new(strategy, signals, config);
        let event = make_bar_event();

        let result = enhanced.on_market_event(&event).await.unwrap();

        assert!(result.is_none());
    }

    // ============================================
    // Entry Filter Integration Tests
    // ============================================

    #[tokio::test]
    async fn entry_filter_blocks_conflicting_signal() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal);
        let signals = make_shared_signals();

        // Set bearish microstructure
        {
            let mut guard = signals.write().await;
            guard.composite = SignalValue::new(Direction::Down, 0.8, 0.9).unwrap();
        }

        let config = MicrostructureFilterConfig::default();
        let mut enhanced = EnhancedStrategy::new(strategy, signals, config).with_entry_filter(0.6);

        let event = make_bar_event();
        let result = enhanced.on_market_event(&event).await.unwrap();

        // Long signal should be blocked due to bearish microstructure
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn entry_filter_allows_aligned_signal() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal);
        let signals = make_shared_signals();

        // Set bullish microstructure
        {
            let mut guard = signals.write().await;
            guard.composite = SignalValue::new(Direction::Up, 0.8, 0.9).unwrap();
        }

        let config = MicrostructureFilterConfig::default();
        let mut enhanced = EnhancedStrategy::new(strategy, signals, config).with_entry_filter(0.6);

        let event = make_bar_event();
        let result = enhanced.on_market_event(&event).await.unwrap();

        // Long signal should be allowed with bullish microstructure
        assert!(result.is_some());
        assert_eq!(result.unwrap().direction, SignalDirection::Long);
    }

    // ============================================
    // Exit Trigger Integration Tests
    // ============================================

    #[tokio::test]
    async fn exit_trigger_forces_exit() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal);
        let signals = make_shared_signals();

        // Set strong liquidation cascade against longs
        {
            let mut guard = signals.write().await;
            guard.liquidation_cascade = SignalValue::new(Direction::Down, 0.9, 0.9).unwrap();
        }

        let config = MicrostructureFilterConfig::default();
        let mut enhanced =
            EnhancedStrategy::new(strategy, signals, config).with_exit_triggers(0.8, 0.9);

        let event = make_bar_event();
        let result = enhanced.on_market_event(&event).await.unwrap();

        // Should receive forced exit signal
        assert!(result.is_some());
        let output = result.unwrap();
        assert_eq!(output.direction, SignalDirection::Exit);
        assert!((output.strength - 1.0).abs() < f64::EPSILON);
    }

    // ============================================
    // Sizing Adjustment Integration Tests
    // ============================================

    #[tokio::test]
    async fn sizing_adjustment_reduces_strength() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal);
        let signals = make_shared_signals();

        // Set high stress condition
        {
            let mut guard = signals.write().await;
            guard.liquidation_cascade = SignalValue::new(Direction::Up, 0.75, 0.9).unwrap();
        }

        let config = MicrostructureFilterConfig::default();
        let mut enhanced =
            EnhancedStrategy::new(strategy, signals, config).with_sizing_adjustment(0.5);

        let event = make_bar_event();
        let result = enhanced.on_market_event(&event).await.unwrap();

        assert!(result.is_some());
        let output = result.unwrap();
        // Strength should be reduced by 50%
        assert!((output.strength - 0.4).abs() < f64::EPSILON);
    }

    // ============================================
    // Last Signal Tracking Tests
    // ============================================

    #[tokio::test]
    async fn last_signal_updated_on_allowed_signal() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal);
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let mut enhanced = EnhancedStrategy::new(strategy, signals, config);
        assert!(enhanced.last_signal().is_none());

        let event = make_bar_event();
        let _ = enhanced.on_market_event(&event).await.unwrap();

        assert!(enhanced.last_signal().is_some());
        assert_eq!(
            enhanced.last_signal().unwrap().direction,
            SignalDirection::Long
        );
    }

    #[tokio::test]
    async fn last_signal_not_updated_on_blocked_signal() {
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal);
        let signals = make_shared_signals();

        // Set blocking condition
        {
            let mut guard = signals.write().await;
            guard.composite = SignalValue::new(Direction::Down, 0.8, 0.9).unwrap();
        }

        let config = MicrostructureFilterConfig::default();
        let mut enhanced = EnhancedStrategy::new(strategy, signals, config).with_entry_filter(0.6);

        let event = make_bar_event();
        let _ = enhanced.on_market_event(&event).await.unwrap();

        // last_signal should not be updated when signal is blocked
        assert!(enhanced.last_signal().is_none());
    }

    // ============================================
    // Proactive Exit Tests
    // ============================================

    #[tokio::test]
    async fn proactive_exit_triggered_on_existing_position() {
        // First, establish a position
        let signal = make_signal(SignalDirection::Long, 0.8);
        let strategy = MockStrategy::new("test").with_signal(signal);
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let mut enhanced =
            EnhancedStrategy::new(strategy, signals.clone(), config).with_exit_triggers(0.8, 0.9);

        // Process first event to establish position
        let event = make_bar_event();
        let _ = enhanced.on_market_event(&event).await.unwrap();

        // Now set extreme conditions
        {
            let mut guard = signals.write().await;
            guard.liquidation_cascade = SignalValue::new(Direction::Down, 0.9, 0.9).unwrap();
        }

        // Process another event - should trigger proactive exit
        let event2 = make_bar_event();
        let result = enhanced.on_market_event(&event2).await.unwrap();

        assert!(result.is_some());
        assert_eq!(result.unwrap().direction, SignalDirection::Exit);
    }

    // ============================================
    // Strategy Name Delegation Tests
    // ============================================

    #[test]
    fn name_delegates_to_inner_strategy() {
        let strategy = MockStrategy::new("my_strategy");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::default();

        let enhanced = EnhancedStrategy::new(strategy, signals, config);

        assert_eq!(enhanced.name(), "my_strategy");
    }

    // ============================================
    // Event Forwarding Tests
    // ============================================

    #[tokio::test]
    async fn events_are_forwarded_to_inner_strategy() {
        let strategy = MockStrategy::new("test");
        let signals = make_shared_signals();
        let config = MicrostructureFilterConfig::disabled();

        let mut enhanced = EnhancedStrategy::new(strategy, signals, config);

        let event = make_bar_event();
        let _ = enhanced.on_market_event(&event).await.unwrap();

        // Inner strategy should have received the event
        assert_eq!(enhanced.inner().events_received.len(), 1);
    }
}
