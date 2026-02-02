//! Settlement reconciliation for cross-exchange arbitrage positions.
//!
//! This module tracks open positions across Kalshi and Polymarket, monitors
//! settlements, and reconciles P&L.
//!
//! # Overview
//!
//! The reconciler:
//! 1. Tracks all open cross-exchange positions
//! 2. Monitors settlement events on both exchanges
//! 3. Reconciles settled positions and calculates actual P&L
//! 4. Provides reporting on historical performance
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_arbitrage_cross::reconciler::{SettlementReconciler, CrossPosition};
//!
//! let mut reconciler = SettlementReconciler::new();
//!
//! // Add position from successful execution
//! reconciler.add_position(position);
//!
//! // Check for settlements
//! let events = reconciler.check_settlements();
//!
//! // Get P&L summary
//! println!("Total P&L: ${}", reconciler.total_pnl());
//! ```

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::executor::CrossPosition;
use crate::types::{Exchange, Side};

// =============================================================================
// Settlement Events
// =============================================================================

/// Outcome of a market settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementOutcome {
    /// YES outcome - price was above threshold.
    Yes,
    /// NO outcome - price was below threshold.
    No,
    /// Push/Void - market was invalidated.
    Void,
}

impl SettlementOutcome {
    /// Returns the winning side for this outcome.
    #[must_use]
    pub fn winning_side(self) -> Option<Side> {
        match self {
            Self::Yes => Some(Side::Yes),
            Self::No => Some(Side::No),
            Self::Void => None,
        }
    }

    /// Returns true if this side wins.
    #[must_use]
    pub fn side_wins(self, side: Side) -> bool {
        match self {
            Self::Yes => side == Side::Yes,
            Self::No => side == Side::No,
            Self::Void => false,
        }
    }

    /// Returns the display string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "YES",
            Self::No => "NO",
            Self::Void => "VOID",
        }
    }
}

impl std::fmt::Display for SettlementOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A settlement event from an exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementEvent {
    /// Position ID that was settled.
    pub position_id: Uuid,

    /// Exchange that reported this settlement.
    pub exchange: Exchange,

    /// The settlement outcome.
    pub outcome: SettlementOutcome,

    /// Payout amount in dollars.
    pub payout: Decimal,

    /// When the settlement occurred.
    pub settled_at: DateTime<Utc>,

    /// Profit/loss for this leg.
    pub leg_pnl: Decimal,

    /// Transaction ID or reference from exchange (if available).
    pub reference: Option<String>,
}

impl SettlementEvent {
    /// Creates a new settlement event.
    #[must_use]
    pub fn new(
        position_id: Uuid,
        exchange: Exchange,
        outcome: SettlementOutcome,
        payout: Decimal,
        leg_pnl: Decimal,
    ) -> Self {
        Self {
            position_id,
            exchange,
            outcome,
            payout,
            settled_at: Utc::now(),
            leg_pnl,
            reference: None,
        }
    }

    /// Adds a reference to the event.
    #[must_use]
    pub fn with_reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }
}

// =============================================================================
// Reconciliation Results
// =============================================================================

/// Result of reconciling a position's settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationResult {
    /// Position ID.
    pub position_id: Uuid,

    /// Kalshi settlement outcome.
    pub kalshi_outcome: Option<SettlementOutcome>,

    /// Polymarket settlement outcome.
    pub polymarket_outcome: Option<SettlementOutcome>,

    /// Whether both exchanges agreed on outcome.
    pub outcomes_match: bool,

    /// Total payout received.
    pub total_payout: Decimal,

    /// Original cost.
    pub original_cost: Decimal,

    /// Actual profit/loss.
    pub actual_pnl: Decimal,

    /// Expected profit (at time of entry).
    pub expected_pnl: Decimal,

    /// Whether actual matched expected.
    pub met_expectations: bool,
}

impl ReconciliationResult {
    /// Returns the payout vs expectation difference.
    #[must_use]
    pub fn pnl_variance(&self) -> Decimal {
        self.actual_pnl - self.expected_pnl
    }

    /// Returns true if this was profitable.
    #[must_use]
    pub fn is_profitable(&self) -> bool {
        self.actual_pnl > Decimal::ZERO
    }
}

// =============================================================================
// Position Status
// =============================================================================

/// Detailed status of a position's legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionStatus {
    /// Both legs are open, awaiting settlement.
    Open,

    /// Kalshi leg has settled, awaiting Polymarket.
    KalshiSettled,

    /// Polymarket leg has settled, awaiting Kalshi.
    PolymarketSettled,

    /// Both legs have settled.
    FullySettled,

    /// Position has an error.
    Error,
}

impl PositionStatus {
    /// Returns true if fully settled.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::FullySettled)
    }

    /// Returns true if partially settled.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        matches!(self, Self::KalshiSettled | Self::PolymarketSettled)
    }
}

// =============================================================================
// Tracked Position
// =============================================================================

/// A position being tracked by the reconciler with settlement state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedPosition {
    /// The original position from execution.
    pub position: CrossPosition,

    /// Settlement status.
    pub status: PositionStatus,

    /// Kalshi settlement event (if received).
    pub kalshi_settlement: Option<SettlementEvent>,

    /// Polymarket settlement event (if received).
    pub polymarket_settlement: Option<SettlementEvent>,

    /// Reconciliation result (if both legs settled).
    pub reconciliation: Option<ReconciliationResult>,

    /// Any errors encountered.
    pub errors: Vec<String>,

    /// Last check timestamp.
    pub last_checked: DateTime<Utc>,
}

impl TrackedPosition {
    /// Creates a new tracked position.
    #[must_use]
    pub fn new(position: CrossPosition) -> Self {
        Self {
            position,
            status: PositionStatus::Open,
            kalshi_settlement: None,
            polymarket_settlement: None,
            reconciliation: None,
            errors: Vec::new(),
            last_checked: Utc::now(),
        }
    }

    /// Records a Kalshi settlement.
    pub fn record_kalshi_settlement(&mut self, event: SettlementEvent) {
        self.kalshi_settlement = Some(event);
        self.update_status();
        self.last_checked = Utc::now();
    }

    /// Records a Polymarket settlement.
    pub fn record_polymarket_settlement(&mut self, event: SettlementEvent) {
        self.polymarket_settlement = Some(event);
        self.update_status();
        self.last_checked = Utc::now();
    }

    /// Updates the status based on settlements.
    fn update_status(&mut self) {
        self.status = match (&self.kalshi_settlement, &self.polymarket_settlement) {
            (Some(_), Some(_)) => PositionStatus::FullySettled,
            (Some(_), None) => PositionStatus::KalshiSettled,
            (None, Some(_)) => PositionStatus::PolymarketSettled,
            (None, None) => PositionStatus::Open,
        };

        // If fully settled, reconcile
        if self.status == PositionStatus::FullySettled {
            self.reconcile();
        }
    }

    /// Reconciles the position after both legs settle.
    fn reconcile(&mut self) {
        let kalshi = self.kalshi_settlement.as_ref();
        let poly = self.polymarket_settlement.as_ref();

        if let (Some(k), Some(p)) = (kalshi, poly) {
            let outcomes_match = k.outcome == p.outcome;
            let total_payout = k.payout + p.payout;
            let actual_pnl = total_payout - self.position.total_cost;

            // For arbitrage, we should always get paid from one side
            // The total payout should be close to $1 per share
            let _expected_payout = self.position.balanced_quantity();
            let met_expectations = (actual_pnl - self.position.expected_profit).abs() < dec!(0.01);

            self.reconciliation = Some(ReconciliationResult {
                position_id: self.position.id,
                kalshi_outcome: Some(k.outcome),
                polymarket_outcome: Some(p.outcome),
                outcomes_match,
                total_payout,
                original_cost: self.position.total_cost,
                actual_pnl,
                expected_pnl: self.position.expected_profit,
                met_expectations,
            });

            if !outcomes_match {
                self.errors.push(format!(
                    "Settlement mismatch: Kalshi={}, Polymarket={}",
                    k.outcome, p.outcome
                ));
            }

            info!(
                position_id = %self.position.id,
                actual_pnl = %actual_pnl,
                expected_pnl = %self.position.expected_profit,
                outcomes_match = outcomes_match,
                "Position reconciled"
            );
        }
    }

    /// Returns the total PnL (if reconciled).
    #[must_use]
    pub fn pnl(&self) -> Option<Decimal> {
        self.reconciliation.as_ref().map(|r| r.actual_pnl)
    }

    /// Returns true if settlement time has passed.
    #[must_use]
    pub fn settlement_time_passed(&self) -> bool {
        Utc::now() > self.position.matched_market.settlement_time
    }

    /// Returns time since settlement (if passed).
    #[must_use]
    pub fn time_since_settlement(&self) -> Option<chrono::Duration> {
        if self.settlement_time_passed() {
            Some(Utc::now() - self.position.matched_market.settlement_time)
        } else {
            None
        }
    }
}

// =============================================================================
// Settlement Reconciler
// =============================================================================

/// Tracks and reconciles cross-exchange arbitrage positions.
pub struct SettlementReconciler {
    /// Open positions being tracked.
    positions: RwLock<HashMap<Uuid, TrackedPosition>>,

    /// Settled positions (for history).
    settled: RwLock<Vec<TrackedPosition>>,

    /// Total realized P&L.
    total_pnl: RwLock<Decimal>,

    /// Number of positions reconciled.
    reconciled_count: RwLock<u32>,
}

impl SettlementReconciler {
    /// Creates a new settlement reconciler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            positions: RwLock::new(HashMap::new()),
            settled: RwLock::new(Vec::new()),
            total_pnl: RwLock::new(Decimal::ZERO),
            reconciled_count: RwLock::new(0),
        }
    }

    /// Adds a position to track.
    pub fn add_position(&self, position: CrossPosition) {
        let id = position.id;
        let tracked = TrackedPosition::new(position);

        info!(
            position_id = %id,
            kalshi_ticker = %tracked.position.matched_market.kalshi_ticker,
            settlement_time = %tracked.position.matched_market.settlement_time,
            "Tracking new position for settlement"
        );

        self.positions.write().insert(id, tracked);
    }

    /// Returns a position by ID.
    #[must_use]
    pub fn get_position(&self, id: Uuid) -> Option<TrackedPosition> {
        self.positions.read().get(&id).cloned()
    }

    /// Records a settlement event.
    pub fn record_settlement(&self, event: SettlementEvent) {
        let position_id = event.position_id;
        let mut positions = self.positions.write();

        // Track whether we need to move to settled and collect info
        let settle_info = if let Some(pos) = positions.get_mut(&position_id) {
            match event.exchange {
                Exchange::Kalshi => {
                    info!(
                        position_id = %position_id,
                        outcome = %event.outcome,
                        payout = %event.payout,
                        "Recording Kalshi settlement"
                    );
                    pos.record_kalshi_settlement(event);
                }
                Exchange::Polymarket => {
                    info!(
                        position_id = %position_id,
                        outcome = %event.outcome,
                        payout = %event.payout,
                        "Recording Polymarket settlement"
                    );
                    pos.record_polymarket_settlement(event);
                }
            }

            // Check if fully settled and collect pnl
            if pos.status.is_settled() {
                Some((position_id, pos.pnl()))
            } else {
                None
            }
        } else {
            warn!(
                position_id = %position_id,
                "Settlement event for unknown position"
            );
            None
        };

        // If fully settled, move to settled history (outside of the borrow)
        if let Some((id, pnl_opt)) = settle_info {
            if let Some(pnl) = pnl_opt {
                *self.total_pnl.write() += pnl;
                *self.reconciled_count.write() += 1;
            }

            if let Some(pos) = positions.remove(&id) {
                self.settled.write().push(pos);
            }
        }
    }

    /// Checks for settlements that need to be processed.
    ///
    /// In production, this would:
    /// 1. Query Kalshi for settled positions
    /// 2. Query Polymarket for settled positions
    /// 3. Record events
    ///
    /// For now, returns positions that are past settlement time but not yet settled.
    #[must_use]
    pub fn check_settlements(&self) -> Vec<SettlementEvent> {
        let positions = self.positions.read();
        let events = Vec::new();

        for (id, pos) in positions.iter() {
            if pos.settlement_time_passed() && pos.status == PositionStatus::Open {
                debug!(
                    position_id = %id,
                    settlement_time = %pos.position.matched_market.settlement_time,
                    "Position past settlement time - needs settlement check"
                );
                // In production, would query exchanges here
            }
        }

        events
    }

    /// Returns all open positions.
    #[must_use]
    pub fn open_positions(&self) -> Vec<TrackedPosition> {
        self.positions.read().values().cloned().collect()
    }

    /// Returns all positions past settlement time but not yet settled.
    #[must_use]
    pub fn pending_settlements(&self) -> Vec<TrackedPosition> {
        self.positions
            .read()
            .values()
            .filter(|p| p.settlement_time_passed() && !p.status.is_settled())
            .cloned()
            .collect()
    }

    /// Returns all settled positions.
    #[must_use]
    pub fn settled_positions(&self) -> Vec<TrackedPosition> {
        self.settled.read().clone()
    }

    /// Returns the total realized P&L.
    #[must_use]
    pub fn total_pnl(&self) -> Decimal {
        *self.total_pnl.read()
    }

    /// Returns the number of positions reconciled.
    #[must_use]
    pub fn reconciled_count(&self) -> u32 {
        *self.reconciled_count.read()
    }

    /// Returns the average P&L per position.
    #[must_use]
    pub fn average_pnl(&self) -> Decimal {
        let count = *self.reconciled_count.read();
        if count == 0 {
            return Decimal::ZERO;
        }
        *self.total_pnl.read() / Decimal::from(count)
    }

    /// Returns the win rate (profitable positions / total).
    #[must_use]
    pub fn win_rate(&self) -> f64 {
        let settled = self.settled.read();
        if settled.is_empty() {
            return 0.0;
        }

        let wins = settled
            .iter()
            .filter(|p| p.reconciliation.as_ref().map_or(false, |r| r.is_profitable()))
            .count();

        wins as f64 / settled.len() as f64
    }

    /// Returns summary statistics.
    #[must_use]
    pub fn summary(&self) -> ReconcilerSummary {
        let positions = self.positions.read();
        let settled = self.settled.read();

        ReconcilerSummary {
            open_positions: positions.len() as u32,
            pending_settlements: positions
                .values()
                .filter(|p| p.settlement_time_passed() && !p.status.is_settled())
                .count() as u32,
            reconciled_positions: *self.reconciled_count.read(),
            total_pnl: *self.total_pnl.read(),
            average_pnl: self.average_pnl(),
            win_rate: self.win_rate(),
            total_cost: settled.iter().map(|p| p.position.total_cost).sum(),
            total_payout: settled
                .iter()
                .filter_map(|p| p.reconciliation.as_ref())
                .map(|r| r.total_payout)
                .sum(),
        }
    }

    /// Removes a position (for cleanup or cancellation).
    pub fn remove_position(&self, id: Uuid) -> Option<TrackedPosition> {
        self.positions.write().remove(&id)
    }

    /// Clears all settled positions (for memory management).
    pub fn clear_settled_history(&self) {
        self.settled.write().clear();
    }
}

impl Default for SettlementReconciler {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Summary Types
// =============================================================================

/// Summary statistics for the reconciler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcilerSummary {
    /// Number of open positions.
    pub open_positions: u32,

    /// Number of positions pending settlement.
    pub pending_settlements: u32,

    /// Number of positions fully reconciled.
    pub reconciled_positions: u32,

    /// Total realized P&L.
    pub total_pnl: Decimal,

    /// Average P&L per position.
    pub average_pnl: Decimal,

    /// Win rate (0.0 to 1.0).
    pub win_rate: f64,

    /// Total cost of all settled positions.
    pub total_cost: Decimal,

    /// Total payout received.
    pub total_payout: Decimal,
}

impl ReconcilerSummary {
    /// Returns the ROI as a percentage.
    #[must_use]
    pub fn roi_pct(&self) -> Decimal {
        if self.total_cost == Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.total_pnl / self.total_cost * dec!(100)
    }
}

// =============================================================================
// Position Iterator (for batch processing)
// =============================================================================

/// Iterator over positions needing settlement checks.
pub struct SettlementCheckIterator<'a> {
    positions: std::vec::IntoIter<&'a TrackedPosition>,
}

impl<'a> Iterator for SettlementCheckIterator<'a> {
    type Item = &'a TrackedPosition;

    fn next(&mut self) -> Option<Self::Item> {
        self.positions.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::CrossPositionStatus as ExecPositionStatus;
    use crate::types::MatchedMarket;

    // ==================== Settlement Outcome Tests ====================

    #[test]
    fn test_settlement_outcome_winning_side() {
        assert_eq!(SettlementOutcome::Yes.winning_side(), Some(Side::Yes));
        assert_eq!(SettlementOutcome::No.winning_side(), Some(Side::No));
        assert_eq!(SettlementOutcome::Void.winning_side(), None);
    }

    #[test]
    fn test_settlement_outcome_side_wins() {
        assert!(SettlementOutcome::Yes.side_wins(Side::Yes));
        assert!(!SettlementOutcome::Yes.side_wins(Side::No));
        assert!(SettlementOutcome::No.side_wins(Side::No));
        assert!(!SettlementOutcome::Void.side_wins(Side::Yes));
    }

    #[test]
    fn test_settlement_outcome_display() {
        assert_eq!(format!("{}", SettlementOutcome::Yes), "YES");
        assert_eq!(format!("{}", SettlementOutcome::No), "NO");
        assert_eq!(format!("{}", SettlementOutcome::Void), "VOID");
    }

    // ==================== Settlement Event Tests ====================

    #[test]
    fn test_settlement_event_creation() {
        let event = SettlementEvent::new(
            Uuid::new_v4(),
            Exchange::Kalshi,
            SettlementOutcome::Yes,
            dec!(100),
            dec!(5),
        );

        assert_eq!(event.exchange, Exchange::Kalshi);
        assert_eq!(event.outcome, SettlementOutcome::Yes);
        assert_eq!(event.payout, dec!(100));
        assert_eq!(event.leg_pnl, dec!(5));
    }

    #[test]
    fn test_settlement_event_with_reference() {
        let event = SettlementEvent::new(
            Uuid::new_v4(),
            Exchange::Polymarket,
            SettlementOutcome::No,
            dec!(50),
            dec!(2),
        )
        .with_reference("tx-12345");

        assert_eq!(event.reference, Some("tx-12345".to_string()));
    }

    // ==================== Position Status Tests ====================

    #[test]
    fn test_position_status_is_settled() {
        assert!(PositionStatus::FullySettled.is_settled());
        assert!(!PositionStatus::Open.is_settled());
        assert!(!PositionStatus::KalshiSettled.is_settled());
    }

    #[test]
    fn test_position_status_is_partial() {
        assert!(PositionStatus::KalshiSettled.is_partial());
        assert!(PositionStatus::PolymarketSettled.is_partial());
        assert!(!PositionStatus::Open.is_partial());
        assert!(!PositionStatus::FullySettled.is_partial());
    }

    // ==================== Tracked Position Tests ====================

    #[test]
    fn test_tracked_position_creation() {
        let position = create_test_position();
        let tracked = TrackedPosition::new(position.clone());

        assert_eq!(tracked.status, PositionStatus::Open);
        assert!(tracked.kalshi_settlement.is_none());
        assert!(tracked.polymarket_settlement.is_none());
        assert!(tracked.reconciliation.is_none());
    }

    #[test]
    fn test_tracked_position_kalshi_settlement() {
        let position = create_test_position();
        let mut tracked = TrackedPosition::new(position.clone());

        let event = SettlementEvent::new(
            position.id,
            Exchange::Kalshi,
            SettlementOutcome::Yes,
            dec!(100),
            dec!(55), // Paid $45, received $100
        );

        tracked.record_kalshi_settlement(event);

        assert_eq!(tracked.status, PositionStatus::KalshiSettled);
        assert!(tracked.kalshi_settlement.is_some());
    }

    #[test]
    fn test_tracked_position_full_settlement() {
        let position = create_test_position();
        let mut tracked = TrackedPosition::new(position.clone());

        // Kalshi YES wins
        let kalshi_event = SettlementEvent::new(
            position.id,
            Exchange::Kalshi,
            SettlementOutcome::Yes,
            dec!(100), // 100 shares * $1 payout
            dec!(55),
        );
        tracked.record_kalshi_settlement(kalshi_event);

        // Polymarket NO loses
        let poly_event = SettlementEvent::new(
            position.id,
            Exchange::Polymarket,
            SettlementOutcome::Yes, // Same outcome
            dec!(0),              // NO shares worthless
            dec!(-50),
        );
        tracked.record_polymarket_settlement(poly_event);

        assert_eq!(tracked.status, PositionStatus::FullySettled);
        assert!(tracked.reconciliation.is_some());

        let recon = tracked.reconciliation.as_ref().unwrap();
        assert!(recon.outcomes_match);
        assert_eq!(recon.total_payout, dec!(100));
    }

    // ==================== Reconciler Tests ====================

    #[test]
    fn test_reconciler_add_position() {
        let reconciler = SettlementReconciler::new();
        let position = create_test_position();
        let id = position.id;

        reconciler.add_position(position);

        assert!(reconciler.get_position(id).is_some());
        assert_eq!(reconciler.open_positions().len(), 1);
    }

    #[test]
    fn test_reconciler_record_settlement() {
        let reconciler = SettlementReconciler::new();
        let position = create_test_position();
        let id = position.id;

        reconciler.add_position(position);

        // Record Kalshi settlement
        let event = SettlementEvent::new(id, Exchange::Kalshi, SettlementOutcome::Yes, dec!(100), dec!(55));
        reconciler.record_settlement(event);

        let tracked = reconciler.get_position(id).unwrap();
        assert_eq!(tracked.status, PositionStatus::KalshiSettled);
    }

    #[test]
    fn test_reconciler_full_reconciliation() {
        let reconciler = SettlementReconciler::new();
        let position = create_test_position();
        let id = position.id;

        reconciler.add_position(position);

        // Record both settlements
        reconciler.record_settlement(SettlementEvent::new(
            id,
            Exchange::Kalshi,
            SettlementOutcome::Yes,
            dec!(100),
            dec!(55),
        ));

        reconciler.record_settlement(SettlementEvent::new(
            id,
            Exchange::Polymarket,
            SettlementOutcome::Yes,
            dec!(0),
            dec!(-50),
        ));

        // Position should be moved to settled
        assert!(reconciler.get_position(id).is_none());
        assert_eq!(reconciler.settled_positions().len(), 1);
        assert_eq!(reconciler.reconciled_count(), 1);
    }

    #[test]
    fn test_reconciler_total_pnl() {
        let reconciler = SettlementReconciler::new();

        // Add two positions
        let pos1 = create_test_position();
        let pos2 = create_test_position();
        let id1 = pos1.id;
        let id2 = pos2.id;

        reconciler.add_position(pos1);
        reconciler.add_position(pos2);

        // Settle both with profit
        for id in [id1, id2] {
            reconciler.record_settlement(SettlementEvent::new(
                id,
                Exchange::Kalshi,
                SettlementOutcome::Yes,
                dec!(100),
                dec!(55),
            ));
            reconciler.record_settlement(SettlementEvent::new(
                id,
                Exchange::Polymarket,
                SettlementOutcome::Yes,
                dec!(0),
                dec!(-50),
            ));
        }

        // Each position: payout $100, cost $95, pnl $5
        assert_eq!(reconciler.reconciled_count(), 2);
        assert_eq!(reconciler.total_pnl(), dec!(10));
        assert_eq!(reconciler.average_pnl(), dec!(5));
    }

    #[test]
    fn test_reconciler_summary() {
        let reconciler = SettlementReconciler::new();
        let position = create_test_position();

        reconciler.add_position(position);

        let summary = reconciler.summary();

        assert_eq!(summary.open_positions, 1);
        assert_eq!(summary.reconciled_positions, 0);
    }

    #[test]
    fn test_reconciler_remove_position() {
        let reconciler = SettlementReconciler::new();
        let position = create_test_position();
        let id = position.id;

        reconciler.add_position(position);
        let removed = reconciler.remove_position(id);

        assert!(removed.is_some());
        assert!(reconciler.get_position(id).is_none());
    }

    // ==================== Reconciliation Result Tests ====================

    #[test]
    fn test_reconciliation_result_profitable() {
        let result = ReconciliationResult {
            position_id: Uuid::new_v4(),
            kalshi_outcome: Some(SettlementOutcome::Yes),
            polymarket_outcome: Some(SettlementOutcome::Yes),
            outcomes_match: true,
            total_payout: dec!(100),
            original_cost: dec!(95),
            actual_pnl: dec!(5),
            expected_pnl: dec!(5),
            met_expectations: true,
        };

        assert!(result.is_profitable());
        assert_eq!(result.pnl_variance(), Decimal::ZERO);
    }

    #[test]
    fn test_reconciliation_result_pnl_variance() {
        let result = ReconciliationResult {
            position_id: Uuid::new_v4(),
            kalshi_outcome: Some(SettlementOutcome::Yes),
            polymarket_outcome: Some(SettlementOutcome::Yes),
            outcomes_match: true,
            total_payout: dec!(100),
            original_cost: dec!(95),
            actual_pnl: dec!(3), // Less than expected
            expected_pnl: dec!(5),
            met_expectations: false,
        };

        assert!(result.is_profitable());
        assert_eq!(result.pnl_variance(), dec!(-2));
    }

    // ==================== Summary Tests ====================

    #[test]
    fn test_summary_roi() {
        let summary = ReconcilerSummary {
            open_positions: 0,
            pending_settlements: 0,
            reconciled_positions: 10,
            total_pnl: dec!(50),
            average_pnl: dec!(5),
            win_rate: 0.8,
            total_cost: dec!(950),
            total_payout: dec!(1000),
        };

        // ROI = 50 / 950 * 100 ≈ 5.26%
        let roi = summary.roi_pct();
        assert!(roi > dec!(5) && roi < dec!(6));
    }

    #[test]
    fn test_summary_roi_zero_cost() {
        let summary = ReconcilerSummary {
            open_positions: 0,
            pending_settlements: 0,
            reconciled_positions: 0,
            total_pnl: Decimal::ZERO,
            average_pnl: Decimal::ZERO,
            win_rate: 0.0,
            total_cost: Decimal::ZERO,
            total_payout: Decimal::ZERO,
        };

        assert_eq!(summary.roi_pct(), Decimal::ZERO);
    }

    // ==================== Helper Functions ====================

    fn create_test_position() -> CrossPosition {
        CrossPosition {
            id: Uuid::new_v4(),
            matched_market: MatchedMarket::new(
                "KXBTC-TEST".to_string(),
                "0xtest".to_string(),
                "yes-token".to_string(),
                "no-token".to_string(),
                "BTC".to_string(),
                dec!(100000),
                Utc::now() + chrono::Duration::hours(1),
                0.95,
            ),
            kalshi_order_id: "kalshi-123".to_string(),
            kalshi_side: Side::Yes,
            kalshi_filled: dec!(100),
            kalshi_price: dec!(45),
            polymarket_order_id: "poly-456".to_string(),
            polymarket_side: Side::No,
            polymarket_filled: dec!(100),
            polymarket_price: dec!(0.50),
            total_cost: dec!(95),
            expected_profit: dec!(5),
            status: ExecPositionStatus::Open,
            created_at: Utc::now(),
            settled_at: None,
            actual_profit: None,
        }
    }
}
