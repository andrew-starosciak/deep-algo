//! Dual-leg execution for Phase 1 arbitrage.
//!
//! This module provides simultaneous YES+NO order execution with atomic
//! success/failure semantics. If only one leg fills, it triggers an
//! automatic unwind attempt to minimize exposure.
//!
//! # Execution Flow
//!
//! 1. Submit YES and NO orders simultaneously
//! 2. Wait for both to reach terminal state
//! 3. If both fill: SUCCESS - position is balanced
//! 4. If one fills: PARTIAL - attempt unwind
//! 5. If neither fills: REJECTED - no exposure
//!
//! # Unwind Strategy
//!
//! When only one leg fills, we attempt to close the position immediately
//! using a FAK (Fill-and-Kill) order at market. This minimizes holding time
//! and directional exposure.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::time::Duration;

use super::execution::{
    OrderParams, OrderResult, OrderStatus, OrderType, PolymarketExecutor, Side,
};
use super::phase1_config::Phase1Config;
use super::types::ArbitrageOpportunity;

// =============================================================================
// Execution Result
// =============================================================================

/// Result of a dual-leg execution attempt.
#[derive(Debug, Clone)]
pub enum DualLegResult {
    /// Both legs filled successfully.
    Success {
        /// YES order result.
        yes_result: OrderResult,
        /// NO order result.
        no_result: OrderResult,
        /// Total cost paid.
        total_cost: Decimal,
        /// Net profit (guaranteed at settlement).
        net_profit: Decimal,
        /// Shares acquired (minimum of YES and NO).
        shares: Decimal,
    },

    /// Only YES leg filled, NO rejected.
    YesOnlyFilled {
        /// YES order result.
        yes_result: OrderResult,
        /// NO order result (rejected/expired).
        no_result: OrderResult,
        /// Unwind attempt result (if any).
        unwind_result: Option<UnwindResult>,
    },

    /// Only NO leg filled, YES rejected.
    NoOnlyFilled {
        /// YES order result (rejected/expired).
        yes_result: OrderResult,
        /// NO order result.
        no_result: OrderResult,
        /// Unwind attempt result (if any).
        unwind_result: Option<UnwindResult>,
    },

    /// Both legs rejected, no exposure.
    BothRejected {
        /// YES order result.
        yes_result: OrderResult,
        /// NO order result.
        no_result: OrderResult,
    },

    /// Execution failed with error.
    Error {
        /// Error that occurred.
        error: String,
    },
}

impl DualLegResult {
    /// Returns true if execution was successful (both legs filled).
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, DualLegResult::Success { .. })
    }

    /// Returns true if there was a partial fill (exposure created).
    #[must_use]
    pub fn is_partial(&self) -> bool {
        matches!(
            self,
            DualLegResult::YesOnlyFilled { .. } | DualLegResult::NoOnlyFilled { .. }
        )
    }

    /// Returns true if both legs were rejected (no exposure).
    #[must_use]
    pub fn is_rejected(&self) -> bool {
        matches!(self, DualLegResult::BothRejected { .. })
    }

    /// Returns the total cost if successful.
    #[must_use]
    pub fn total_cost(&self) -> Option<Decimal> {
        match self {
            DualLegResult::Success { total_cost, .. } => Some(*total_cost),
            _ => None,
        }
    }

    /// Returns the net profit if successful.
    #[must_use]
    pub fn net_profit(&self) -> Option<Decimal> {
        match self {
            DualLegResult::Success { net_profit, .. } => Some(*net_profit),
            _ => None,
        }
    }

    /// Returns the imbalance (exposure) from this execution.
    #[must_use]
    pub fn imbalance(&self) -> Decimal {
        match self {
            DualLegResult::Success { .. } => Decimal::ZERO,
            DualLegResult::YesOnlyFilled {
                yes_result,
                unwind_result,
                ..
            } => {
                let filled = yes_result.filled_size;
                let unwound = unwind_result
                    .as_ref()
                    .map(|u| u.filled_size)
                    .unwrap_or(Decimal::ZERO);
                filled - unwound
            }
            DualLegResult::NoOnlyFilled {
                no_result,
                unwind_result,
                ..
            } => {
                let filled = no_result.filled_size;
                let unwound = unwind_result
                    .as_ref()
                    .map(|u| u.filled_size)
                    .unwrap_or(Decimal::ZERO);
                // NO imbalance is negative (more NO than YES)
                -(filled - unwound)
            }
            DualLegResult::BothRejected { .. } => Decimal::ZERO,
            DualLegResult::Error { .. } => Decimal::ZERO,
        }
    }
}

/// Result of an unwind attempt.
#[derive(Debug, Clone)]
pub struct UnwindResult {
    /// Order result from unwind attempt.
    pub order_result: OrderResult,
    /// Amount successfully unwound.
    pub filled_size: Decimal,
    /// Whether unwind was complete.
    pub complete: bool,
    /// Slippage from original price (if any).
    pub slippage: Decimal,
}

// =============================================================================
// Dual Leg Executor
// =============================================================================

/// Executor for dual-leg arbitrage positions.
///
/// Handles simultaneous YES+NO execution with automatic unwind on partial fills.
pub struct DualLegExecutor<E: PolymarketExecutor> {
    /// Underlying executor.
    executor: E,
    /// Phase 1 configuration (for validation).
    #[allow(dead_code)] // Used for validation in future enhancements
    config: Phase1Config,
    /// Timeout for order execution.
    #[allow(dead_code)] // Reserved for future timeout implementation
    timeout: Duration,
}

impl<E: PolymarketExecutor> DualLegExecutor<E> {
    /// Creates a new dual-leg executor.
    #[must_use]
    pub fn new(executor: E) -> Self {
        Self {
            executor,
            config: Phase1Config::new(),
            timeout: Duration::from_secs(3),
        }
    }

    /// Creates a new dual-leg executor with custom timeout.
    #[must_use]
    pub fn with_timeout(executor: E, timeout: Duration) -> Self {
        Self {
            executor,
            config: Phase1Config::new(),
            timeout,
        }
    }

    /// Returns a reference to the inner executor.
    ///
    /// Useful for accessing executor-specific methods like circuit breaker checks.
    #[must_use]
    pub fn executor(&self) -> &E {
        &self.executor
    }

    /// Executes a dual-leg arbitrage trade.
    ///
    /// Submits YES and NO orders simultaneously, then handles the results:
    /// - Both fill: Return success
    /// - One fills: Attempt unwind, return partial
    /// - Neither fills: Return rejected
    pub async fn execute(
        &self,
        opportunity: &ArbitrageOpportunity,
        shares: Decimal,
    ) -> DualLegResult {
        // Validate opportunity first
        let validation = self.config.validate_opportunity(opportunity);
        if !validation.is_valid() {
            return DualLegResult::Error {
                error: format!("Opportunity validation failed: {:?}", validation),
            };
        }

        // Calculate position value and validate
        let yes_cost = shares * opportunity.yes_worst_fill;
        let no_cost = shares * opportunity.no_worst_fill;
        let total_cost = yes_cost + no_cost;

        if total_cost > self.config.max_position_value() {
            return DualLegResult::Error {
                error: format!(
                    "Position value {} exceeds max {}",
                    total_cost,
                    self.config.max_position_value()
                ),
            };
        }

        // Create YES and NO orders
        let yes_order = OrderParams {
            token_id: opportunity.yes_token_id.clone(),
            side: Side::Buy,
            price: opportunity.yes_worst_fill,
            size: shares,
            order_type: OrderType::Fok,
            neg_risk: true,
        };

        let no_order = OrderParams {
            token_id: opportunity.no_token_id.clone(),
            side: Side::Buy,
            price: opportunity.no_worst_fill,
            size: shares,
            order_type: OrderType::Fok,
            neg_risk: true,
        };

        // Submit both orders
        let results = match self
            .executor
            .submit_orders_batch(vec![yes_order, no_order])
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return DualLegResult::Error {
                    error: format!("Batch submission failed: {}", e),
                }
            }
        };

        // Check we got both results
        if results.len() != 2 {
            return DualLegResult::Error {
                error: format!("Expected 2 results, got {}", results.len()),
            };
        }

        let yes_result = results[0].clone();
        let no_result = results[1].clone();

        // Determine outcome
        let yes_filled = yes_result.status == OrderStatus::Filled;
        let no_filled = no_result.status == OrderStatus::Filled;

        match (yes_filled, no_filled) {
            (true, true) => {
                // Both filled - success!
                let actual_yes_cost = yes_result.fill_notional();
                let actual_no_cost = no_result.fill_notional();
                let actual_total_cost = actual_yes_cost + actual_no_cost;
                let min_shares = yes_result.filled_size.min(no_result.filled_size);

                // Calculate net profit (shares pay out $1 each, minus cost and fees)
                let net_profit = if min_shares > Decimal::ZERO {
                    let gross = min_shares - actual_total_cost;
                    let pair_cost = actual_total_cost / min_shares;
                    let expected_fee = self.config.expected_fee(pair_cost);
                    gross - expected_fee * min_shares
                } else {
                    Decimal::ZERO
                };

                DualLegResult::Success {
                    yes_result,
                    no_result,
                    total_cost: actual_total_cost,
                    net_profit,
                    shares: min_shares,
                }
            }
            (true, false) => {
                // Only YES filled - need to unwind
                let unwind_result = self
                    .attempt_unwind(&yes_result, &opportunity.yes_token_id)
                    .await;
                DualLegResult::YesOnlyFilled {
                    yes_result,
                    no_result,
                    unwind_result: Some(unwind_result),
                }
            }
            (false, true) => {
                // Only NO filled - need to unwind
                let unwind_result = self
                    .attempt_unwind(&no_result, &opportunity.no_token_id)
                    .await;
                DualLegResult::NoOnlyFilled {
                    yes_result,
                    no_result,
                    unwind_result: Some(unwind_result),
                }
            }
            (false, false) => {
                // Neither filled - no exposure
                DualLegResult::BothRejected {
                    yes_result,
                    no_result,
                }
            }
        }
    }

    /// Attempts to unwind a position by selling at market.
    async fn attempt_unwind(&self, filled_order: &OrderResult, token_id: &str) -> UnwindResult {
        let size = filled_order.filled_size;
        let original_price = filled_order.avg_fill_price.unwrap_or(dec!(0.50));

        // Create FAK sell order to unwind
        let unwind_order = OrderParams {
            token_id: token_id.to_string(),
            side: Side::Sell,
            price: dec!(0.01), // Accept any price (will fill at best bid)
            size,
            order_type: OrderType::Fak, // Fill what we can
            neg_risk: true,
        };

        match self.executor.submit_order(unwind_order).await {
            Ok(result) => {
                let filled_size = result.filled_size;
                let complete = filled_size >= size;
                let sell_price = result.avg_fill_price.unwrap_or(original_price);
                let slippage = original_price - sell_price;

                UnwindResult {
                    order_result: result,
                    filled_size,
                    complete,
                    slippage,
                }
            }
            Err(e) => UnwindResult {
                order_result: OrderResult::rejected("unwind-failed", e.to_string()),
                filled_size: Decimal::ZERO,
                complete: false,
                slippage: Decimal::ZERO,
            },
        }
    }

    /// Calculates the slippage between expected and actual fill prices.
    #[must_use]
    pub fn calculate_slippage(
        expected_yes: Decimal,
        expected_no: Decimal,
        actual_yes: Decimal,
        actual_no: Decimal,
    ) -> SlippageMetrics {
        let yes_slippage = actual_yes - expected_yes;
        let no_slippage = actual_no - expected_no;
        let total_slippage = yes_slippage + no_slippage;

        SlippageMetrics {
            yes_slippage,
            no_slippage,
            total_slippage,
            yes_slippage_pct: if expected_yes > Decimal::ZERO {
                yes_slippage / expected_yes * dec!(100)
            } else {
                Decimal::ZERO
            },
            no_slippage_pct: if expected_no > Decimal::ZERO {
                no_slippage / expected_no * dec!(100)
            } else {
                Decimal::ZERO
            },
        }
    }
}

/// Slippage metrics from execution.
#[derive(Debug, Clone, Copy)]
pub struct SlippageMetrics {
    /// YES side slippage (positive = worse than expected).
    pub yes_slippage: Decimal,
    /// NO side slippage (positive = worse than expected).
    pub no_slippage: Decimal,
    /// Total slippage.
    pub total_slippage: Decimal,
    /// YES slippage as percentage.
    pub yes_slippage_pct: Decimal,
    /// NO slippage as percentage.
    pub no_slippage_pct: Decimal,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitrage::paper_executor::{PaperExecutor, PaperExecutorConfig};
    use chrono::Utc;

    fn create_test_opportunity() -> ArbitrageOpportunity {
        ArbitrageOpportunity {
            market_id: "test-market".to_string(),
            yes_token_id: "yes-token".to_string(),
            no_token_id: "no-token".to_string(),
            yes_worst_fill: dec!(0.48),
            no_worst_fill: dec!(0.48),
            pair_cost: dec!(0.96),
            gross_profit_per_pair: dec!(0.04),
            expected_fee: dec!(0.0104),
            gas_cost: dec!(0.014),
            net_profit_per_pair: dec!(0.0156),
            roi: dec!(1.625),
            recommended_size: dec!(500),
            total_investment: dec!(480),
            guaranteed_payout: dec!(500),
            yes_depth: dec!(2000),
            no_depth: dec!(2000),
            risk_score: 0.1,
            detected_at: Utc::now(),
        }
    }

    // ==================== Both Legs Filled Tests ====================

    #[tokio::test]
    async fn test_both_legs_filled_success() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());
        let dual = DualLegExecutor::new(executor);
        let opp = create_test_opportunity();

        let result = dual.execute(&opp, dec!(100)).await;

        assert!(result.is_success());
        assert!(!result.is_partial());
        assert!(!result.is_rejected());
        assert_eq!(result.imbalance(), Decimal::ZERO);

        // Verify costs
        if let DualLegResult::Success {
            total_cost, shares, ..
        } = result
        {
            // 100 * 0.48 + 100 * 0.48 = 96
            assert_eq!(total_cost, dec!(96));
            assert_eq!(shares, dec!(100));
        } else {
            panic!("Expected Success");
        }
    }

    #[tokio::test]
    async fn test_both_legs_filled_net_profit() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());
        let dual = DualLegExecutor::new(executor);
        let opp = create_test_opportunity();

        let result = dual.execute(&opp, dec!(100)).await;

        if let DualLegResult::Success { net_profit, .. } = result {
            // Gross profit = 100 - 96 = 4
            // Fee = 0.01 * (2 - 0.96) * 100 = 1.04
            // Net = 4 - 1.04 = 2.96
            assert!(net_profit > dec!(2));
            assert!(net_profit < dec!(4));
        } else {
            panic!("Expected Success");
        }
    }

    // ==================== YES Only Filled Tests ====================

    #[tokio::test]
    async fn test_yes_only_filled_triggers_unwind() {
        // Create executor that fills YES but not NO
        // Initial balance of 50 means:
        // - YES order: 100 * 0.48 = $48 - FILLS (balance becomes $2)
        // - NO order: 100 * 0.48 = $48 - REJECTED (only $2 left)
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(50),
            ..PaperExecutorConfig::always_fill()
        });
        let dual = DualLegExecutor::new(executor);
        let opp = create_test_opportunity();

        let result = dual.execute(&opp, dec!(100)).await;

        // First order fills, second fails due to insufficient balance
        assert!(result.is_partial(), "Expected partial, got {:?}", result);

        match result {
            DualLegResult::YesOnlyFilled { unwind_result, .. } => {
                assert!(unwind_result.is_some());
            }
            _ => panic!("Expected YesOnlyFilled, got {:?}", result),
        }
    }

    // ==================== NO Only Filled Tests ====================

    #[test]
    fn test_no_only_filled_structure() {
        // Test the NoOnlyFilled structure with incomplete unwind
        // (complete unwind results in 0 imbalance)
        let result = DualLegResult::NoOnlyFilled {
            yes_result: OrderResult::rejected("yes-123", "No fill"),
            no_result: OrderResult::filled("no-123", dec!(100), dec!(0.48)),
            unwind_result: Some(UnwindResult {
                order_result: OrderResult::filled("unwind-123", dec!(80), dec!(0.46)),
                filled_size: dec!(80), // Only 80 of 100 unwound
                complete: false,
                slippage: dec!(0.02),
            }),
        };

        assert!(result.is_partial());
        // Imbalance should be negative (more NO than YES)
        // 100 NO filled, 80 unwound = -20 imbalance
        assert_eq!(result.imbalance(), dec!(-20));
    }

    // ==================== Both Rejected Tests ====================

    #[tokio::test]
    async fn test_both_rejected_no_exposure() {
        let executor = PaperExecutor::new(PaperExecutorConfig::never_fill());
        let dual = DualLegExecutor::new(executor);
        let opp = create_test_opportunity();

        let result = dual.execute(&opp, dec!(100)).await;

        assert!(result.is_rejected());
        assert!(!result.is_success());
        assert!(!result.is_partial());
        assert_eq!(result.imbalance(), Decimal::ZERO);

        match result {
            DualLegResult::BothRejected {
                yes_result,
                no_result,
            } => {
                assert!(!yes_result.is_filled());
                assert!(!no_result.is_filled());
            }
            _ => panic!("Expected BothRejected"),
        }
    }

    // ==================== Slippage Calculation Tests ====================

    #[test]
    fn test_slippage_calculation() {
        let metrics = DualLegExecutor::<PaperExecutor>::calculate_slippage(
            dec!(0.48), // expected YES
            dec!(0.48), // expected NO
            dec!(0.49), // actual YES
            dec!(0.49), // actual NO
        );

        assert_eq!(metrics.yes_slippage, dec!(0.01));
        assert_eq!(metrics.no_slippage, dec!(0.01));
        assert_eq!(metrics.total_slippage, dec!(0.02));
    }

    #[test]
    fn test_slippage_negative() {
        // Better than expected (negative slippage)
        let metrics = DualLegExecutor::<PaperExecutor>::calculate_slippage(
            dec!(0.48),
            dec!(0.48),
            dec!(0.47),
            dec!(0.47),
        );

        assert_eq!(metrics.yes_slippage, dec!(-0.01));
        assert_eq!(metrics.no_slippage, dec!(-0.01));
        assert_eq!(metrics.total_slippage, dec!(-0.02));
    }

    #[test]
    fn test_slippage_mixed() {
        let metrics = DualLegExecutor::<PaperExecutor>::calculate_slippage(
            dec!(0.48),
            dec!(0.48),
            dec!(0.49), // YES worse
            dec!(0.47), // NO better
        );

        assert_eq!(metrics.yes_slippage, dec!(0.01));
        assert_eq!(metrics.no_slippage, dec!(-0.01));
        assert_eq!(metrics.total_slippage, Decimal::ZERO);
    }

    #[test]
    fn test_slippage_percentage() {
        let metrics = DualLegExecutor::<PaperExecutor>::calculate_slippage(
            dec!(0.50),
            dec!(0.50),
            dec!(0.51),
            dec!(0.51),
        );

        // 0.01 / 0.50 * 100 = 2%
        assert_eq!(metrics.yes_slippage_pct, dec!(2));
        assert_eq!(metrics.no_slippage_pct, dec!(2));
    }

    // ==================== Validation Tests ====================

    #[tokio::test]
    async fn test_rejects_invalid_opportunity() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());
        let dual = DualLegExecutor::new(executor);

        // Create opportunity with pair cost too high
        let mut opp = create_test_opportunity();
        opp.pair_cost = dec!(0.98); // Exceeds MAX_PAIR_COST

        let result = dual.execute(&opp, dec!(100)).await;

        match result {
            DualLegResult::Error { error } => {
                assert!(error.contains("validation failed"));
            }
            _ => panic!("Expected Error"),
        }
    }

    #[tokio::test]
    async fn test_rejects_position_too_large() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());
        let dual = DualLegExecutor::new(executor);
        let opp = create_test_opportunity();

        // Try to buy too many shares (exceeds $500 limit)
        // 600 shares * 0.96 = $576 > $500
        let result = dual.execute(&opp, dec!(600)).await;

        match result {
            DualLegResult::Error { error } => {
                assert!(error.contains("exceeds max"));
            }
            _ => panic!("Expected Error, got {:?}", result),
        }
    }

    // ==================== Unwind Result Tests ====================

    #[test]
    fn test_unwind_result_complete() {
        let unwind = UnwindResult {
            order_result: OrderResult::filled("unwind-1", dec!(100), dec!(0.47)),
            filled_size: dec!(100),
            complete: true,
            slippage: dec!(0.01),
        };

        assert!(unwind.complete);
        assert_eq!(unwind.filled_size, dec!(100));
    }

    #[test]
    fn test_unwind_result_partial() {
        let unwind = UnwindResult {
            order_result: OrderResult {
                order_id: "unwind-2".to_string(),
                status: OrderStatus::PartiallyFilled,
                filled_size: dec!(50),
                avg_fill_price: Some(dec!(0.45)),
                error: None,
            },
            filled_size: dec!(50),
            complete: false,
            slippage: dec!(0.03),
        };

        assert!(!unwind.complete);
        assert_eq!(unwind.filled_size, dec!(50));
    }

    // ==================== Imbalance Calculation Tests ====================

    #[test]
    fn test_imbalance_yes_only_no_unwind() {
        let result = DualLegResult::YesOnlyFilled {
            yes_result: OrderResult::filled("yes", dec!(100), dec!(0.48)),
            no_result: OrderResult::rejected("no", "rejected"),
            unwind_result: None,
        };

        // 100 YES filled, 0 unwound = 100 imbalance
        assert_eq!(result.imbalance(), dec!(100));
    }

    #[test]
    fn test_imbalance_yes_only_partial_unwind() {
        let result = DualLegResult::YesOnlyFilled {
            yes_result: OrderResult::filled("yes", dec!(100), dec!(0.48)),
            no_result: OrderResult::rejected("no", "rejected"),
            unwind_result: Some(UnwindResult {
                order_result: OrderResult::filled("unwind", dec!(60), dec!(0.45)),
                filled_size: dec!(60),
                complete: false,
                slippage: dec!(0.03),
            }),
        };

        // 100 YES filled, 60 unwound = 40 imbalance
        assert_eq!(result.imbalance(), dec!(40));
    }

    #[test]
    fn test_imbalance_no_only() {
        let result = DualLegResult::NoOnlyFilled {
            yes_result: OrderResult::rejected("yes", "rejected"),
            no_result: OrderResult::filled("no", dec!(100), dec!(0.48)),
            unwind_result: Some(UnwindResult {
                order_result: OrderResult::filled("unwind", dec!(100), dec!(0.45)),
                filled_size: dec!(100),
                complete: true,
                slippage: dec!(0.03),
            }),
        };

        // 100 NO filled, 100 unwound = 0 imbalance (but negative sign)
        assert_eq!(result.imbalance(), Decimal::ZERO);
    }

    // ==================== DualLegResult Accessors Tests ====================

    #[test]
    fn test_dual_leg_result_accessors() {
        let success = DualLegResult::Success {
            yes_result: OrderResult::filled("yes", dec!(100), dec!(0.48)),
            no_result: OrderResult::filled("no", dec!(100), dec!(0.48)),
            total_cost: dec!(96),
            net_profit: dec!(2.96),
            shares: dec!(100),
        };

        assert_eq!(success.total_cost(), Some(dec!(96)));
        assert_eq!(success.net_profit(), Some(dec!(2.96)));

        let rejected = DualLegResult::BothRejected {
            yes_result: OrderResult::rejected("yes", "no fill"),
            no_result: OrderResult::rejected("no", "no fill"),
        };

        assert_eq!(rejected.total_cost(), None);
        assert_eq!(rejected.net_profit(), None);
    }

    // ==================== Edge Cases ====================

    #[tokio::test]
    async fn test_zero_shares() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());
        let dual = DualLegExecutor::new(executor);
        let opp = create_test_opportunity();

        // Zero shares should still work (trivially)
        let result = dual.execute(&opp, Decimal::ZERO).await;

        // With zero shares, total cost is 0 which is <= 500, so validation passes
        // The execution succeeds with 0 shares
        assert!(result.is_success());
        if let DualLegResult::Success {
            shares, net_profit, ..
        } = result
        {
            assert_eq!(shares, Decimal::ZERO);
            assert_eq!(net_profit, Decimal::ZERO);
        }
    }

    #[test]
    fn test_slippage_zero_expected() {
        let metrics = DualLegExecutor::<PaperExecutor>::calculate_slippage(
            Decimal::ZERO,
            Decimal::ZERO,
            dec!(0.48),
            dec!(0.48),
        );

        // Should handle division by zero gracefully
        assert_eq!(metrics.yes_slippage_pct, Decimal::ZERO);
        assert_eq!(metrics.no_slippage_pct, Decimal::ZERO);
    }
}
