//! Hard stop and time stop rules.

use rust_decimal::Decimal;

use crate::types::{CloseReason, ManagerConfig, OptionsPosition, StopAction};

/// Check all stop rules against a position. Returns an action if triggered.
pub fn check_stop_rules(pos: &OptionsPosition, config: &ManagerConfig) -> Option<StopAction> {
    // 1. Hard stop: position lost more than threshold
    if let Some(action) = check_hard_stop(pos, config) {
        return Some(action);
    }

    // 2. Time stop: losing position close to expiry
    if let Some(action) = check_time_stop(pos, config) {
        return Some(action);
    }

    None
}

/// Close if position has lost more than `hard_stop_pct` of cost basis.
fn check_hard_stop(pos: &OptionsPosition, config: &ManagerConfig) -> Option<StopAction> {
    let loss_pct = -pos.pnl_pct();
    if loss_pct >= config.hard_stop_pct {
        tracing::warn!(
            ticker = pos.ticker,
            pnl_pct = %pos.pnl_pct(),
            threshold = %config.hard_stop_pct,
            "Hard stop triggered"
        );
        return Some(StopAction::CloseAll {
            reason: CloseReason::HardStop,
        });
    }
    None
}

/// Close losing positions within `time_stop_dte` days of expiry.
fn check_time_stop(pos: &OptionsPosition, config: &ManagerConfig) -> Option<StopAction> {
    let dte = pos.days_to_expiry();
    let is_losing = pos.unrealized_pnl < Decimal::ZERO;

    if dte <= config.time_stop_dte && is_losing {
        tracing::warn!(
            ticker = pos.ticker,
            dte,
            pnl_pct = %pos.pnl_pct(),
            "Time stop triggered — losing position near expiry"
        );
        return Some(StopAction::CloseAll {
            reason: CloseReason::TimeStop,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};
    use rust_decimal_macros::dec;

    fn make_position(pnl: Decimal, cost_basis: Decimal, dte_days: i64) -> OptionsPosition {
        let expiry = Utc::now().date_naive() + chrono::Duration::days(dte_days);
        OptionsPosition {
            id: 1,
            recommendation_id: 1,
            ticker: "NVDA".to_string(),
            right: "call".to_string(),
            strike: dec!(140),
            expiry,
            quantity: 1,
            avg_fill_price: dec!(9.00),
            current_price: dec!(9.00) + pnl / dec!(100),
            cost_basis,
            unrealized_pnl: pnl,
            realized_pnl: dec!(0),
            status: "open".to_string(),
            opened_at: Utc::now(),
        }
    }

    #[test]
    fn hard_stop_triggers_at_threshold() {
        let config = ManagerConfig::default();
        // Position lost 60% — should trigger (threshold is 50%)
        let pos = make_position(dec!(-600), dec!(1000), 30);
        let action = check_stop_rules(&pos, &config);
        assert!(matches!(
            action,
            Some(StopAction::CloseAll {
                reason: CloseReason::HardStop
            })
        ));
    }

    #[test]
    fn hard_stop_does_not_trigger_below_threshold() {
        let config = ManagerConfig::default();
        // Position lost 30% — should NOT trigger
        let pos = make_position(dec!(-300), dec!(1000), 30);
        let action = check_stop_rules(&pos, &config);
        assert!(action.is_none());
    }

    #[test]
    fn time_stop_triggers_near_expiry_and_losing() {
        let config = ManagerConfig::default();
        // 5 DTE and losing — should trigger (threshold is 7 DTE)
        let pos = make_position(dec!(-100), dec!(1000), 5);
        let action = check_stop_rules(&pos, &config);
        assert!(matches!(
            action,
            Some(StopAction::CloseAll {
                reason: CloseReason::TimeStop
            })
        ));
    }

    #[test]
    fn time_stop_does_not_trigger_when_winning() {
        let config = ManagerConfig::default();
        // 5 DTE but winning — should NOT trigger
        let pos = make_position(dec!(200), dec!(1000), 5);
        let action = check_stop_rules(&pos, &config);
        assert!(action.is_none());
    }
}
