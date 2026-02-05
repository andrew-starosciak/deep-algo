//! Settlement handler for 15-minute window outcomes.
//!
//! This module handles the settlement of binary option positions after each
//! 15-minute window closes. It determines the outcome (YES won or NO won) and
//! calculates realized P&L for both hedged and unhedged positions.
//!
//! # Settlement Logic
//!
//! - **Hedged positions**: Guaranteed profit = 1.0 - total_cost (win regardless of outcome)
//! - **Unhedged positions**: Binary outcome based on BTC price vs reference
//!   - YES held and YES wins: profit = 1.0 - entry_price
//!   - YES held and NO wins: loss = -entry_price
//!   - NO held and NO wins: profit = 1.0 - entry_price
//!   - NO held and YES wins: loss = -entry_price
//!
//! # Settlement Timing
//!
//! Windows settle approximately 1 minute after close. The outcome is determined
//! by comparing the final BTC spot price to the window's reference price.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

use super::auto_executor::WindowPositionTracker;
use super::gabagool_detector::GabagoolDirection;
use super::reference_tracker::WINDOW_DURATION_MS;

/// The outcome of a settled window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowOutcome {
    /// YES won - BTC finished above reference price.
    YesWon,
    /// NO won - BTC finished below reference price.
    NoWon,
}

impl WindowOutcome {
    /// Returns the winning direction.
    #[must_use]
    pub fn winning_direction(&self) -> GabagoolDirection {
        match self {
            Self::YesWon => GabagoolDirection::Yes,
            Self::NoWon => GabagoolDirection::No,
        }
    }
}

/// Settlement result for a single window position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementResult {
    /// Window start timestamp (ms).
    pub window_start_ms: i64,
    /// The outcome of the window.
    pub outcome: WindowOutcome,
    /// Whether the position was hedged.
    pub was_hedged: bool,
    /// Total cost invested.
    pub total_cost: Decimal,
    /// Payout received (shares * $1.00 for winning side).
    pub payout: Decimal,
    /// Realized P&L.
    pub realized_pnl: Decimal,
    /// Settlement timestamp.
    pub settled_at: DateTime<Utc>,
}

/// Configuration for the settlement handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementConfig {
    /// Delay (ms) after window close before checking settlement.
    /// Polymarket settles shortly after window ends.
    pub settlement_delay_ms: i64,
    /// Maximum age (ms) for pending settlements before considering them stale.
    pub max_pending_age_ms: i64,
}

impl Default for SettlementConfig {
    fn default() -> Self {
        Self {
            settlement_delay_ms: 60_000,        // 1 minute
            max_pending_age_ms: 30 * 60 * 1000, // 30 minutes
        }
    }
}

/// Handles settlement of window positions.
///
/// Tracks positions awaiting settlement and calculates P&L when outcomes
/// are determined.
#[derive(Debug)]
pub struct SettlementHandler {
    /// Market condition ID for the BTC 15-min market.
    condition_id: String,
    /// Pending settlements: window_start_ms -> positions.
    pending: HashMap<i64, WindowPositionTracker>,
    /// Settlement history.
    history: Vec<SettlementResult>,
    /// Configuration.
    config: SettlementConfig,
    /// Total realized P&L across all settlements.
    total_realized_pnl: Decimal,
    /// Number of settlements processed.
    settlements_count: u64,
    /// Number of winning settlements.
    winning_settlements: u64,
}

impl SettlementHandler {
    /// Creates a new settlement handler.
    #[must_use]
    pub fn new(condition_id: impl Into<String>, config: SettlementConfig) -> Self {
        Self {
            condition_id: condition_id.into(),
            pending: HashMap::new(),
            history: Vec::new(),
            config,
            total_realized_pnl: Decimal::ZERO,
            settlements_count: 0,
            winning_settlements: 0,
        }
    }

    /// Creates a handler with default config.
    #[must_use]
    pub fn with_defaults(condition_id: impl Into<String>) -> Self {
        Self::new(condition_id, SettlementConfig::default())
    }

    /// Records a position for later settlement.
    ///
    /// Call this when transitioning to a new window to track the old position.
    pub fn record_position(&mut self, position: WindowPositionTracker) {
        if !position.has_position() {
            return;
        }

        let window_start = position.window_start_ms;
        info!(
            window_start = window_start,
            hedged = position.is_hedged(),
            total_cost = %position.total_cost,
            "Recording position for settlement"
        );

        self.pending.insert(window_start, position);
    }

    /// Checks for windows that can be settled.
    ///
    /// Returns windows that have passed the settlement delay.
    pub fn settleable_windows(&self, current_time_ms: i64) -> Vec<i64> {
        self.pending
            .keys()
            .filter(|&&window_start| {
                let window_end = window_start + WINDOW_DURATION_MS;
                let settlement_time = window_end + self.config.settlement_delay_ms;
                current_time_ms >= settlement_time
            })
            .copied()
            .collect()
    }

    /// Settles a window with the given outcome.
    ///
    /// Returns the settlement result if there was a position for this window.
    pub fn settle_window(
        &mut self,
        window_start_ms: i64,
        outcome: WindowOutcome,
    ) -> Option<SettlementResult> {
        let position = self.pending.remove(&window_start_ms)?;

        let result = self.calculate_settlement(&position, outcome);

        self.total_realized_pnl += result.realized_pnl;
        self.settlements_count += 1;
        if result.realized_pnl > Decimal::ZERO {
            self.winning_settlements += 1;
        }

        info!(
            window_start = window_start_ms,
            outcome = ?outcome,
            hedged = result.was_hedged,
            pnl = %result.realized_pnl,
            "Window settled"
        );

        self.history.push(result.clone());
        Some(result)
    }

    /// Calculates the settlement result for a position.
    fn calculate_settlement(
        &self,
        position: &WindowPositionTracker,
        outcome: WindowOutcome,
    ) -> SettlementResult {
        let was_hedged = position.is_hedged();
        let total_cost = position.total_cost;

        let payout = if was_hedged {
            // Hedged: we hold both YES and NO, one wins
            // Payout = min(yes_qty, no_qty) * $1.00
            let yes_qty = position
                .yes_position
                .as_ref()
                .map_or(Decimal::ZERO, |p| p.quantity);
            let no_qty = position
                .no_position
                .as_ref()
                .map_or(Decimal::ZERO, |p| p.quantity);
            yes_qty.min(no_qty)
        } else {
            // Unhedged: only one side, binary outcome
            match (&position.yes_position, &position.no_position) {
                (Some(yes_pos), None) => {
                    // Holding YES
                    if outcome == WindowOutcome::YesWon {
                        yes_pos.quantity // $1.00 per share
                    } else {
                        Decimal::ZERO // Lose everything
                    }
                }
                (None, Some(no_pos)) => {
                    // Holding NO
                    if outcome == WindowOutcome::NoWon {
                        no_pos.quantity // $1.00 per share
                    } else {
                        Decimal::ZERO // Lose everything
                    }
                }
                _ => Decimal::ZERO, // No position or both (shouldn't happen for unhedged)
            }
        };

        let realized_pnl = payout - total_cost;

        SettlementResult {
            window_start_ms: position.window_start_ms,
            outcome,
            was_hedged,
            total_cost,
            payout,
            realized_pnl,
            settled_at: Utc::now(),
        }
    }

    /// Determines the outcome based on spot price vs reference.
    ///
    /// This is a helper for simulated settlement when actual Polymarket
    /// settlement data is not available.
    #[must_use]
    pub fn determine_outcome(spot_price: f64, reference_price: f64) -> WindowOutcome {
        if spot_price > reference_price {
            WindowOutcome::YesWon
        } else {
            WindowOutcome::NoWon
        }
    }

    /// Removes stale pending settlements.
    ///
    /// Returns the number of stale positions removed.
    pub fn cleanup_stale(&mut self, current_time_ms: i64) -> usize {
        let stale_threshold = current_time_ms - self.config.max_pending_age_ms;

        let stale_windows: Vec<i64> = self
            .pending
            .keys()
            .filter(|&&window_start| window_start < stale_threshold)
            .copied()
            .collect();

        let count = stale_windows.len();
        for window in stale_windows {
            warn!(window_start = window, "Removing stale pending settlement");
            self.pending.remove(&window);
        }

        count
    }

    /// Returns the number of pending settlements.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Returns the total realized P&L.
    #[must_use]
    pub fn total_realized_pnl(&self) -> Decimal {
        self.total_realized_pnl
    }

    /// Returns the number of settlements.
    #[must_use]
    pub fn settlements_count(&self) -> u64 {
        self.settlements_count
    }

    /// Returns the win rate (settlements with positive P&L).
    #[must_use]
    pub fn win_rate(&self) -> f64 {
        if self.settlements_count == 0 {
            0.0
        } else {
            self.winning_settlements as f64 / self.settlements_count as f64
        }
    }

    /// Returns the settlement history.
    #[must_use]
    pub fn history(&self) -> &[SettlementResult] {
        &self.history
    }

    /// Returns the condition ID.
    #[must_use]
    pub fn condition_id(&self) -> &str {
        &self.condition_id
    }

    /// Clears all state.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.history.clear();
        self.total_realized_pnl = Decimal::ZERO;
        self.settlements_count = 0;
        self.winning_settlements = 0;
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitrage::gabagool_detector::OpenPosition;
    use rust_decimal_macros::dec;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn make_window_position(
        window_start_ms: i64,
        yes_price: Option<Decimal>,
        no_price: Option<Decimal>,
        quantity: Decimal,
    ) -> WindowPositionTracker {
        let mut tracker = WindowPositionTracker::new(window_start_ms);

        if let Some(price) = yes_price {
            tracker.record_entry(OpenPosition {
                direction: GabagoolDirection::Yes,
                entry_price: price,
                quantity,
                entry_time_ms: window_start_ms + 60_000,
                window_start_ms,
            });
        }

        if let Some(price) = no_price {
            if yes_price.is_some() {
                tracker.record_hedge(OpenPosition {
                    direction: GabagoolDirection::No,
                    entry_price: price,
                    quantity,
                    entry_time_ms: window_start_ms + 120_000,
                    window_start_ms,
                });
            } else {
                tracker.record_entry(OpenPosition {
                    direction: GabagoolDirection::No,
                    entry_price: price,
                    quantity,
                    entry_time_ms: window_start_ms + 60_000,
                    window_start_ms,
                });
            }
        }

        tracker
    }

    // =========================================================================
    // Settlement P&L Tests - Hedged Positions
    // =========================================================================

    #[test]
    fn test_hedged_position_pnl_yes_wins() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Hedged position: YES at $0.35, NO at $0.60 = $0.95 total cost
        // Guaranteed profit = $1.00 - $0.95 = $0.05 per share
        let position = make_window_position(0, Some(dec!(0.35)), Some(dec!(0.60)), dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::YesWon).unwrap();

        assert!(result.was_hedged);
        assert_eq!(result.total_cost, dec!(95)); // 100 * (0.35 + 0.60)
        assert_eq!(result.payout, dec!(100)); // min(100, 100) * $1.00
        assert_eq!(result.realized_pnl, dec!(5)); // 100 - 95
    }

    #[test]
    fn test_hedged_position_pnl_no_wins() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Same hedged position, but NO wins - profit should be the same!
        let position = make_window_position(0, Some(dec!(0.35)), Some(dec!(0.60)), dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::NoWon).unwrap();

        assert!(result.was_hedged);
        assert_eq!(result.total_cost, dec!(95));
        assert_eq!(result.payout, dec!(100));
        assert_eq!(result.realized_pnl, dec!(5)); // Guaranteed profit regardless of outcome
    }

    #[test]
    fn test_hedged_position_breakeven() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Hedged at exactly $1.00 total = breakeven
        let position = make_window_position(0, Some(dec!(0.50)), Some(dec!(0.50)), dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::YesWon).unwrap();

        assert_eq!(result.total_cost, dec!(100));
        assert_eq!(result.payout, dec!(100));
        assert_eq!(result.realized_pnl, Decimal::ZERO);
    }

    #[test]
    fn test_hedged_position_loss_expensive_hedge() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Hedged at > $1.00 = guaranteed loss
        let position = make_window_position(0, Some(dec!(0.55)), Some(dec!(0.50)), dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::YesWon).unwrap();

        assert_eq!(result.total_cost, dec!(105)); // 100 * (0.55 + 0.50)
        assert_eq!(result.payout, dec!(100));
        assert_eq!(result.realized_pnl, dec!(-5)); // Loss
    }

    // =========================================================================
    // Settlement P&L Tests - Unhedged Positions (YES)
    // =========================================================================

    #[test]
    fn test_unhedged_yes_wins() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Holding YES at $0.35, YES wins
        let position = make_window_position(0, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::YesWon).unwrap();

        assert!(!result.was_hedged);
        assert_eq!(result.total_cost, dec!(35)); // 100 * 0.35
        assert_eq!(result.payout, dec!(100)); // Win: 100 * $1.00
        assert_eq!(result.realized_pnl, dec!(65)); // 100 - 35
    }

    #[test]
    fn test_unhedged_yes_loses() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Holding YES at $0.35, NO wins (we lose)
        let position = make_window_position(0, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::NoWon).unwrap();

        assert!(!result.was_hedged);
        assert_eq!(result.total_cost, dec!(35));
        assert_eq!(result.payout, Decimal::ZERO); // Lose: $0.00
        assert_eq!(result.realized_pnl, dec!(-35)); // Lost our entire stake
    }

    // =========================================================================
    // Settlement P&L Tests - Unhedged Positions (NO)
    // =========================================================================

    #[test]
    fn test_unhedged_no_wins() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Holding NO at $0.40, NO wins
        let position = make_window_position(0, None, Some(dec!(0.40)), dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::NoWon).unwrap();

        assert!(!result.was_hedged);
        assert_eq!(result.total_cost, dec!(40)); // 100 * 0.40
        assert_eq!(result.payout, dec!(100)); // Win: 100 * $1.00
        assert_eq!(result.realized_pnl, dec!(60)); // 100 - 40
    }

    #[test]
    fn test_unhedged_no_loses() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Holding NO at $0.40, YES wins (we lose)
        let position = make_window_position(0, None, Some(dec!(0.40)), dec!(100));
        handler.record_position(position);

        let result = handler.settle_window(0, WindowOutcome::YesWon).unwrap();

        assert!(!result.was_hedged);
        assert_eq!(result.total_cost, dec!(40));
        assert_eq!(result.payout, Decimal::ZERO);
        assert_eq!(result.realized_pnl, dec!(-40)); // Lost our entire stake
    }

    // =========================================================================
    // Window Transition Tests
    // =========================================================================

    #[test]
    fn test_record_and_settle_multiple_windows() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Window 1: hedged profit
        let pos1 = make_window_position(0, Some(dec!(0.35)), Some(dec!(0.60)), dec!(100));
        handler.record_position(pos1);

        // Window 2: unhedged win
        let pos2 = make_window_position(WINDOW_DURATION_MS, Some(dec!(0.30)), None, dec!(100));
        handler.record_position(pos2);

        assert_eq!(handler.pending_count(), 2);

        // Settle window 1
        let result1 = handler.settle_window(0, WindowOutcome::YesWon).unwrap();
        assert_eq!(result1.realized_pnl, dec!(5));

        // Settle window 2
        let result2 = handler
            .settle_window(WINDOW_DURATION_MS, WindowOutcome::YesWon)
            .unwrap();
        assert_eq!(result2.realized_pnl, dec!(70)); // 100 - 30

        assert_eq!(handler.pending_count(), 0);
        assert_eq!(handler.total_realized_pnl(), dec!(75)); // 5 + 70
        assert_eq!(handler.settlements_count(), 2);
    }

    #[test]
    fn test_no_settlement_for_empty_position() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Empty position (no YES or NO)
        let position = WindowPositionTracker::new(0);
        handler.record_position(position);

        // Empty positions are not recorded
        assert_eq!(handler.pending_count(), 0);

        // Settlement returns None
        let result = handler.settle_window(0, WindowOutcome::YesWon);
        assert!(result.is_none());
    }

    // =========================================================================
    // Outcome Determination Tests
    // =========================================================================

    #[test]
    fn test_determine_outcome_yes_wins() {
        // Spot > Reference = YES wins
        let outcome = SettlementHandler::determine_outcome(78500.0, 78000.0);
        assert_eq!(outcome, WindowOutcome::YesWon);
    }

    #[test]
    fn test_determine_outcome_no_wins() {
        // Spot < Reference = NO wins
        let outcome = SettlementHandler::determine_outcome(77500.0, 78000.0);
        assert_eq!(outcome, WindowOutcome::NoWon);
    }

    #[test]
    fn test_determine_outcome_exact_equals_no_wins() {
        // Spot == Reference = NO wins (not strictly greater)
        let outcome = SettlementHandler::determine_outcome(78000.0, 78000.0);
        assert_eq!(outcome, WindowOutcome::NoWon);
    }

    // =========================================================================
    // Settleable Windows Tests
    // =========================================================================

    #[test]
    fn test_settleable_windows_timing() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        let window_start = 0_i64;
        let window_end = WINDOW_DURATION_MS;
        let settlement_time = window_end + 60_000; // 1 minute after close

        let position = make_window_position(window_start, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(position);

        // Before window ends - not settleable
        let mid_window = window_start + WINDOW_DURATION_MS / 2;
        assert!(handler.settleable_windows(mid_window).is_empty());

        // Window just ended - not settleable (need settlement delay)
        assert!(handler.settleable_windows(window_end).is_empty());

        // Just before settlement delay - not settleable
        assert!(handler.settleable_windows(settlement_time - 1).is_empty());

        // At settlement time - settleable
        let settleable = handler.settleable_windows(settlement_time);
        assert_eq!(settleable.len(), 1);
        assert_eq!(settleable[0], window_start);

        // After settlement time - still settleable
        assert!(!handler
            .settleable_windows(settlement_time + 10_000)
            .is_empty());
    }

    // =========================================================================
    // Statistics Tests
    // =========================================================================

    #[test]
    fn test_win_rate_calculation() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        // Win
        let pos1 = make_window_position(0, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(pos1);
        handler.settle_window(0, WindowOutcome::YesWon);

        // Win
        let pos2 = make_window_position(1, Some(dec!(0.35)), Some(dec!(0.60)), dec!(100));
        handler.record_position(pos2);
        handler.settle_window(1, WindowOutcome::NoWon);

        // Loss
        let pos3 = make_window_position(2, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(pos3);
        handler.settle_window(2, WindowOutcome::NoWon);

        // Loss
        let pos4 = make_window_position(3, None, Some(dec!(0.40)), dec!(100));
        handler.record_position(pos4);
        handler.settle_window(3, WindowOutcome::YesWon);

        assert_eq!(handler.settlements_count(), 4);
        assert!((handler.win_rate() - 0.5).abs() < 0.001); // 2 wins out of 4 = 50%
    }

    #[test]
    fn test_win_rate_no_settlements() {
        let handler = SettlementHandler::with_defaults("test-market");
        assert_eq!(handler.win_rate(), 0.0);
    }

    // =========================================================================
    // Cleanup Tests
    // =========================================================================

    #[test]
    fn test_cleanup_stale_settlements() {
        let config = SettlementConfig {
            settlement_delay_ms: 60_000,
            max_pending_age_ms: 30 * 60 * 1000, // 30 minutes
        };
        let mut handler = SettlementHandler::new("test-market", config);

        // Old position (1 hour ago)
        let old_window = 0_i64;
        let pos1 = make_window_position(old_window, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(pos1);

        // Recent position
        let recent_window = 60 * 60 * 1000_i64; // 1 hour
        let pos2 = make_window_position(recent_window, Some(dec!(0.40)), None, dec!(100));
        handler.record_position(pos2);

        assert_eq!(handler.pending_count(), 2);

        // Cleanup with current time 1.5 hours
        let current_time = 90 * 60 * 1000_i64;
        let removed = handler.cleanup_stale(current_time);

        assert_eq!(removed, 1);
        assert_eq!(handler.pending_count(), 1);
        assert!(handler.pending.contains_key(&recent_window));
        assert!(!handler.pending.contains_key(&old_window));
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_settle_nonexistent_window() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        let result = handler.settle_window(999999, WindowOutcome::YesWon);
        assert!(result.is_none());
    }

    #[test]
    fn test_double_settlement_same_window() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        let position = make_window_position(0, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(position);

        // First settlement succeeds
        let result1 = handler.settle_window(0, WindowOutcome::YesWon);
        assert!(result1.is_some());

        // Second settlement fails (position already removed)
        let result2 = handler.settle_window(0, WindowOutcome::YesWon);
        assert!(result2.is_none());

        assert_eq!(handler.settlements_count(), 1);
    }

    #[test]
    fn test_clear_resets_state() {
        let mut handler = SettlementHandler::with_defaults("test-market");

        let position = make_window_position(0, Some(dec!(0.35)), None, dec!(100));
        handler.record_position(position);
        handler.settle_window(0, WindowOutcome::YesWon);

        assert_eq!(handler.settlements_count(), 1);
        assert!(handler.total_realized_pnl() > Decimal::ZERO);

        handler.clear();

        assert_eq!(handler.pending_count(), 0);
        assert_eq!(handler.settlements_count(), 0);
        assert_eq!(handler.total_realized_pnl(), Decimal::ZERO);
        assert!(handler.history().is_empty());
    }

    // =========================================================================
    // WindowOutcome Tests
    // =========================================================================

    #[test]
    fn test_window_outcome_winning_direction() {
        assert_eq!(
            WindowOutcome::YesWon.winning_direction(),
            GabagoolDirection::Yes
        );
        assert_eq!(
            WindowOutcome::NoWon.winning_direction(),
            GabagoolDirection::No
        );
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = SettlementConfig::default();
        assert_eq!(config.settlement_delay_ms, 60_000);
        assert_eq!(config.max_pending_age_ms, 30 * 60 * 1000);
    }
}
