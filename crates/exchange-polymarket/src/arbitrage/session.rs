//! Arbitrage session state and Go/No-Go tracking.
//!
//! This module provides session management for Phase 1 arbitrage validation:
//! - Tracks execution history and statistics
//! - Calculates Wilson CI for fill rate
//! - Provides Go/No-Go recommendations
//! - Supports both paper and live trading modes
//!
//! # Go/No-Go Criteria
//!
//! After MIN_VALIDATION_TRADES (100) executions:
//! - Fill rate Wilson CI lower bound > 80%
//! - Total P&L > 0
//! - Max imbalance <= 50 shares
//!
//! # Recommendations
//!
//! Based on validation results:
//! - `ProceedToPhase2`: Criteria met, ready for larger positions
//! - `ContinuePaper`: Need more data, keep testing
//! - `StopTrading`: Criteria failed, investigate issues

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use super::dual_leg_executor::DualLegResult;
use super::metrics::ArbitrageMetrics;
use super::phase1_config::{Phase1Config, MAX_IMBALANCE, MIN_VALIDATION_TRADES, TARGET_FILL_RATE};

// =============================================================================
// Session Types
// =============================================================================

/// Trading mode for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradingMode {
    /// Paper trading (simulated execution).
    Paper,
    /// Live trading (real orders).
    Live,
}

impl std::fmt::Display for TradingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradingMode::Paper => write!(f, "PAPER"),
            TradingMode::Live => write!(f, "LIVE"),
        }
    }
}

/// Go/No-Go recommendation based on session statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recommendation {
    /// Proceed to Phase 2 with larger positions.
    ProceedToPhase2 {
        /// Reason for proceeding.
        reason: String,
    },
    /// Continue paper trading, need more data.
    ContinuePaper {
        /// Reason for continuing.
        reason: String,
        /// Number of trades needed.
        trades_needed: u32,
    },
    /// Stop trading, investigation needed.
    StopTrading {
        /// Reason for stopping.
        reason: String,
    },
}

impl Recommendation {
    /// Returns true if recommendation is to proceed.
    #[must_use]
    pub fn is_proceed(&self) -> bool {
        matches!(self, Recommendation::ProceedToPhase2 { .. })
    }

    /// Returns true if recommendation is to continue.
    #[must_use]
    pub fn is_continue(&self) -> bool {
        matches!(self, Recommendation::ContinuePaper { .. })
    }

    /// Returns true if recommendation is to stop.
    #[must_use]
    pub fn is_stop(&self) -> bool {
        matches!(self, Recommendation::StopTrading { .. })
    }
}

impl std::fmt::Display for Recommendation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Recommendation::ProceedToPhase2 { reason } => {
                write!(f, "PROCEED TO PHASE 2: {}", reason)
            }
            Recommendation::ContinuePaper {
                reason,
                trades_needed,
            } => {
                write!(
                    f,
                    "CONTINUE PAPER ({} more trades): {}",
                    trades_needed, reason
                )
            }
            Recommendation::StopTrading { reason } => {
                write!(f, "STOP TRADING: {}", reason)
            }
        }
    }
}

/// Record of a single execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Unique execution ID.
    pub id: u64,
    /// Timestamp of execution.
    pub timestamp: DateTime<Utc>,
    /// Market ID.
    pub market_id: String,
    /// Whether execution was successful.
    pub success: bool,
    /// Whether it was a partial fill.
    pub partial: bool,
    /// Shares traded (if successful).
    pub shares: Option<Decimal>,
    /// Total cost (if successful).
    pub cost: Option<Decimal>,
    /// Net profit (if successful).
    pub profit: Option<Decimal>,
    /// Imbalance created.
    pub imbalance: Decimal,
    /// Any error message.
    pub error: Option<String>,
}

// =============================================================================
// Arbitrage Session
// =============================================================================

/// Session state for arbitrage trading.
///
/// Tracks execution history, calculates statistics, and provides
/// Go/No-Go recommendations based on Phase 1 criteria.
#[derive(Debug)]
pub struct ArbitrageSession {
    /// Session ID.
    pub id: uuid::Uuid,
    /// Trading mode.
    pub mode: TradingMode,
    /// Session start time.
    pub started_at: DateTime<Utc>,
    /// Phase 1 configuration (for future extensibility).
    #[allow(dead_code)] // Reserved for config-driven behavior
    config: Phase1Config,
    /// Execution history (bounded to last N executions).
    history: VecDeque<ExecutionRecord>,
    /// Maximum history size.
    max_history: usize,
    /// Next execution ID.
    next_id: u64,
    /// Aggregated metrics.
    metrics: ArbitrageMetrics,
    /// Running profit total.
    total_profit: Decimal,
    /// Running cost total.
    total_cost: Decimal,
    /// Maximum imbalance observed.
    max_imbalance: Decimal,
    /// Current cumulative imbalance.
    current_imbalance: Decimal,
}

impl ArbitrageSession {
    /// Creates a new arbitrage session.
    #[must_use]
    pub fn new(mode: TradingMode) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            mode,
            started_at: Utc::now(),
            config: Phase1Config::new(),
            history: VecDeque::new(),
            max_history: 1000,
            next_id: 1,
            metrics: ArbitrageMetrics::new(),
            total_profit: Decimal::ZERO,
            total_cost: Decimal::ZERO,
            max_imbalance: Decimal::ZERO,
            current_imbalance: Decimal::ZERO,
        }
    }

    /// Creates a new paper trading session.
    #[must_use]
    pub fn paper() -> Self {
        Self::new(TradingMode::Paper)
    }

    /// Creates a new live trading session.
    #[must_use]
    pub fn live() -> Self {
        Self::new(TradingMode::Live)
    }

    /// Records a dual-leg execution result.
    pub fn record_execution(&mut self, market_id: &str, result: &DualLegResult) {
        let id = self.next_id;
        self.next_id += 1;

        let (success, partial, shares, cost, profit, imbalance, error) = match result {
            DualLegResult::Success {
                total_cost,
                net_profit,
                shares,
                ..
            } => {
                self.total_profit += *net_profit;
                self.total_cost += *total_cost;
                (
                    true,
                    false,
                    Some(*shares),
                    Some(*total_cost),
                    Some(*net_profit),
                    Decimal::ZERO,
                    None,
                )
            }
            DualLegResult::YesOnlyFilled { .. } => {
                let imb = result.imbalance();
                self.current_imbalance += imb;
                self.update_max_imbalance();
                (
                    false,
                    true,
                    None,
                    None,
                    None,
                    imb,
                    Some("YES only filled".to_string()),
                )
            }
            DualLegResult::NoOnlyFilled { .. } => {
                let imb = result.imbalance();
                self.current_imbalance += imb;
                self.update_max_imbalance();
                (
                    false,
                    true,
                    None,
                    None,
                    None,
                    imb,
                    Some("NO only filled".to_string()),
                )
            }
            DualLegResult::BothRejected { .. } => (
                false,
                false,
                None,
                None,
                None,
                Decimal::ZERO,
                Some("Both rejected".to_string()),
            ),
            DualLegResult::Error { error } => (
                false,
                false,
                None,
                None,
                None,
                Decimal::ZERO,
                Some(error.clone()),
            ),
        };

        // Record in metrics
        self.metrics.record_execution(success, partial);
        if let Some(imb) = self.history.back().map(|r| r.imbalance) {
            self.metrics.record_imbalance(imb);
        }

        // Create execution record
        let record = ExecutionRecord {
            id,
            timestamp: Utc::now(),
            market_id: market_id.to_string(),
            success,
            partial,
            shares,
            cost,
            profit,
            imbalance,
            error,
        };

        // Add to history (bounded)
        self.history.push_back(record);
        if self.history.len() > self.max_history {
            self.history.pop_front();
        }
    }

    /// Updates the maximum imbalance tracking.
    fn update_max_imbalance(&mut self) {
        let abs_imbalance = self.current_imbalance.abs();
        if abs_imbalance > self.max_imbalance {
            self.max_imbalance = abs_imbalance;
        }
    }

    /// Returns the total number of executions.
    #[must_use]
    pub fn total_executions(&self) -> u32 {
        self.metrics.attempts
    }

    /// Returns the number of successful executions.
    #[must_use]
    pub fn successful_executions(&self) -> u32 {
        self.metrics.successful_pairs
    }

    /// Returns the number of partial fills.
    #[must_use]
    pub fn partial_fills(&self) -> u32 {
        self.metrics.partial_fills
    }

    /// Returns the fill rate.
    #[must_use]
    pub fn fill_rate(&self) -> f64 {
        self.metrics.fill_rate
    }

    /// Returns the Wilson CI for fill rate.
    #[must_use]
    pub fn fill_rate_wilson_ci(&self) -> (f64, f64) {
        self.metrics.fill_rate_wilson_ci
    }

    /// Returns the total profit.
    #[must_use]
    pub fn total_profit(&self) -> Decimal {
        self.total_profit
    }

    /// Returns the total cost.
    #[must_use]
    pub fn total_cost(&self) -> Decimal {
        self.total_cost
    }

    /// Returns the ROI.
    #[must_use]
    pub fn roi(&self) -> Decimal {
        if self.total_cost > Decimal::ZERO {
            self.total_profit / self.total_cost * dec!(100)
        } else {
            Decimal::ZERO
        }
    }

    /// Returns the maximum imbalance observed.
    #[must_use]
    pub fn max_imbalance(&self) -> Decimal {
        self.max_imbalance
    }

    /// Returns the current cumulative imbalance.
    #[must_use]
    pub fn current_imbalance(&self) -> Decimal {
        self.current_imbalance
    }

    /// Returns recent execution history.
    #[must_use]
    pub fn recent_history(&self, count: usize) -> Vec<&ExecutionRecord> {
        self.history.iter().rev().take(count).collect()
    }

    /// Checks if minimum validation trades have been reached.
    #[must_use]
    pub fn has_minimum_trades(&self) -> bool {
        self.total_executions() >= MIN_VALIDATION_TRADES
    }

    /// Checks if fill rate meets the target.
    #[must_use]
    pub fn fill_rate_meets_target(&self) -> bool {
        self.fill_rate_wilson_ci().0 >= TARGET_FILL_RATE
    }

    /// Checks if P&L is positive.
    #[must_use]
    pub fn pnl_positive(&self) -> bool {
        self.total_profit > Decimal::ZERO
    }

    /// Checks if max imbalance is within limits.
    #[must_use]
    pub fn imbalance_acceptable(&self) -> bool {
        self.max_imbalance <= MAX_IMBALANCE
    }

    /// Generates a Go/No-Go recommendation.
    #[must_use]
    pub fn recommendation(&self) -> Recommendation {
        let total = self.total_executions();
        let needed = MIN_VALIDATION_TRADES.saturating_sub(total);

        // Not enough trades yet
        if !self.has_minimum_trades() {
            return Recommendation::ContinuePaper {
                reason: format!(
                    "Only {} of {} required trades completed",
                    total, MIN_VALIDATION_TRADES
                ),
                trades_needed: needed,
            };
        }

        // Check fill rate
        let (ci_lower, _) = self.fill_rate_wilson_ci();
        if ci_lower < TARGET_FILL_RATE {
            return Recommendation::StopTrading {
                reason: format!(
                    "Fill rate CI lower bound {:.1}% below target {:.1}%",
                    ci_lower * 100.0,
                    TARGET_FILL_RATE * 100.0
                ),
            };
        }

        // Check P&L
        if !self.pnl_positive() {
            return Recommendation::StopTrading {
                reason: format!("Total P&L is negative: {}", self.total_profit),
            };
        }

        // Check imbalance
        if !self.imbalance_acceptable() {
            return Recommendation::StopTrading {
                reason: format!(
                    "Max imbalance {} exceeds limit {}",
                    self.max_imbalance, MAX_IMBALANCE
                ),
            };
        }

        // All criteria met!
        Recommendation::ProceedToPhase2 {
            reason: format!(
                "All criteria met: {} trades, {:.1}% fill rate CI, {} profit, {} max imbalance",
                total,
                ci_lower * 100.0,
                self.total_profit,
                self.max_imbalance
            ),
        }
    }

    /// Returns a summary of the session for logging.
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        let (ci_lower, ci_upper) = self.fill_rate_wilson_ci();

        SessionSummary {
            session_id: self.id,
            mode: self.mode,
            started_at: self.started_at,
            duration_secs: (Utc::now() - self.started_at).num_seconds() as u64,
            total_executions: self.total_executions(),
            successful_executions: self.successful_executions(),
            partial_fills: self.partial_fills(),
            fill_rate: self.fill_rate(),
            fill_rate_ci_lower: ci_lower,
            fill_rate_ci_upper: ci_upper,
            total_profit: self.total_profit,
            total_cost: self.total_cost,
            roi: self.roi(),
            max_imbalance: self.max_imbalance,
            current_imbalance: self.current_imbalance,
            recommendation: self.recommendation(),
        }
    }
}

/// Summary of session state for logging/display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session ID.
    pub session_id: uuid::Uuid,
    /// Trading mode.
    pub mode: TradingMode,
    /// Session start time.
    pub started_at: DateTime<Utc>,
    /// Duration in seconds.
    pub duration_secs: u64,
    /// Total executions.
    pub total_executions: u32,
    /// Successful executions.
    pub successful_executions: u32,
    /// Partial fills.
    pub partial_fills: u32,
    /// Fill rate.
    pub fill_rate: f64,
    /// Fill rate CI lower bound.
    pub fill_rate_ci_lower: f64,
    /// Fill rate CI upper bound.
    pub fill_rate_ci_upper: f64,
    /// Total profit.
    pub total_profit: Decimal,
    /// Total cost.
    pub total_cost: Decimal,
    /// ROI percentage.
    pub roi: Decimal,
    /// Maximum imbalance.
    pub max_imbalance: Decimal,
    /// Current imbalance.
    pub current_imbalance: Decimal,
    /// Go/No-Go recommendation.
    pub recommendation: Recommendation,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitrage::execution::OrderResult;

    fn create_success_result(shares: Decimal, cost: Decimal, profit: Decimal) -> DualLegResult {
        DualLegResult::Success {
            yes_result: OrderResult::filled("yes", shares, cost / shares / dec!(2)),
            no_result: OrderResult::filled("no", shares, cost / shares / dec!(2)),
            total_cost: cost,
            net_profit: profit,
            shares,
        }
    }

    fn create_rejected_result() -> DualLegResult {
        DualLegResult::BothRejected {
            yes_result: OrderResult::rejected("yes", "no fill"),
            no_result: OrderResult::rejected("no", "no fill"),
        }
    }

    fn create_partial_yes_result(shares: Decimal, imbalance: Decimal) -> DualLegResult {
        DualLegResult::YesOnlyFilled {
            yes_result: OrderResult::filled("yes", shares, dec!(0.48)),
            no_result: OrderResult::rejected("no", "no fill"),
            unwind_result: Some(super::super::dual_leg_executor::UnwindResult {
                order_result: OrderResult::filled("unwind", shares - imbalance, dec!(0.45)),
                filled_size: shares - imbalance,
                complete: imbalance == Decimal::ZERO,
                slippage: dec!(0.03),
            }),
        }
    }

    // ==================== Session Creation Tests ====================

    #[test]
    fn test_session_new_paper() {
        let session = ArbitrageSession::paper();
        assert_eq!(session.mode, TradingMode::Paper);
        assert_eq!(session.total_executions(), 0);
        assert_eq!(session.total_profit(), Decimal::ZERO);
    }

    #[test]
    fn test_session_new_live() {
        let session = ArbitrageSession::live();
        assert_eq!(session.mode, TradingMode::Live);
    }

    // ==================== Execution Tracking Tests ====================

    #[test]
    fn test_session_tracks_executions() {
        let mut session = ArbitrageSession::paper();

        // Record 5 successful executions
        for i in 0..5 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution(&format!("market-{}", i), &result);
        }

        assert_eq!(session.total_executions(), 5);
        assert_eq!(session.successful_executions(), 5);
        assert_eq!(session.partial_fills(), 0);
        assert_eq!(session.total_profit(), dec!(15)); // 5 * 3
        assert_eq!(session.total_cost(), dec!(480)); // 5 * 96
    }

    #[test]
    fn test_session_tracks_partial_fills() {
        let mut session = ArbitrageSession::paper();

        // Record partial fill
        let result = create_partial_yes_result(dec!(100), dec!(20));
        session.record_execution("market-1", &result);

        assert_eq!(session.total_executions(), 1);
        assert_eq!(session.successful_executions(), 0);
        assert_eq!(session.partial_fills(), 1);
    }

    #[test]
    fn test_session_tracks_rejections() {
        let mut session = ArbitrageSession::paper();

        // Record rejection
        let result = create_rejected_result();
        session.record_execution("market-1", &result);

        assert_eq!(session.total_executions(), 1);
        assert_eq!(session.successful_executions(), 0);
        assert_eq!(session.partial_fills(), 0);
    }

    // ==================== Fill Rate Tests ====================

    #[test]
    fn test_fill_rate_calculation() {
        let mut session = ArbitrageSession::paper();

        // 8 successes, 2 rejections = 80% fill rate
        for _ in 0..8 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }
        for _ in 0..2 {
            let result = create_rejected_result();
            session.record_execution("market", &result);
        }

        assert!((session.fill_rate() - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_fill_rate_wilson_ci() {
        let mut session = ArbitrageSession::paper();

        // 80 successes, 20 rejections
        for _ in 0..80 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }
        for _ in 0..20 {
            let result = create_rejected_result();
            session.record_execution("market", &result);
        }

        let (ci_lower, ci_upper) = session.fill_rate_wilson_ci();
        // With 100 samples at 80%, CI should be roughly (0.71, 0.87)
        assert!(ci_lower > 0.70, "CI lower was {}", ci_lower);
        assert!(ci_upper < 0.90, "CI upper was {}", ci_upper);
    }

    // ==================== Go/No-Go Tests ====================

    #[test]
    fn test_go_no_go_after_100_trades() {
        let mut session = ArbitrageSession::paper();

        // 90 successes, 10 rejections = 90% fill rate
        for _ in 0..90 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }
        for _ in 0..10 {
            let result = create_rejected_result();
            session.record_execution("market", &result);
        }

        assert!(session.has_minimum_trades());
        let rec = session.recommendation();
        assert!(rec.is_proceed(), "Expected proceed, got {:?}", rec);
    }

    #[test]
    fn test_recommendation_proceed_to_phase2() {
        let mut session = ArbitrageSession::paper();

        // All successful trades
        for _ in 0..100 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }

        let rec = session.recommendation();
        assert!(rec.is_proceed());
        match rec {
            Recommendation::ProceedToPhase2 { reason } => {
                assert!(reason.contains("All criteria met"));
            }
            _ => panic!("Expected ProceedToPhase2"),
        }
    }

    #[test]
    fn test_recommendation_continue_paper() {
        let mut session = ArbitrageSession::paper();

        // Only 50 trades
        for _ in 0..50 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }

        let rec = session.recommendation();
        assert!(rec.is_continue());
        match rec {
            Recommendation::ContinuePaper { trades_needed, .. } => {
                assert_eq!(trades_needed, 50);
            }
            _ => panic!("Expected ContinuePaper"),
        }
    }

    #[test]
    fn test_recommendation_stop_low_fill_rate() {
        let mut session = ArbitrageSession::paper();

        // 50% fill rate (too low)
        for _ in 0..50 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }
        for _ in 0..50 {
            let result = create_rejected_result();
            session.record_execution("market", &result);
        }

        let rec = session.recommendation();
        assert!(rec.is_stop(), "Expected stop, got {:?}", rec);
        match rec {
            Recommendation::StopTrading { reason } => {
                assert!(reason.contains("Fill rate"));
            }
            _ => panic!("Expected StopTrading"),
        }
    }

    #[test]
    fn test_recommendation_stop_negative_pnl() {
        let mut session = ArbitrageSession::paper();

        // 100% fill rate but negative profit
        for _ in 0..100 {
            let result = create_success_result(dec!(100), dec!(96), dec!(-1)); // Negative profit
            session.record_execution("market", &result);
        }

        let rec = session.recommendation();
        assert!(rec.is_stop(), "Expected stop, got {:?}", rec);
        match rec {
            Recommendation::StopTrading { reason } => {
                assert!(reason.contains("P&L"));
            }
            _ => panic!("Expected StopTrading"),
        }
    }

    #[test]
    fn test_recommendation_stop_high_imbalance() {
        let mut session = ArbitrageSession::paper();

        // 90 successes
        for _ in 0..90 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }

        // 10 partial fills with high imbalance
        for _ in 0..10 {
            let result = create_partial_yes_result(dec!(100), dec!(60)); // 60 imbalance each
            session.record_execution("market", &result);
        }

        let rec = session.recommendation();
        assert!(rec.is_stop(), "Expected stop, got {:?}", rec);
        match rec {
            Recommendation::StopTrading { reason } => {
                assert!(reason.contains("imbalance"));
            }
            _ => panic!("Expected StopTrading"),
        }
    }

    // ==================== Imbalance Tracking Tests ====================

    #[test]
    fn test_imbalance_tracking() {
        let mut session = ArbitrageSession::paper();

        // Partial fill with 30 imbalance
        let result = create_partial_yes_result(dec!(100), dec!(30));
        session.record_execution("market-1", &result);

        assert_eq!(session.max_imbalance(), dec!(30));
        assert_eq!(session.current_imbalance(), dec!(30));
    }

    #[test]
    fn test_imbalance_accumulates() {
        let mut session = ArbitrageSession::paper();

        // Two partial fills
        let result1 = create_partial_yes_result(dec!(100), dec!(20));
        session.record_execution("market-1", &result1);

        let result2 = create_partial_yes_result(dec!(100), dec!(15));
        session.record_execution("market-2", &result2);

        // Cumulative imbalance = 20 + 15 = 35
        assert_eq!(session.current_imbalance(), dec!(35));
        assert_eq!(session.max_imbalance(), dec!(35));
    }

    // ==================== ROI Tests ====================

    #[test]
    fn test_roi_calculation() {
        let mut session = ArbitrageSession::paper();

        // 10 trades: profit 30, cost 960
        for _ in 0..10 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }

        // ROI = 30 / 960 * 100 = 3.125%
        let roi = session.roi();
        assert!(roi > dec!(3));
        assert!(roi < dec!(4));
    }

    #[test]
    fn test_roi_zero_cost() {
        let session = ArbitrageSession::paper();
        assert_eq!(session.roi(), Decimal::ZERO);
    }

    // ==================== History Tests ====================

    #[test]
    fn test_recent_history() {
        let mut session = ArbitrageSession::paper();

        for i in 0..10 {
            let result = create_success_result(dec!(100), dec!(96), Decimal::from(i));
            session.record_execution(&format!("market-{}", i), &result);
        }

        let recent = session.recent_history(3);
        assert_eq!(recent.len(), 3);
        // Most recent first
        assert_eq!(recent[0].id, 10);
        assert_eq!(recent[1].id, 9);
        assert_eq!(recent[2].id, 8);
    }

    #[test]
    fn test_history_bounded() {
        let mut session = ArbitrageSession::paper();
        session.max_history = 10; // Limit to 10

        // Add 15 executions
        for _ in 0..15 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }

        // Should only keep last 10
        assert_eq!(session.history.len(), 10);
        // First record should be #6 (1-5 dropped)
        assert_eq!(session.history.front().unwrap().id, 6);
    }

    // ==================== Summary Tests ====================

    #[test]
    fn test_session_summary() {
        let mut session = ArbitrageSession::paper();

        for _ in 0..10 {
            let result = create_success_result(dec!(100), dec!(96), dec!(3));
            session.record_execution("market", &result);
        }

        let summary = session.summary();
        assert_eq!(summary.mode, TradingMode::Paper);
        assert_eq!(summary.total_executions, 10);
        assert_eq!(summary.successful_executions, 10);
        assert_eq!(summary.total_profit, dec!(30));
    }

    // ==================== Trading Mode Tests ====================

    #[test]
    fn test_trading_mode_display() {
        assert_eq!(TradingMode::Paper.to_string(), "PAPER");
        assert_eq!(TradingMode::Live.to_string(), "LIVE");
    }

    // ==================== Recommendation Display Tests ====================

    #[test]
    fn test_recommendation_display() {
        let proceed = Recommendation::ProceedToPhase2 {
            reason: "All good".to_string(),
        };
        assert!(proceed.to_string().contains("PROCEED"));

        let continue_rec = Recommendation::ContinuePaper {
            reason: "Need more".to_string(),
            trades_needed: 50,
        };
        assert!(continue_rec.to_string().contains("CONTINUE"));
        assert!(continue_rec.to_string().contains("50"));

        let stop = Recommendation::StopTrading {
            reason: "Failed".to_string(),
        };
        assert!(stop.to_string().contains("STOP"));
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_empty_session_recommendation() {
        let session = ArbitrageSession::paper();
        let rec = session.recommendation();
        assert!(rec.is_continue());
        match rec {
            Recommendation::ContinuePaper { trades_needed, .. } => {
                assert_eq!(trades_needed, 100);
            }
            _ => panic!("Expected ContinuePaper"),
        }
    }

    #[test]
    fn test_error_result_tracking() {
        let mut session = ArbitrageSession::paper();

        let result = DualLegResult::Error {
            error: "Network error".to_string(),
        };
        session.record_execution("market-1", &result);

        assert_eq!(session.total_executions(), 1);
        assert_eq!(session.successful_executions(), 0);

        let history = session.recent_history(1);
        assert!(history[0].error.is_some());
        assert!(history[0].error.as_ref().unwrap().contains("Network"));
    }
}
