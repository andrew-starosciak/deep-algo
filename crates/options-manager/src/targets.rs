//! Profit target rules — mechanical profit-taking ladder.

use rust_decimal::Decimal;

use crate::types::{CloseReason, ManagerConfig, OptionsPosition, StopAction};

/// Check profit target rules. Returns an action if a target is hit.
pub fn check_profit_targets(pos: &OptionsPosition, config: &ManagerConfig) -> Option<StopAction> {
    let pnl_pct = pos.pnl_pct();

    // Target 2: +100% → sell remaining
    if pnl_pct >= config.profit_target_2_pct {
        tracing::info!(
            ticker = pos.ticker,
            pnl_pct = %pnl_pct,
            "Profit target 2 hit — closing remaining"
        );
        return Some(StopAction::CloseAll {
            reason: CloseReason::ProfitTarget,
        });
    }

    // Target 1: +50% → sell half
    if pnl_pct >= config.profit_target_1_pct && pos.quantity > 1 {
        let half = pos.quantity / 2;
        if half > 0 {
            tracing::info!(
                ticker = pos.ticker,
                pnl_pct = %pnl_pct,
                sell_quantity = half,
                "Profit target 1 hit — selling half"
            );
            return Some(StopAction::ClosePartial {
                quantity: half,
                reason: CloseReason::ProfitTarget,
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    fn make_position(pnl: Decimal, cost_basis: Decimal, quantity: i32) -> OptionsPosition {
        let expiry = Utc::now().date_naive() + chrono::Duration::days(30);
        OptionsPosition {
            id: 1,
            recommendation_id: 1,
            ticker: "AAPL".to_string(),
            right: "call".to_string(),
            strike: dec!(180),
            expiry,
            quantity,
            avg_fill_price: dec!(5.00),
            current_price: dec!(5.00),
            cost_basis,
            unrealized_pnl: pnl,
            realized_pnl: dec!(0),
            status: "open".to_string(),
            opened_at: Utc::now(),
        }
    }

    #[test]
    fn profit_target_1_sells_half() {
        let config = ManagerConfig::default();
        // +60% gain, 4 contracts → should sell 2
        let pos = make_position(dec!(600), dec!(1000), 4);
        let action = check_profit_targets(&pos, &config);
        assert!(matches!(
            action,
            Some(StopAction::ClosePartial {
                quantity: 2,
                reason: CloseReason::ProfitTarget,
            })
        ));
    }

    #[test]
    fn profit_target_2_closes_all() {
        let config = ManagerConfig::default();
        // +120% gain → should close all
        let pos = make_position(dec!(1200), dec!(1000), 2);
        let action = check_profit_targets(&pos, &config);
        assert!(matches!(
            action,
            Some(StopAction::CloseAll {
                reason: CloseReason::ProfitTarget,
            })
        ));
    }

    #[test]
    fn no_target_hit_below_threshold() {
        let config = ManagerConfig::default();
        // +30% gain — no target hit
        let pos = make_position(dec!(300), dec!(1000), 4);
        let action = check_profit_targets(&pos, &config);
        assert!(action.is_none());
    }
}
