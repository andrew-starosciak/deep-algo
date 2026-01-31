//! Fee models for binary outcome markets.
//!
//! This module provides fee calculation for different market types,
//! with Polymarket's tiered fee structure as the primary implementation.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Trait for calculating trading fees.
pub trait FeeModel: Send + Sync {
    /// Calculates the fee for a given trade.
    ///
    /// # Arguments
    /// * `stake` - The amount being wagered
    /// * `price` - The price per share (0.0 to 1.0)
    ///
    /// # Returns
    /// The fee amount in the same units as stake
    fn calculate_fee(&self, stake: Decimal, price: Decimal) -> Decimal;

    /// Returns the name of this fee model.
    fn name(&self) -> &str;
}

/// Fee tier for Polymarket's tiered fee structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeeTier {
    /// Tier 0: 2% taker fee (new users)
    Tier0,
    /// Tier 1: 1.5% taker fee
    Tier1,
    /// Tier 2: 1% taker fee
    Tier2,
    /// Tier 3: 0.5% taker fee (highest volume traders)
    Tier3,
    /// Maker: 0% fee (providing liquidity)
    Maker,
}

impl FeeTier {
    /// Returns the fee rate for this tier as a decimal (e.g., 0.02 for 2%).
    #[must_use]
    pub fn rate(&self) -> Decimal {
        use rust_decimal_macros::dec;
        match self {
            Self::Tier0 => dec!(0.02),
            Self::Tier1 => dec!(0.015),
            Self::Tier2 => dec!(0.01),
            Self::Tier3 => dec!(0.005),
            Self::Maker => dec!(0),
        }
    }

    /// Returns the fee rate as a percentage for display.
    #[must_use]
    pub fn rate_percent(&self) -> f64 {
        match self {
            Self::Tier0 => 2.0,
            Self::Tier1 => 1.5,
            Self::Tier2 => 1.0,
            Self::Tier3 => 0.5,
            Self::Maker => 0.0,
        }
    }
}

/// Polymarket's fee model with tiered taker fees.
///
/// Fees are calculated on the potential profit, not the stake.
/// This matches Polymarket's actual fee structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolymarketFees {
    /// The fee tier for this trader.
    pub tier: FeeTier,
}

impl PolymarketFees {
    /// Creates a new Polymarket fee model with the specified tier.
    #[must_use]
    pub const fn new(tier: FeeTier) -> Self {
        Self { tier }
    }

    /// Creates a fee model for the default (highest) tier.
    #[must_use]
    pub const fn default_tier() -> Self {
        Self::new(FeeTier::Tier0)
    }

    /// Creates a fee model for makers (0% fee).
    #[must_use]
    pub const fn maker() -> Self {
        Self::new(FeeTier::Maker)
    }

    /// Returns the current fee tier.
    #[must_use]
    pub const fn tier(&self) -> FeeTier {
        self.tier
    }

    /// Calculates the potential profit for a winning bet.
    ///
    /// For binary markets: potential_profit = stake * (1 - price) / price
    /// This is equivalent to: shares - stake where shares = stake / price
    fn potential_profit(stake: Decimal, price: Decimal) -> Decimal {
        if price == Decimal::ZERO || price >= Decimal::ONE {
            return Decimal::ZERO;
        }
        stake * (Decimal::ONE - price) / price
    }
}

impl FeeModel for PolymarketFees {
    /// Calculates the fee based on potential profit.
    ///
    /// Fee = potential_profit * tier_rate
    fn calculate_fee(&self, stake: Decimal, price: Decimal) -> Decimal {
        let profit = Self::potential_profit(stake, price);
        profit * self.tier.rate()
    }

    fn name(&self) -> &str {
        "polymarket"
    }
}

impl Default for PolymarketFees {
    fn default() -> Self {
        Self::default_tier()
    }
}

/// A zero-fee model for testing or fee-free markets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZeroFees;

impl FeeModel for ZeroFees {
    fn calculate_fee(&self, _stake: Decimal, _price: Decimal) -> Decimal {
        Decimal::ZERO
    }

    fn name(&self) -> &str {
        "zero"
    }
}

/// A flat percentage fee model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlatFees {
    /// The fee rate as a decimal (e.g., 0.01 for 1%).
    pub rate: Decimal,
}

impl FlatFees {
    /// Creates a new flat fee model with the specified rate.
    #[must_use]
    pub const fn new(rate: Decimal) -> Self {
        Self { rate }
    }
}

impl FeeModel for FlatFees {
    /// Calculates a flat percentage fee on the stake.
    fn calculate_fee(&self, stake: Decimal, _price: Decimal) -> Decimal {
        stake * self.rate
    }

    fn name(&self) -> &str {
        "flat"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================================
    // FeeTier Tests
    // ============================================================

    #[test]
    fn fee_tier_rates_are_correct() {
        assert_eq!(FeeTier::Tier0.rate(), dec!(0.02));
        assert_eq!(FeeTier::Tier1.rate(), dec!(0.015));
        assert_eq!(FeeTier::Tier2.rate(), dec!(0.01));
        assert_eq!(FeeTier::Tier3.rate(), dec!(0.005));
        assert_eq!(FeeTier::Maker.rate(), dec!(0));
    }

    #[test]
    fn fee_tier_rate_percent_are_correct() {
        assert!((FeeTier::Tier0.rate_percent() - 2.0).abs() < f64::EPSILON);
        assert!((FeeTier::Tier1.rate_percent() - 1.5).abs() < f64::EPSILON);
        assert!((FeeTier::Tier2.rate_percent() - 1.0).abs() < f64::EPSILON);
        assert!((FeeTier::Tier3.rate_percent() - 0.5).abs() < f64::EPSILON);
        assert!((FeeTier::Maker.rate_percent() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fee_tier_serializes_correctly() {
        let tier0 = FeeTier::Tier0;
        let json = serde_json::to_string(&tier0).unwrap();
        assert_eq!(json, r#""Tier0""#);
    }

    #[test]
    fn fee_tier_deserializes_correctly() {
        let tier: FeeTier = serde_json::from_str(r#""Tier2""#).unwrap();
        assert_eq!(tier, FeeTier::Tier2);
    }

    // ============================================================
    // PolymarketFees Tests
    // ============================================================

    #[test]
    fn polymarket_fees_new_sets_tier() {
        let fees = PolymarketFees::new(FeeTier::Tier2);
        assert_eq!(fees.tier(), FeeTier::Tier2);
    }

    #[test]
    fn polymarket_fees_default_tier_is_tier0() {
        let fees = PolymarketFees::default_tier();
        assert_eq!(fees.tier(), FeeTier::Tier0);
    }

    #[test]
    fn polymarket_fees_maker_is_zero() {
        let fees = PolymarketFees::maker();
        assert_eq!(fees.tier(), FeeTier::Maker);
        assert_eq!(fees.tier().rate(), dec!(0));
    }

    #[test]
    fn polymarket_fees_default_impl_matches_default_tier() {
        let fees1 = PolymarketFees::default();
        let fees2 = PolymarketFees::default_tier();
        assert_eq!(fees1.tier(), fees2.tier());
    }

    #[test]
    fn polymarket_fees_name_is_correct() {
        let fees = PolymarketFees::default();
        assert_eq!(fees.name(), "polymarket");
    }

    // ============================================================
    // Fee Calculation Tests - Tier 0 (2%)
    // ============================================================

    #[test]
    fn polymarket_tier0_fee_at_50_percent_price() {
        let fees = PolymarketFees::new(FeeTier::Tier0);

        // stake = $100, price = $0.50
        // potential_profit = 100 * (1 - 0.50) / 0.50 = 100 * 0.50 / 0.50 = $100
        // fee = 100 * 0.02 = $2.00
        let fee = fees.calculate_fee(dec!(100), dec!(0.50));
        assert_eq!(fee, dec!(2));
    }

    #[test]
    fn polymarket_tier0_fee_at_45_percent_price() {
        let fees = PolymarketFees::new(FeeTier::Tier0);

        // stake = $100, price = $0.45
        // potential_profit = 100 * (1 - 0.45) / 0.45 = 100 * 0.55 / 0.45 = $122.222...
        // fee = 122.222... * 0.02 = $2.4444...
        let fee = fees.calculate_fee(dec!(100), dec!(0.45));
        let expected = dec!(100) * dec!(0.55) / dec!(0.45) * dec!(0.02);
        assert_eq!(fee, expected);
    }

    #[test]
    fn polymarket_tier0_fee_at_90_percent_price() {
        let fees = PolymarketFees::new(FeeTier::Tier0);

        // stake = $100, price = $0.90
        // potential_profit = 100 * (1 - 0.90) / 0.90 = 100 * 0.10 / 0.90 = $11.111...
        // fee = 11.111... * 0.02 = $0.2222...
        let fee = fees.calculate_fee(dec!(100), dec!(0.90));
        let expected = dec!(100) * dec!(0.10) / dec!(0.90) * dec!(0.02);
        assert_eq!(fee, expected);
    }

    // ============================================================
    // Fee Calculation Tests - Other Tiers
    // ============================================================

    #[test]
    fn polymarket_tier1_fee_calculation() {
        let fees = PolymarketFees::new(FeeTier::Tier1);

        // stake = $100, price = $0.50, potential_profit = $100
        // fee = 100 * 0.015 = $1.50
        let fee = fees.calculate_fee(dec!(100), dec!(0.50));
        assert_eq!(fee, dec!(1.50));
    }

    #[test]
    fn polymarket_tier2_fee_calculation() {
        let fees = PolymarketFees::new(FeeTier::Tier2);

        // stake = $100, price = $0.50, potential_profit = $100
        // fee = 100 * 0.01 = $1.00
        let fee = fees.calculate_fee(dec!(100), dec!(0.50));
        assert_eq!(fee, dec!(1));
    }

    #[test]
    fn polymarket_tier3_fee_calculation() {
        let fees = PolymarketFees::new(FeeTier::Tier3);

        // stake = $100, price = $0.50, potential_profit = $100
        // fee = 100 * 0.005 = $0.50
        let fee = fees.calculate_fee(dec!(100), dec!(0.50));
        assert_eq!(fee, dec!(0.50));
    }

    #[test]
    fn polymarket_maker_has_zero_fee() {
        let fees = PolymarketFees::maker();

        let fee = fees.calculate_fee(dec!(100), dec!(0.50));
        assert_eq!(fee, dec!(0));
    }

    // ============================================================
    // Edge Case Tests
    // ============================================================

    #[test]
    fn polymarket_fee_with_zero_stake() {
        let fees = PolymarketFees::new(FeeTier::Tier0);
        let fee = fees.calculate_fee(dec!(0), dec!(0.50));
        assert_eq!(fee, dec!(0));
    }

    #[test]
    fn polymarket_fee_with_zero_price() {
        let fees = PolymarketFees::new(FeeTier::Tier0);
        // Zero price means infinite potential profit, but we handle gracefully
        let fee = fees.calculate_fee(dec!(100), dec!(0));
        assert_eq!(fee, dec!(0));
    }

    #[test]
    fn polymarket_fee_with_price_equals_one() {
        let fees = PolymarketFees::new(FeeTier::Tier0);
        // Price = 1.0 means zero potential profit
        let fee = fees.calculate_fee(dec!(100), dec!(1));
        assert_eq!(fee, dec!(0));
    }

    #[test]
    fn polymarket_fee_with_price_above_one() {
        let fees = PolymarketFees::new(FeeTier::Tier0);
        // Invalid price > 1.0, should handle gracefully
        let fee = fees.calculate_fee(dec!(100), dec!(1.5));
        assert_eq!(fee, dec!(0));
    }

    #[test]
    fn polymarket_fee_with_very_low_price() {
        let fees = PolymarketFees::new(FeeTier::Tier0);

        // stake = $100, price = $0.01
        // potential_profit = 100 * (1 - 0.01) / 0.01 = 100 * 0.99 / 0.01 = $9900
        // fee = 9900 * 0.02 = $198
        let fee = fees.calculate_fee(dec!(100), dec!(0.01));
        let expected = dec!(100) * dec!(0.99) / dec!(0.01) * dec!(0.02);
        assert_eq!(fee, expected);
    }

    #[test]
    fn polymarket_fee_with_very_high_price() {
        let fees = PolymarketFees::new(FeeTier::Tier0);

        // stake = $100, price = $0.99
        // potential_profit = 100 * (1 - 0.99) / 0.99 = 100 * 0.01 / 0.99 = $1.0101...
        // fee = 1.0101... * 0.02 = $0.0202...
        let fee = fees.calculate_fee(dec!(100), dec!(0.99));
        let expected = dec!(100) * dec!(0.01) / dec!(0.99) * dec!(0.02);
        assert_eq!(fee, expected);
    }

    // ============================================================
    // ZeroFees Tests
    // ============================================================

    #[test]
    fn zero_fees_always_returns_zero() {
        let fees = ZeroFees;

        assert_eq!(fees.calculate_fee(dec!(100), dec!(0.50)), dec!(0));
        assert_eq!(fees.calculate_fee(dec!(1000), dec!(0.10)), dec!(0));
        assert_eq!(fees.calculate_fee(dec!(0), dec!(0.99)), dec!(0));
    }

    #[test]
    fn zero_fees_name_is_correct() {
        let fees = ZeroFees;
        assert_eq!(fees.name(), "zero");
    }

    // ============================================================
    // FlatFees Tests
    // ============================================================

    #[test]
    fn flat_fees_calculation_at_one_percent() {
        let fees = FlatFees::new(dec!(0.01));

        // stake = $100, fee = 100 * 0.01 = $1
        let fee = fees.calculate_fee(dec!(100), dec!(0.50));
        assert_eq!(fee, dec!(1));
    }

    #[test]
    fn flat_fees_calculation_at_half_percent() {
        let fees = FlatFees::new(dec!(0.005));

        // stake = $200, fee = 200 * 0.005 = $1
        let fee = fees.calculate_fee(dec!(200), dec!(0.50));
        assert_eq!(fee, dec!(1));
    }

    #[test]
    fn flat_fees_ignores_price() {
        let fees = FlatFees::new(dec!(0.01));

        // Fee should be same regardless of price
        let fee1 = fees.calculate_fee(dec!(100), dec!(0.10));
        let fee2 = fees.calculate_fee(dec!(100), dec!(0.90));
        assert_eq!(fee1, fee2);
        assert_eq!(fee1, dec!(1));
    }

    #[test]
    fn flat_fees_name_is_correct() {
        let fees = FlatFees::new(dec!(0.01));
        assert_eq!(fees.name(), "flat");
    }

    // ============================================================
    // Trait Object Tests
    // ============================================================

    #[test]
    fn fee_model_trait_object_works() {
        let models: Vec<Box<dyn FeeModel>> = vec![
            Box::new(PolymarketFees::new(FeeTier::Tier0)),
            Box::new(ZeroFees),
            Box::new(FlatFees::new(dec!(0.01))),
        ];

        let stake = dec!(100);
        let price = dec!(0.50);

        // Each should calculate without panic
        for model in &models {
            let fee = model.calculate_fee(stake, price);
            assert!(fee >= dec!(0));
        }
    }

    #[test]
    fn fee_model_names_are_unique() {
        let poly = PolymarketFees::default();
        let zero = ZeroFees;
        let flat = FlatFees::new(dec!(0.01));

        assert_ne!(poly.name(), zero.name());
        assert_ne!(poly.name(), flat.name());
        assert_ne!(zero.name(), flat.name());
    }

    // ============================================================
    // Serialization Tests
    // ============================================================

    #[test]
    fn polymarket_fees_serialization_roundtrip() {
        let fees = PolymarketFees::new(FeeTier::Tier2);
        let json = serde_json::to_string(&fees).unwrap();
        let deserialized: PolymarketFees = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tier(), FeeTier::Tier2);
    }

    #[test]
    fn flat_fees_serialization_roundtrip() {
        let fees = FlatFees::new(dec!(0.015));
        let json = serde_json::to_string(&fees).unwrap();
        let deserialized: FlatFees = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.rate, dec!(0.015));
    }
}
