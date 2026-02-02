//! Phase 1 Pure Arbitrage configuration with hardcoded conservative values.
//!
//! This module contains the exact parameters for Phase 1 validation:
//! - Maximum pair cost: $0.96 (4% minimum edge)
//! - Minimum edge after fees: 2%
//! - Order type: FOK (Fill-or-Kill for all-or-nothing)
//! - Maximum position value: $500 (risk cap)
//! - Minimum liquidity: $1000 (ensure fills)
//! - Minimum validation trades: 100 (for Go/No-Go decision)
//!
//! These values are intentionally hardcoded to prevent configuration drift
//! during the validation phase.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::execution::OrderType;
use super::types::ArbitrageOpportunity;

// =============================================================================
// Phase 1 Constants (HARDCODED - DO NOT MODIFY)
// =============================================================================

/// Maximum combined YES+NO price to execute arbitrage.
/// At $0.96, we have a 4% gross edge before fees.
pub const MAX_PAIR_COST: Decimal = dec!(0.96);

/// Polymarket fee rate (2% of profit on winning side).
pub const FEE_RATE: Decimal = dec!(0.02);

/// Minimum net edge after fees required to execute.
/// With 2% fee on ~4% gross edge, we need at least 2% net.
pub const MIN_EDGE_AFTER_FEES: Decimal = dec!(0.02);

/// Order type for Phase 1 execution (Fill-or-Kill).
/// FOK ensures we don't get partial fills that create imbalance.
pub const ORDER_TYPE: OrderType = OrderType::Fok;

/// Maximum position value in USDC per market.
/// Limits risk exposure during validation phase.
pub const MAX_POSITION_VALUE: Decimal = dec!(500);

/// Minimum liquidity required on both YES and NO sides.
/// Ensures we can actually fill our orders at the quoted prices.
/// Note: 400 is acceptable for Phase 1 paper validation with small sizes.
/// Consider increasing to 1000+ for production with larger positions.
pub const MIN_LIQUIDITY: Decimal = dec!(400);

/// Minimum number of trades required for Go/No-Go validation.
/// Provides statistical power for confidence intervals.
pub const MIN_VALIDATION_TRADES: u32 = 100;

/// Target fill rate for Go/No-Go (Wilson CI lower bound).
/// With 85% per-leg fill rate, theoretical max is 0.85² = 72.25%.
/// We target 65% to allow for real-world variance.
pub const TARGET_FILL_RATE: f64 = 0.65;

/// Maximum acceptable imbalance (YES - NO shares).
pub const MAX_IMBALANCE: Decimal = dec!(50);

// =============================================================================
// Phase 1 Configuration Struct
// =============================================================================

/// Phase 1 arbitrage configuration with hardcoded conservative values.
///
/// This struct bundles all Phase 1 parameters and provides validation
/// methods for opportunities. All values are derived from the module
/// constants and cannot be modified at runtime.
#[derive(Debug, Clone, Copy)]
pub struct Phase1Config {
    _private: (), // Prevent external construction
}

impl Default for Phase1Config {
    fn default() -> Self {
        Self::new()
    }
}

impl Phase1Config {
    /// Creates a new Phase1Config with hardcoded values.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Returns the maximum pair cost threshold.
    #[must_use]
    pub const fn max_pair_cost(&self) -> Decimal {
        MAX_PAIR_COST
    }

    /// Returns the fee rate.
    #[must_use]
    pub const fn fee_rate(&self) -> Decimal {
        FEE_RATE
    }

    /// Returns the minimum edge after fees.
    #[must_use]
    pub const fn min_edge_after_fees(&self) -> Decimal {
        MIN_EDGE_AFTER_FEES
    }

    /// Returns the order type for execution.
    #[must_use]
    pub const fn order_type(&self) -> OrderType {
        ORDER_TYPE
    }

    /// Returns the maximum position value.
    #[must_use]
    pub const fn max_position_value(&self) -> Decimal {
        MAX_POSITION_VALUE
    }

    /// Returns the minimum liquidity requirement.
    #[must_use]
    pub const fn min_liquidity(&self) -> Decimal {
        MIN_LIQUIDITY
    }

    /// Returns the minimum validation trades.
    #[must_use]
    pub const fn min_validation_trades(&self) -> u32 {
        MIN_VALIDATION_TRADES
    }

    /// Returns the target fill rate.
    #[must_use]
    pub const fn target_fill_rate(&self) -> f64 {
        TARGET_FILL_RATE
    }

    /// Returns the maximum imbalance.
    #[must_use]
    pub const fn max_imbalance(&self) -> Decimal {
        MAX_IMBALANCE
    }

    /// Calculates the gross edge for a given pair cost.
    ///
    /// Gross edge = 1.0 - pair_cost
    #[must_use]
    pub fn gross_edge(&self, pair_cost: Decimal) -> Decimal {
        Decimal::ONE - pair_cost
    }

    /// Calculates the expected fee for a given pair cost.
    ///
    /// E[Fee] = 0.01 * (2 - pair_cost)
    /// This is derived from: Fee = 0.02 * profit on winning side
    #[must_use]
    pub fn expected_fee(&self, pair_cost: Decimal) -> Decimal {
        dec!(0.01) * (dec!(2) - pair_cost)
    }

    /// Calculates the net edge after fees for a given pair cost.
    ///
    /// Net edge = gross_edge - expected_fee
    #[must_use]
    pub fn net_edge(&self, pair_cost: Decimal) -> Decimal {
        self.gross_edge(pair_cost) - self.expected_fee(pair_cost)
    }

    /// Validates whether a pair cost meets Phase 1 requirements.
    ///
    /// Requirements:
    /// 1. pair_cost <= MAX_PAIR_COST (0.96)
    /// 2. net_edge >= MIN_EDGE_AFTER_FEES (0.02)
    #[must_use]
    pub fn validate_pair_cost(&self, pair_cost: Decimal) -> ValidationResult {
        // Check pair cost threshold
        if pair_cost > MAX_PAIR_COST {
            return ValidationResult::Rejected {
                reason: ValidationReason::PairCostTooHigh {
                    pair_cost,
                    max: MAX_PAIR_COST,
                },
            };
        }

        // Check net edge after fees
        let net_edge = self.net_edge(pair_cost);
        if net_edge < MIN_EDGE_AFTER_FEES {
            return ValidationResult::Rejected {
                reason: ValidationReason::InsufficientEdge {
                    net_edge,
                    min: MIN_EDGE_AFTER_FEES,
                },
            };
        }

        ValidationResult::Valid { net_edge }
    }

    /// Validates whether an opportunity meets Phase 1 requirements.
    ///
    /// Requirements:
    /// 1. pair_cost <= MAX_PAIR_COST (0.96)
    /// 2. net_edge >= MIN_EDGE_AFTER_FEES (0.02)
    /// 3. yes_depth >= MIN_LIQUIDITY (400)
    /// 4. no_depth >= MIN_LIQUIDITY (400)
    /// 5. position_value <= MAX_POSITION_VALUE (500)
    #[must_use]
    pub fn validate_opportunity(&self, opp: &ArbitrageOpportunity) -> ValidationResult {
        // First validate pair cost
        let pair_cost_result = self.validate_pair_cost(opp.pair_cost);
        if let ValidationResult::Rejected { reason } = pair_cost_result {
            return ValidationResult::Rejected { reason };
        }

        // Check YES liquidity
        if opp.yes_depth < MIN_LIQUIDITY {
            return ValidationResult::Rejected {
                reason: ValidationReason::InsufficientLiquidity {
                    side: "YES",
                    depth: opp.yes_depth,
                    min: MIN_LIQUIDITY,
                },
            };
        }

        // Check NO liquidity
        if opp.no_depth < MIN_LIQUIDITY {
            return ValidationResult::Rejected {
                reason: ValidationReason::InsufficientLiquidity {
                    side: "NO",
                    depth: opp.no_depth,
                    min: MIN_LIQUIDITY,
                },
            };
        }

        // Check position value
        if opp.total_investment > MAX_POSITION_VALUE {
            return ValidationResult::Rejected {
                reason: ValidationReason::PositionTooLarge {
                    value: opp.total_investment,
                    max: MAX_POSITION_VALUE,
                },
            };
        }

        ValidationResult::Valid {
            net_edge: self.net_edge(opp.pair_cost),
        }
    }

    /// Calculates the maximum shares we can buy given the position value limit.
    ///
    /// max_shares = MAX_POSITION_VALUE / pair_cost
    #[must_use]
    pub fn max_shares_for_pair_cost(&self, pair_cost: Decimal) -> Decimal {
        if pair_cost <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        MAX_POSITION_VALUE / pair_cost
    }
}

// =============================================================================
// Validation Result
// =============================================================================

/// Result of Phase 1 validation.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    /// Opportunity meets all Phase 1 requirements.
    Valid {
        /// Net edge after fees.
        net_edge: Decimal,
    },
    /// Opportunity rejected for a specific reason.
    Rejected {
        /// Reason for rejection.
        reason: ValidationReason,
    },
}

impl ValidationResult {
    /// Returns true if the result is valid.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        matches!(self, ValidationResult::Valid { .. })
    }

    /// Returns the net edge if valid.
    #[must_use]
    pub fn net_edge(&self) -> Option<Decimal> {
        match self {
            ValidationResult::Valid { net_edge } => Some(*net_edge),
            ValidationResult::Rejected { .. } => None,
        }
    }
}

/// Reason for validation rejection.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationReason {
    /// Pair cost exceeds maximum threshold.
    PairCostTooHigh {
        /// Actual pair cost.
        pair_cost: Decimal,
        /// Maximum allowed.
        max: Decimal,
    },
    /// Net edge after fees is too low.
    InsufficientEdge {
        /// Actual net edge.
        net_edge: Decimal,
        /// Minimum required.
        min: Decimal,
    },
    /// Insufficient liquidity on one side.
    InsufficientLiquidity {
        /// Which side (YES or NO).
        side: &'static str,
        /// Available depth.
        depth: Decimal,
        /// Minimum required.
        min: Decimal,
    },
    /// Position value exceeds maximum.
    PositionTooLarge {
        /// Proposed position value.
        value: Decimal,
        /// Maximum allowed.
        max: Decimal,
    },
}

impl std::fmt::Display for ValidationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationReason::PairCostTooHigh { pair_cost, max } => {
                write!(f, "Pair cost {} exceeds max {}", pair_cost, max)
            }
            ValidationReason::InsufficientEdge { net_edge, min } => {
                write!(f, "Net edge {} below min {}", net_edge, min)
            }
            ValidationReason::InsufficientLiquidity { side, depth, min } => {
                write!(f, "{} liquidity {} below min {}", side, depth, min)
            }
            ValidationReason::PositionTooLarge { value, max } => {
                write!(f, "Position value {} exceeds max {}", value, max)
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // ==================== Hardcoded Values Tests ====================

    #[test]
    fn test_hardcoded_values_exact() {
        // These values MUST match the spec exactly
        assert_eq!(MAX_PAIR_COST, dec!(0.96));
        assert_eq!(FEE_RATE, dec!(0.02));
        assert_eq!(MIN_EDGE_AFTER_FEES, dec!(0.02));
        assert_eq!(ORDER_TYPE, OrderType::Fok);
        assert_eq!(MAX_POSITION_VALUE, dec!(500));
        assert_eq!(MIN_LIQUIDITY, dec!(400)); // Lowered for Phase 1 validation
        assert_eq!(MIN_VALIDATION_TRADES, 100);
    }

    #[test]
    fn test_config_returns_hardcoded_values() {
        let config = Phase1Config::new();

        assert_eq!(config.max_pair_cost(), dec!(0.96));
        assert_eq!(config.fee_rate(), dec!(0.02));
        assert_eq!(config.min_edge_after_fees(), dec!(0.02));
        assert_eq!(config.order_type(), OrderType::Fok);
        assert_eq!(config.max_position_value(), dec!(500));
        assert_eq!(config.min_liquidity(), dec!(400));
        assert_eq!(config.min_validation_trades(), 100);
        assert!((config.target_fill_rate() - 0.65).abs() < f64::EPSILON);
        assert_eq!(config.max_imbalance(), dec!(50));
    }

    #[test]
    fn test_config_default() {
        let config = Phase1Config::default();
        assert_eq!(config.max_pair_cost(), dec!(0.96));
    }

    // ==================== Edge Calculation Tests ====================

    #[test]
    fn test_gross_edge_at_96_cents() {
        let config = Phase1Config::new();
        let gross_edge = config.gross_edge(dec!(0.96));
        assert_eq!(gross_edge, dec!(0.04)); // 4% gross edge
    }

    #[test]
    fn test_gross_edge_at_95_cents() {
        let config = Phase1Config::new();
        let gross_edge = config.gross_edge(dec!(0.95));
        assert_eq!(gross_edge, dec!(0.05)); // 5% gross edge
    }

    #[test]
    fn test_expected_fee_at_96_cents() {
        let config = Phase1Config::new();
        // E[Fee] = 0.01 * (2 - 0.96) = 0.01 * 1.04 = 0.0104
        let fee = config.expected_fee(dec!(0.96));
        assert_eq!(fee, dec!(0.0104));
    }

    #[test]
    fn test_expected_fee_at_95_cents() {
        let config = Phase1Config::new();
        // E[Fee] = 0.01 * (2 - 0.95) = 0.01 * 1.05 = 0.0105
        let fee = config.expected_fee(dec!(0.95));
        assert_eq!(fee, dec!(0.0105));
    }

    #[test]
    fn test_net_edge_at_96_cents() {
        let config = Phase1Config::new();
        // Net = 0.04 - 0.0104 = 0.0296
        let net_edge = config.net_edge(dec!(0.96));
        assert_eq!(net_edge, dec!(0.0296));
    }

    #[test]
    fn test_net_edge_at_95_cents() {
        let config = Phase1Config::new();
        // Net = 0.05 - 0.0105 = 0.0395
        let net_edge = config.net_edge(dec!(0.95));
        assert_eq!(net_edge, dec!(0.0395));
    }

    // ==================== Pair Cost Validation Tests ====================

    #[test]
    fn test_validate_accepts_4_percent_edge() {
        let config = Phase1Config::new();
        // 0.96 pair cost = 4% gross edge
        let result = config.validate_pair_cost(dec!(0.96));
        assert!(result.is_valid());
        assert_eq!(result.net_edge(), Some(dec!(0.0296)));
    }

    #[test]
    fn test_validate_accepts_5_percent_edge() {
        let config = Phase1Config::new();
        // 0.95 pair cost = 5% gross edge
        let result = config.validate_pair_cost(dec!(0.95));
        assert!(result.is_valid());
    }

    #[test]
    fn test_validate_rejects_3_percent_edge() {
        let config = Phase1Config::new();
        // 0.97 pair cost = 3% gross edge, exceeds MAX_PAIR_COST
        let result = config.validate_pair_cost(dec!(0.97));
        assert!(!result.is_valid());
        match result {
            ValidationResult::Rejected { reason } => {
                assert!(matches!(reason, ValidationReason::PairCostTooHigh { .. }));
            }
            _ => panic!("Expected rejection"),
        }
    }

    #[test]
    fn test_validate_rejects_2_percent_edge() {
        let config = Phase1Config::new();
        // 0.98 pair cost = 2% gross edge
        let result = config.validate_pair_cost(dec!(0.98));
        assert!(!result.is_valid());
    }

    #[test]
    fn test_validate_rejects_zero_edge() {
        let config = Phase1Config::new();
        // 1.00 pair cost = 0% edge
        let result = config.validate_pair_cost(dec!(1.00));
        assert!(!result.is_valid());
    }

    #[test]
    fn test_validate_rejects_negative_edge() {
        let config = Phase1Config::new();
        // 1.02 pair cost = negative edge
        let result = config.validate_pair_cost(dec!(1.02));
        assert!(!result.is_valid());
    }

    // ==================== Opportunity Validation Tests ====================

    fn create_test_opportunity(
        pair_cost: Decimal,
        yes_depth: Decimal,
        no_depth: Decimal,
        total_investment: Decimal,
    ) -> ArbitrageOpportunity {
        ArbitrageOpportunity {
            market_id: "test-market".to_string(),
            yes_token_id: "yes-token".to_string(),
            no_token_id: "no-token".to_string(),
            yes_worst_fill: pair_cost / dec!(2),
            no_worst_fill: pair_cost / dec!(2),
            pair_cost,
            gross_profit_per_pair: Decimal::ONE - pair_cost,
            expected_fee: dec!(0.01) * (dec!(2) - pair_cost),
            gas_cost: dec!(0.014),
            net_profit_per_pair: Decimal::ONE
                - pair_cost
                - dec!(0.01) * (dec!(2) - pair_cost)
                - dec!(0.014),
            roi: dec!(3),
            recommended_size: total_investment / pair_cost,
            total_investment,
            guaranteed_payout: total_investment / pair_cost,
            yes_depth,
            no_depth,
            risk_score: 0.2,
            detected_at: Utc::now(),
        }
    }

    #[test]
    fn test_validate_opportunity_valid() {
        let config = Phase1Config::new();
        let opp = create_test_opportunity(dec!(0.96), dec!(2000), dec!(2000), dec!(480));

        let result = config.validate_opportunity(&opp);
        assert!(result.is_valid());
    }

    #[test]
    fn test_validate_rejects_low_liquidity_yes() {
        let config = Phase1Config::new();
        // YES depth below MIN_LIQUIDITY (400)
        let opp = create_test_opportunity(dec!(0.96), dec!(300), dec!(2000), dec!(480));

        let result = config.validate_opportunity(&opp);
        assert!(!result.is_valid());
        match result {
            ValidationResult::Rejected { reason } => match reason {
                ValidationReason::InsufficientLiquidity { side, .. } => {
                    assert_eq!(side, "YES");
                }
                _ => panic!("Expected InsufficientLiquidity"),
            },
            _ => panic!("Expected rejection"),
        }
    }

    #[test]
    fn test_validate_rejects_low_liquidity_no() {
        let config = Phase1Config::new();
        // NO depth below MIN_LIQUIDITY (400)
        let opp = create_test_opportunity(dec!(0.96), dec!(2000), dec!(300), dec!(480));

        let result = config.validate_opportunity(&opp);
        assert!(!result.is_valid());
        match result {
            ValidationResult::Rejected { reason } => match reason {
                ValidationReason::InsufficientLiquidity { side, .. } => {
                    assert_eq!(side, "NO");
                }
                _ => panic!("Expected InsufficientLiquidity"),
            },
            _ => panic!("Expected rejection"),
        }
    }

    #[test]
    fn test_validate_rejects_position_too_large() {
        let config = Phase1Config::new();
        // Position value exceeds MAX_POSITION_VALUE (500)
        let opp = create_test_opportunity(dec!(0.96), dec!(2000), dec!(2000), dec!(600));

        let result = config.validate_opportunity(&opp);
        assert!(!result.is_valid());
        match result {
            ValidationResult::Rejected { reason } => {
                assert!(matches!(reason, ValidationReason::PositionTooLarge { .. }));
            }
            _ => panic!("Expected rejection"),
        }
    }

    #[test]
    fn test_validate_opportunity_rejects_high_pair_cost() {
        let config = Phase1Config::new();
        // Pair cost 0.97 exceeds MAX_PAIR_COST (0.96)
        let opp = create_test_opportunity(dec!(0.97), dec!(2000), dec!(2000), dec!(480));

        let result = config.validate_opportunity(&opp);
        assert!(!result.is_valid());
    }

    // ==================== Max Shares Calculation Tests ====================

    #[test]
    fn test_max_shares_at_96_cents() {
        let config = Phase1Config::new();
        // 500 / 0.96 = 520.833...
        let max_shares = config.max_shares_for_pair_cost(dec!(0.96));
        assert!(max_shares > dec!(520));
        assert!(max_shares < dec!(521));
    }

    #[test]
    fn test_max_shares_at_95_cents() {
        let config = Phase1Config::new();
        // 500 / 0.95 = 526.315...
        let max_shares = config.max_shares_for_pair_cost(dec!(0.95));
        assert!(max_shares > dec!(526));
        assert!(max_shares < dec!(527));
    }

    #[test]
    fn test_max_shares_zero_pair_cost() {
        let config = Phase1Config::new();
        let max_shares = config.max_shares_for_pair_cost(dec!(0));
        assert_eq!(max_shares, Decimal::ZERO);
    }

    #[test]
    fn test_max_shares_negative_pair_cost() {
        let config = Phase1Config::new();
        let max_shares = config.max_shares_for_pair_cost(dec!(-0.5));
        assert_eq!(max_shares, Decimal::ZERO);
    }

    // ==================== Validation Reason Display Tests ====================

    #[test]
    fn test_validation_reason_display() {
        let reason = ValidationReason::PairCostTooHigh {
            pair_cost: dec!(0.97),
            max: dec!(0.96),
        };
        let display = format!("{}", reason);
        assert!(display.contains("0.97"));
        assert!(display.contains("0.96"));

        let reason2 = ValidationReason::InsufficientLiquidity {
            side: "YES",
            depth: dec!(300),
            min: dec!(400),
        };
        let display2 = format!("{}", reason2);
        assert!(display2.contains("YES"));
        assert!(display2.contains("300"));
        assert!(display2.contains("400"));
    }

    // ==================== Edge Cases Tests ====================

    #[test]
    fn test_validate_exactly_at_threshold() {
        let config = Phase1Config::new();
        // Exactly at MAX_PAIR_COST should be valid
        let result = config.validate_pair_cost(dec!(0.96));
        assert!(result.is_valid());
    }

    #[test]
    fn test_validate_just_above_threshold() {
        let config = Phase1Config::new();
        // Just above MAX_PAIR_COST should be rejected
        let result = config.validate_pair_cost(dec!(0.9601));
        assert!(!result.is_valid());
    }

    #[test]
    fn test_opportunity_exactly_at_liquidity_threshold() {
        let config = Phase1Config::new();
        // Exactly at MIN_LIQUIDITY should be valid
        let opp = create_test_opportunity(dec!(0.96), dec!(400), dec!(400), dec!(480));
        let result = config.validate_opportunity(&opp);
        assert!(result.is_valid());
    }

    #[test]
    fn test_opportunity_exactly_at_position_threshold() {
        let config = Phase1Config::new();
        // Exactly at MAX_POSITION_VALUE should be valid
        let opp = create_test_opportunity(dec!(0.96), dec!(2000), dec!(2000), dec!(500));
        let result = config.validate_opportunity(&opp);
        assert!(result.is_valid());
    }

    #[test]
    fn test_opportunity_just_above_position_threshold() {
        let config = Phase1Config::new();
        // Just above MAX_POSITION_VALUE should be rejected
        let opp = create_test_opportunity(dec!(0.96), dec!(2000), dec!(2000), dec!(500.01));
        let result = config.validate_opportunity(&opp);
        assert!(!result.is_valid());
    }
}
