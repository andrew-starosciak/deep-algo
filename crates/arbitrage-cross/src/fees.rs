//! Fee calculations for cross-exchange arbitrage.
//!
//! This module provides fee calculations for both Kalshi and Polymarket,
//! allowing accurate profit projections for arbitrage opportunities.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::types::Exchange;

// =============================================================================
// Fee Constants
// =============================================================================

/// Default Kalshi fee rate (0.7% per trade).
pub const KALSHI_DEFAULT_FEE_RATE: Decimal = dec!(0.007);

/// Default Polymarket trading fee rate (0.01% per trade).
pub const POLYMARKET_TRADING_FEE_RATE: Decimal = dec!(0.0001);

/// Polymarket profit fee rate (2% on profit, only on winning side).
pub const POLYMARKET_PROFIT_FEE_RATE: Decimal = dec!(0.02);

// =============================================================================
// Fee Configuration
// =============================================================================

/// Configuration for fee calculations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeConfig {
    /// Kalshi fee rate (as decimal, e.g., 0.007 for 0.7%).
    pub kalshi_fee_rate: Decimal,

    /// Polymarket trading fee rate (as decimal).
    pub polymarket_trading_fee_rate: Decimal,

    /// Polymarket profit fee rate (as decimal).
    pub polymarket_profit_fee_rate: Decimal,

    /// Whether to include estimated gas costs for Polymarket.
    pub include_gas_estimate: bool,

    /// Estimated gas cost per Polymarket trade in USDC.
    pub estimated_gas_per_trade: Decimal,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            kalshi_fee_rate: KALSHI_DEFAULT_FEE_RATE,
            polymarket_trading_fee_rate: POLYMARKET_TRADING_FEE_RATE,
            polymarket_profit_fee_rate: POLYMARKET_PROFIT_FEE_RATE,
            include_gas_estimate: false,
            estimated_gas_per_trade: dec!(0.01),
        }
    }
}

impl FeeConfig {
    /// Creates a new fee configuration with custom rates.
    #[must_use]
    pub fn new(
        kalshi_fee_rate: Decimal,
        polymarket_trading_fee_rate: Decimal,
        polymarket_profit_fee_rate: Decimal,
    ) -> Self {
        Self {
            kalshi_fee_rate,
            polymarket_trading_fee_rate,
            polymarket_profit_fee_rate,
            include_gas_estimate: false,
            estimated_gas_per_trade: dec!(0.01),
        }
    }

    /// Enables gas cost estimation.
    #[must_use]
    pub fn with_gas_estimate(mut self, gas_per_trade: Decimal) -> Self {
        self.include_gas_estimate = true;
        self.estimated_gas_per_trade = gas_per_trade;
        self
    }
}

// =============================================================================
// Fee Calculator
// =============================================================================

/// Calculator for cross-exchange arbitrage fees.
#[derive(Debug, Clone)]
pub struct FeeCalculator {
    config: FeeConfig,
}

impl FeeCalculator {
    /// Creates a new fee calculator with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: FeeConfig::default(),
        }
    }

    /// Creates a new fee calculator with custom configuration.
    #[must_use]
    pub fn with_config(config: FeeConfig) -> Self {
        Self { config }
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &FeeConfig {
        &self.config
    }

    /// Calculates the fee for a Kalshi trade.
    ///
    /// Kalshi charges a flat percentage fee on the trade value.
    ///
    /// # Arguments
    /// * `price_cents` - Price in cents (1-99)
    /// * `contracts` - Number of contracts
    ///
    /// # Returns
    /// Fee amount in cents.
    #[must_use]
    pub fn calculate_kalshi_fee(&self, price_cents: Decimal, contracts: Decimal) -> Decimal {
        let trade_value = price_cents * contracts;
        trade_value * self.config.kalshi_fee_rate
    }

    /// Calculates the trading fee for a Polymarket trade.
    ///
    /// Polymarket charges a small percentage fee on the trade value.
    ///
    /// # Arguments
    /// * `price` - Price in dollars (0.01-0.99)
    /// * `shares` - Number of shares
    ///
    /// # Returns
    /// Fee amount in dollars.
    #[must_use]
    pub fn calculate_polymarket_trading_fee(&self, price: Decimal, shares: Decimal) -> Decimal {
        let trade_value = price * shares;
        trade_value * self.config.polymarket_trading_fee_rate
    }

    /// Calculates the profit fee for a Polymarket trade.
    ///
    /// Polymarket charges 2% on the profit of winning positions.
    /// For arbitrage, we always have one winning side.
    ///
    /// # Arguments
    /// * `price` - Price paid for the winning side (0.01-0.99)
    /// * `shares` - Number of shares
    ///
    /// # Returns
    /// Profit fee amount in dollars.
    #[must_use]
    pub fn calculate_polymarket_profit_fee(&self, price: Decimal, shares: Decimal) -> Decimal {
        // Profit per share = $1 - price
        let profit_per_share = Decimal::ONE - price;
        let total_profit = profit_per_share * shares;
        total_profit * self.config.polymarket_profit_fee_rate
    }

    /// Calculates the total fees for a Polymarket trade.
    ///
    /// Includes both trading fee and expected profit fee.
    #[must_use]
    pub fn calculate_polymarket_total_fee(&self, price: Decimal, shares: Decimal) -> Decimal {
        let trading_fee = self.calculate_polymarket_trading_fee(price, shares);
        let profit_fee = self.calculate_polymarket_profit_fee(price, shares);
        let gas_fee = if self.config.include_gas_estimate {
            self.config.estimated_gas_per_trade
        } else {
            Decimal::ZERO
        };

        trading_fee + profit_fee + gas_fee
    }

    /// Calculates all fees for a cross-exchange arbitrage trade.
    ///
    /// # Arguments
    /// * `kalshi_price_cents` - Kalshi price in cents (1-99)
    /// * `kalshi_contracts` - Number of Kalshi contracts
    /// * `polymarket_price` - Polymarket price in dollars (0.01-0.99)
    /// * `polymarket_shares` - Number of Polymarket shares
    ///
    /// # Returns
    /// Detailed fee breakdown.
    #[must_use]
    pub fn calculate_arbitrage_fees(
        &self,
        kalshi_price_cents: Decimal,
        kalshi_contracts: Decimal,
        polymarket_price: Decimal,
        polymarket_shares: Decimal,
    ) -> ArbitrageFees {
        // Kalshi fee (in cents, convert to dollars)
        let kalshi_fee_cents = self.calculate_kalshi_fee(kalshi_price_cents, kalshi_contracts);
        let kalshi_fee = kalshi_fee_cents / dec!(100);

        // Polymarket trading fee
        let polymarket_trading_fee =
            self.calculate_polymarket_trading_fee(polymarket_price, polymarket_shares);

        // Polymarket profit fee (on the winning side)
        let polymarket_profit_fee =
            self.calculate_polymarket_profit_fee(polymarket_price, polymarket_shares);

        // Gas cost if enabled
        let gas_cost = if self.config.include_gas_estimate {
            // Two trades: one Polymarket buy
            self.config.estimated_gas_per_trade
        } else {
            Decimal::ZERO
        };

        let total_fee = kalshi_fee + polymarket_trading_fee + polymarket_profit_fee + gas_cost;

        ArbitrageFees {
            kalshi_fee,
            polymarket_trading_fee,
            polymarket_profit_fee,
            gas_cost,
            total_fee,
        }
    }

    /// Calculates the minimum edge required to be profitable after fees.
    ///
    /// # Arguments
    /// * `kalshi_price_cents` - Kalshi price in cents
    /// * `polymarket_price` - Polymarket price in dollars
    ///
    /// # Returns
    /// Minimum gross edge (as decimal) required to break even.
    #[must_use]
    pub fn minimum_profitable_edge(
        &self,
        kalshi_price_cents: Decimal,
        polymarket_price: Decimal,
    ) -> Decimal {
        // Kalshi fee as percentage of $1 payout
        let kalshi_fee_pct = self.config.kalshi_fee_rate * kalshi_price_cents / dec!(100);

        // Polymarket fee as percentage of $1 payout
        let poly_trading_pct = self.config.polymarket_trading_fee_rate * polymarket_price;
        let poly_profit_pct =
            self.config.polymarket_profit_fee_rate * (Decimal::ONE - polymarket_price);

        // Gas as percentage (assuming $1 payout)
        let gas_pct = if self.config.include_gas_estimate {
            self.config.estimated_gas_per_trade
        } else {
            Decimal::ZERO
        };

        kalshi_fee_pct + poly_trading_pct + poly_profit_pct + gas_pct
    }

    /// Returns the fee rate for a specific exchange.
    #[must_use]
    pub fn fee_rate_for_exchange(&self, exchange: Exchange) -> Decimal {
        match exchange {
            Exchange::Kalshi => self.config.kalshi_fee_rate,
            Exchange::Polymarket => {
                self.config.polymarket_trading_fee_rate + self.config.polymarket_profit_fee_rate
            }
        }
    }
}

impl Default for FeeCalculator {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Fee Results
// =============================================================================

/// Detailed breakdown of arbitrage fees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageFees {
    /// Kalshi trading fee in dollars.
    pub kalshi_fee: Decimal,

    /// Polymarket trading fee in dollars.
    pub polymarket_trading_fee: Decimal,

    /// Polymarket profit fee in dollars.
    pub polymarket_profit_fee: Decimal,

    /// Gas cost in dollars.
    pub gas_cost: Decimal,

    /// Total fees in dollars.
    pub total_fee: Decimal,
}

impl ArbitrageFees {
    /// Returns the total Polymarket fee (trading + profit).
    #[must_use]
    pub fn total_polymarket_fee(&self) -> Decimal {
        self.polymarket_trading_fee + self.polymarket_profit_fee
    }

    /// Returns fees as a percentage of a given trade value.
    #[must_use]
    pub fn as_percentage_of(&self, trade_value: Decimal) -> Decimal {
        if trade_value == Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.total_fee / trade_value * dec!(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== FeeConfig Tests ====================

    #[test]
    fn test_fee_config_default() {
        let config = FeeConfig::default();

        assert_eq!(config.kalshi_fee_rate, dec!(0.007));
        assert_eq!(config.polymarket_trading_fee_rate, dec!(0.0001));
        assert_eq!(config.polymarket_profit_fee_rate, dec!(0.02));
        assert!(!config.include_gas_estimate);
    }

    #[test]
    fn test_fee_config_custom() {
        let config = FeeConfig::new(dec!(0.01), dec!(0.001), dec!(0.03));

        assert_eq!(config.kalshi_fee_rate, dec!(0.01));
        assert_eq!(config.polymarket_trading_fee_rate, dec!(0.001));
        assert_eq!(config.polymarket_profit_fee_rate, dec!(0.03));
    }

    #[test]
    fn test_fee_config_with_gas() {
        let config = FeeConfig::default().with_gas_estimate(dec!(0.05));

        assert!(config.include_gas_estimate);
        assert_eq!(config.estimated_gas_per_trade, dec!(0.05));
    }

    // ==================== FeeCalculator Creation Tests ====================

    #[test]
    fn test_fee_calculator_default() {
        let calc = FeeCalculator::new();
        assert_eq!(calc.config().kalshi_fee_rate, KALSHI_DEFAULT_FEE_RATE);
    }

    #[test]
    fn test_fee_calculator_with_config() {
        let config = FeeConfig::new(dec!(0.005), dec!(0.0002), dec!(0.015));
        let calc = FeeCalculator::with_config(config);

        assert_eq!(calc.config().kalshi_fee_rate, dec!(0.005));
    }

    // ==================== Kalshi Fee Tests ====================

    #[test]
    fn test_kalshi_fee_basic() {
        let calc = FeeCalculator::new();

        // 50 cents * 100 contracts = 5000 cents = $50
        // Fee = $50 * 0.007 = $0.35 (35 cents)
        let fee = calc.calculate_kalshi_fee(dec!(50), dec!(100));
        assert_eq!(fee, dec!(35)); // 35 cents
    }

    #[test]
    fn test_kalshi_fee_zero_contracts() {
        let calc = FeeCalculator::new();
        let fee = calc.calculate_kalshi_fee(dec!(50), dec!(0));
        assert_eq!(fee, Decimal::ZERO);
    }

    #[test]
    fn test_kalshi_fee_different_prices() {
        let calc = FeeCalculator::new();

        // Low price: 20 cents * 100 = 2000 cents, fee = 14 cents
        let fee_low = calc.calculate_kalshi_fee(dec!(20), dec!(100));
        assert_eq!(fee_low, dec!(14));

        // High price: 80 cents * 100 = 8000 cents, fee = 56 cents
        let fee_high = calc.calculate_kalshi_fee(dec!(80), dec!(100));
        assert_eq!(fee_high, dec!(56));
    }

    // ==================== Polymarket Fee Tests ====================

    #[test]
    fn test_polymarket_trading_fee_basic() {
        let calc = FeeCalculator::new();

        // $0.50 * 100 shares = $50, fee = $50 * 0.0001 = $0.005
        let fee = calc.calculate_polymarket_trading_fee(dec!(0.50), dec!(100));
        assert_eq!(fee, dec!(0.005));
    }

    #[test]
    fn test_polymarket_trading_fee_zero() {
        let calc = FeeCalculator::new();
        let fee = calc.calculate_polymarket_trading_fee(dec!(0.50), dec!(0));
        assert_eq!(fee, Decimal::ZERO);
    }

    #[test]
    fn test_polymarket_profit_fee_basic() {
        let calc = FeeCalculator::new();

        // Price = $0.40, 100 shares
        // Profit per share = $1 - $0.40 = $0.60
        // Total profit = $0.60 * 100 = $60
        // Fee = $60 * 0.02 = $1.20
        let fee = calc.calculate_polymarket_profit_fee(dec!(0.40), dec!(100));
        assert_eq!(fee, dec!(1.2));
    }

    #[test]
    fn test_polymarket_profit_fee_high_price() {
        let calc = FeeCalculator::new();

        // Price = $0.90, 100 shares
        // Profit per share = $1 - $0.90 = $0.10
        // Total profit = $0.10 * 100 = $10
        // Fee = $10 * 0.02 = $0.20
        let fee = calc.calculate_polymarket_profit_fee(dec!(0.90), dec!(100));
        assert_eq!(fee, dec!(0.2));
    }

    #[test]
    fn test_polymarket_total_fee() {
        let calc = FeeCalculator::new();

        // Price = $0.50, 100 shares
        // Trading fee = $50 * 0.0001 = $0.005
        // Profit fee = $50 * 0.02 = $1.00
        // Total = $1.005
        let fee = calc.calculate_polymarket_total_fee(dec!(0.50), dec!(100));
        assert_eq!(fee, dec!(1.005));
    }

    #[test]
    fn test_polymarket_total_fee_with_gas() {
        let config = FeeConfig::default().with_gas_estimate(dec!(0.01));
        let calc = FeeCalculator::with_config(config);

        // Price = $0.50, 100 shares
        // Trading fee = $0.005
        // Profit fee = $1.00
        // Gas = $0.01
        // Total = $1.015
        let fee = calc.calculate_polymarket_total_fee(dec!(0.50), dec!(100));
        assert_eq!(fee, dec!(1.015));
    }

    // ==================== Arbitrage Fees Tests ====================

    #[test]
    fn test_arbitrage_fees_calculation() {
        let calc = FeeCalculator::new();

        // Kalshi: 46 cents, 100 contracts
        // Polymarket: $0.52, 100 shares
        let fees = calc.calculate_arbitrage_fees(dec!(46), dec!(100), dec!(0.52), dec!(100));

        // Kalshi fee: 46 * 100 * 0.007 = 32.2 cents = $0.322
        assert_eq!(fees.kalshi_fee, dec!(0.322));

        // Poly trading: 52 * 0.0001 = $0.0052
        assert_eq!(fees.polymarket_trading_fee, dec!(0.0052));

        // Poly profit: (1 - 0.52) * 100 * 0.02 = 0.48 * 100 * 0.02 = $0.96
        assert_eq!(fees.polymarket_profit_fee, dec!(0.96));

        // No gas
        assert_eq!(fees.gas_cost, Decimal::ZERO);

        // Total: 0.322 + 0.0052 + 0.96 = $1.2872
        assert_eq!(fees.total_fee, dec!(1.2872));
    }

    #[test]
    fn test_arbitrage_fees_with_gas() {
        let config = FeeConfig::default().with_gas_estimate(dec!(0.02));
        let calc = FeeCalculator::with_config(config);

        let fees = calc.calculate_arbitrage_fees(dec!(50), dec!(100), dec!(0.50), dec!(100));

        // Should include gas cost
        assert_eq!(fees.gas_cost, dec!(0.02));
        assert!(fees.total_fee > dec!(0));
    }

    // ==================== Minimum Edge Tests ====================

    #[test]
    fn test_minimum_profitable_edge() {
        let calc = FeeCalculator::new();

        // Kalshi 50 cents, Poly $0.50
        let min_edge = calc.minimum_profitable_edge(dec!(50), dec!(0.50));

        // Kalshi: 0.007 * 50 / 100 = 0.0035
        // Poly trading: 0.0001 * 0.50 = 0.00005
        // Poly profit: 0.02 * 0.50 = 0.01
        // Total: ~0.01355
        assert!(min_edge > dec!(0.01));
        assert!(min_edge < dec!(0.02));
    }

    #[test]
    fn test_minimum_profitable_edge_different_prices() {
        let calc = FeeCalculator::new();

        // Lower Kalshi price should result in lower minimum edge
        let edge_low = calc.minimum_profitable_edge(dec!(30), dec!(0.50));
        let edge_high = calc.minimum_profitable_edge(dec!(70), dec!(0.50));

        assert!(edge_low < edge_high);
    }

    // ==================== Fee Rate Tests ====================

    #[test]
    fn test_fee_rate_for_exchange() {
        let calc = FeeCalculator::new();

        assert_eq!(calc.fee_rate_for_exchange(Exchange::Kalshi), dec!(0.007));

        // Polymarket rate is trading + profit
        let poly_rate = calc.fee_rate_for_exchange(Exchange::Polymarket);
        assert_eq!(poly_rate, dec!(0.0001) + dec!(0.02));
    }

    // ==================== ArbitrageFees Tests ====================

    #[test]
    fn test_arbitrage_fees_total_polymarket() {
        let fees = ArbitrageFees {
            kalshi_fee: dec!(0.35),
            polymarket_trading_fee: dec!(0.005),
            polymarket_profit_fee: dec!(1.0),
            gas_cost: Decimal::ZERO,
            total_fee: dec!(1.355),
        };

        assert_eq!(fees.total_polymarket_fee(), dec!(1.005));
    }

    #[test]
    fn test_arbitrage_fees_as_percentage() {
        let fees = ArbitrageFees {
            kalshi_fee: dec!(0.50),
            polymarket_trading_fee: dec!(0.01),
            polymarket_profit_fee: dec!(0.50),
            gas_cost: Decimal::ZERO,
            total_fee: dec!(1.01),
        };

        // $1.01 fee on $100 trade = 1.01%
        let pct = fees.as_percentage_of(dec!(100));
        assert_eq!(pct, dec!(1.01));
    }

    #[test]
    fn test_arbitrage_fees_as_percentage_zero() {
        let fees = ArbitrageFees {
            kalshi_fee: dec!(0.50),
            polymarket_trading_fee: dec!(0.01),
            polymarket_profit_fee: dec!(0.50),
            gas_cost: Decimal::ZERO,
            total_fee: dec!(1.01),
        };

        let pct = fees.as_percentage_of(Decimal::ZERO);
        assert_eq!(pct, Decimal::ZERO);
    }

    // ==================== Serialization Tests ====================

    #[test]
    fn test_fee_config_serialization() {
        let config = FeeConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: FeeConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.kalshi_fee_rate, deserialized.kalshi_fee_rate);
    }

    #[test]
    fn test_arbitrage_fees_serialization() {
        let fees = ArbitrageFees {
            kalshi_fee: dec!(0.35),
            polymarket_trading_fee: dec!(0.005),
            polymarket_profit_fee: dec!(1.0),
            gas_cost: dec!(0.01),
            total_fee: dec!(1.365),
        };

        let json = serde_json::to_string(&fees).unwrap();
        let deserialized: ArbitrageFees = serde_json::from_str(&json).unwrap();

        assert_eq!(fees.total_fee, deserialized.total_fee);
        assert_eq!(fees.gas_cost, deserialized.gas_cost);
    }
}
