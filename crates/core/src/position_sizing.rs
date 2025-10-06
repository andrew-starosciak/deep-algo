use anyhow::Result;
use rust_decimal::Decimal;

/// Calculate position size with leverage support
///
/// # Arguments
/// * `account_equity` - Current account equity in USDC
/// * `leverage` - Leverage multiplier (1-50 for Hyperliquid)
/// * `risk_per_trade_pct` - Percentage of equity to risk (0.0-1.0)
/// * `max_position_pct` - Maximum position as percentage of equity (0.0-1.0)
/// * `entry_price` - Entry price for the asset
///
/// # Returns
/// Token quantity to purchase
///
/// # Errors
/// Returns error if parameters are invalid
pub fn calculate_position_size(
    account_equity: Decimal,
    leverage: u8,
    risk_per_trade_pct: f64,
    max_position_pct: f64,
    entry_price: Decimal,
) -> Result<Decimal> {
    if leverage == 0 || leverage > 50 {
        anyhow::bail!("Leverage must be between 1 and 50");
    }

    if entry_price <= Decimal::ZERO {
        anyhow::bail!("Entry price must be positive");
    }

    // Calculate position value with leverage
    let leverage_dec = Decimal::from(leverage);
    let risk_pct = Decimal::try_from(risk_per_trade_pct)?;
    let max_pct = Decimal::try_from(max_position_pct)?;

    // Position value = equity × leverage × risk_per_trade_pct
    let position_value = account_equity * leverage_dec * risk_pct;

    // Cap at max_position_pct of equity
    let max_position_value = account_equity * max_pct;
    let final_position_value = position_value.min(max_position_value);

    // Convert to token quantity
    let quantity = final_position_value / entry_price;

    Ok(quantity)
}

/// Calculate required margin for a position
///
/// # Arguments
/// * `position_value` - Total position value in USDC
/// * `leverage` - Leverage multiplier
///
/// # Returns
/// Required margin in USDC
#[must_use]
pub fn calculate_required_margin(position_value: Decimal, leverage: u8) -> Decimal {
    if leverage == 0 {
        return position_value;
    }
    position_value / Decimal::from(leverage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_basic_position_sizing() {
        let quantity = calculate_position_size(
            dec!(10000), // $10k equity
            1,           // 1x leverage (no leverage)
            0.05,        // 5% risk
            0.20,        // 20% max position
            dec!(100),   // $100 entry price
        )
        .unwrap();

        // Expected: $10k × 1 × 0.05 = $500 position → $500/$100 = 5.0 tokens
        assert_eq!(quantity, dec!(5.0));
    }

    #[test]
    fn test_with_leverage() {
        let quantity = calculate_position_size(
            dec!(10000), // $10k equity
            5,           // 5x leverage
            0.05,        // 5% risk
            0.50,        // 50% max position (high enough to not cap)
            dec!(100),   // $100 entry price
        )
        .unwrap();

        // Expected: $10k × 5 × 0.05 = $2500 position → $2500/$100 = 25.0 tokens
        // Max cap: $10k × 0.50 = $5000 (doesn't cap)
        assert_eq!(quantity, dec!(25.0));
    }

    #[test]
    fn test_max_position_cap() {
        let quantity = calculate_position_size(
            dec!(10000), // $10k equity
            10,          // 10x leverage
            0.50,        // 50% risk (aggressive)
            0.20,        // 20% max position (caps at $2k)
            dec!(100),   // $100 entry price
        )
        .unwrap();

        // Without cap: $10k × 10 × 0.50 = $50k
        // With cap: min($50k, $10k × 0.20) = $2k → $2k/$100 = 20.0 tokens
        assert_eq!(quantity, dec!(20.0));
    }

    #[test]
    fn test_required_margin() {
        let margin = calculate_required_margin(dec!(5000), 5);
        // $5000 position with 5x leverage = $1000 margin
        assert_eq!(margin, dec!(1000));
    }

    #[test]
    fn test_invalid_leverage() {
        let result = calculate_position_size(dec!(10000), 0, 0.05, 0.20, dec!(100));
        assert!(result.is_err());

        let result = calculate_position_size(dec!(10000), 51, 0.05, 0.20, dec!(100));
        assert!(result.is_err());
    }
}
