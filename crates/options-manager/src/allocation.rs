//! Capital allocation enforcement.
//!
//! Ensures total swing options exposure stays within limits.

use anyhow::Result;
use rust_decimal::Decimal;

use crate::types::ManagerConfig;

/// Check if a new position would exceed allocation limits.
pub fn check_allocation(
    new_position_usd: Decimal,
    current_options_total_usd: Decimal,
    account_equity: Decimal,
    config: &ManagerConfig,
) -> AllocationCheck {
    let max_allowed = account_equity * config.max_allocation_pct / Decimal::from(100);
    let after_trade = current_options_total_usd + new_position_usd;

    if after_trade > max_allowed {
        AllocationCheck::Rejected {
            current_pct: (current_options_total_usd / account_equity) * Decimal::from(100),
            would_be_pct: (after_trade / account_equity) * Decimal::from(100),
            max_pct: config.max_allocation_pct,
        }
    } else {
        AllocationCheck::Approved {
            remaining_capacity: max_allowed - after_trade,
            utilization_pct: (after_trade / max_allowed) * Decimal::from(100),
        }
    }
}

/// Result of an allocation check.
#[derive(Debug)]
pub enum AllocationCheck {
    Approved {
        remaining_capacity: Decimal,
        utilization_pct: Decimal,
    },
    Rejected {
        current_pct: Decimal,
        would_be_pct: Decimal,
        max_pct: Decimal,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn approves_within_limit() {
        let config = ManagerConfig::default(); // 10% max
        let result = check_allocation(
            dec!(2000),   // New position
            dec!(5000),   // Current total
            dec!(200000), // Account equity
        &config,
        );
        assert!(matches!(result, AllocationCheck::Approved { .. }));
    }

    #[test]
    fn rejects_over_limit() {
        let config = ManagerConfig::default(); // 10% max = $20k on $200k
        let result = check_allocation(
            dec!(5000),   // New position
            dec!(18000),  // Current total ($23k total > $20k limit)
            dec!(200000), // Account equity
            &config,
        );
        assert!(matches!(result, AllocationCheck::Rejected { .. }));
    }
}
