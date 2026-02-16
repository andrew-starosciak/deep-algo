"""Deterministic stop, target, and allocation rules.

Pure functions — no I/O, no side effects. Ported from crates/options-manager.
All financial math uses Decimal.
"""

from __future__ import annotations

import logging
from decimal import Decimal

from ib.types import CloseReason, ManagerConfig, OptionsPosition, StopAction

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Stop rules
# ---------------------------------------------------------------------------


def check_hard_stop(pos: OptionsPosition, config: ManagerConfig) -> StopAction | None:
    """Close if position has lost more than hard_stop_pct of cost basis."""
    loss_pct = -pos.pnl_pct()
    if loss_pct >= config.hard_stop_pct:
        logger.warning(
            "Hard stop triggered: %s pnl_pct=%.1f%% threshold=%.1f%%",
            pos.ticker, pos.pnl_pct(), config.hard_stop_pct,
        )
        return StopAction(close_all=True, reason=CloseReason.HARD_STOP)
    return None


def check_time_stop(pos: OptionsPosition, config: ManagerConfig) -> StopAction | None:
    """Close losing positions within time_stop_dte days of expiry."""
    dte = pos.days_to_expiry()
    is_losing = pos.unrealized_pnl < 0

    if dte <= config.time_stop_dte and is_losing:
        logger.warning(
            "Time stop triggered: %s dte=%d pnl_pct=%.1f%%",
            pos.ticker, dte, pos.pnl_pct(),
        )
        return StopAction(close_all=True, reason=CloseReason.TIME_STOP)
    return None


def check_stop_rules(pos: OptionsPosition, config: ManagerConfig) -> StopAction | None:
    """Run all stop checks in priority order. Returns first triggered action."""
    action = check_hard_stop(pos, config)
    if action is not None:
        return action

    action = check_time_stop(pos, config)
    if action is not None:
        return action

    return None


# ---------------------------------------------------------------------------
# Profit targets
# ---------------------------------------------------------------------------


def check_profit_targets(pos: OptionsPosition, config: ManagerConfig) -> StopAction | None:
    """Mechanical profit-taking ladder.

    Target 2 (+100%): close all remaining.
    Target 1 (+50%): sell half if quantity > 1.
    """
    pnl_pct = pos.pnl_pct()

    # Target 2 checked first (higher priority)
    if pnl_pct >= config.profit_target_2_pct:
        logger.info(
            "Profit target 2 hit: %s pnl_pct=%.1f%% — closing remaining",
            pos.ticker, pnl_pct,
        )
        return StopAction(close_all=True, reason=CloseReason.PROFIT_TARGET)

    # Target 1: sell half
    if pnl_pct >= config.profit_target_1_pct and pos.quantity > 1:
        half = pos.quantity // 2
        if half > 0:
            logger.info(
                "Profit target 1 hit: %s pnl_pct=%.1f%% — selling %d of %d",
                pos.ticker, pnl_pct, half, pos.quantity,
            )
            return StopAction(close_all=False, quantity=half, reason=CloseReason.PROFIT_TARGET)

    return None


# ---------------------------------------------------------------------------
# Allocation
# ---------------------------------------------------------------------------


def check_allocation(
    new_position_usd: Decimal,
    current_options_total_usd: Decimal,
    account_equity: Decimal,
    config: ManagerConfig,
) -> tuple[bool, str]:
    """Check if a new position would exceed allocation limits.

    Returns (approved, reason_string).
    """
    if account_equity <= 0:
        return (False, "Rejected: account equity is zero or negative")

    max_allowed = account_equity * config.max_allocation_pct / Decimal("100")
    after_trade = current_options_total_usd + new_position_usd

    if after_trade > max_allowed:
        current_pct = (current_options_total_usd / account_equity) * 100
        would_be_pct = (after_trade / account_equity) * 100
        return (
            False,
            f"Rejected: would be {would_be_pct:.1f}% (current {current_pct:.1f}%, "
            f"max {config.max_allocation_pct}%)",
        )

    remaining = max_allowed - after_trade
    utilization = (after_trade / max_allowed) * 100 if max_allowed else Decimal("0")
    return (
        True,
        f"Approved: {utilization:.1f}% utilized, ${remaining:.0f} remaining capacity",
    )
