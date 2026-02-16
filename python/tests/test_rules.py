"""Tests for deterministic stop, target, and allocation rules.

Ported from crates/options-manager/src/{stops,targets,allocation}.rs tests.
"""

from __future__ import annotations

import datetime as _dt
from decimal import Decimal

from ib.rules import check_allocation, check_profit_targets, check_stop_rules
from ib.types import CloseReason, ManagerConfig, OptionsPosition


def _make_position(
    pnl: Decimal,
    cost_basis: Decimal,
    dte_days: int = 30,
    quantity: int = 1,
    ticker: str = "NVDA",
) -> OptionsPosition:
    """Build a test position with given P&L and DTE."""
    expiry = _dt.date.today() + _dt.timedelta(days=dte_days)
    return OptionsPosition(
        id=1,
        recommendation_id=1,
        ticker=ticker,
        right="call",
        strike=Decimal("140"),
        expiry=expiry,
        quantity=quantity,
        avg_fill_price=Decimal("9.00"),
        current_price=Decimal("9.00"),
        cost_basis=cost_basis,
        unrealized_pnl=pnl,
    )


# ---------------------------------------------------------------------------
# Stop tests (4 from Rust)
# ---------------------------------------------------------------------------


def test_hard_stop_triggers_at_threshold():
    config = ManagerConfig()
    # Lost 60% — should trigger (threshold is 50%)
    pos = _make_position(pnl=Decimal("-600"), cost_basis=Decimal("1000"), dte_days=30)
    action = check_stop_rules(pos, config)
    assert action is not None
    assert action.close_all is True
    assert action.reason == CloseReason.HARD_STOP


def test_hard_stop_does_not_trigger_below_threshold():
    config = ManagerConfig()
    # Lost 30% — should NOT trigger
    pos = _make_position(pnl=Decimal("-300"), cost_basis=Decimal("1000"), dte_days=30)
    action = check_stop_rules(pos, config)
    assert action is None


def test_time_stop_triggers_near_expiry_and_losing():
    config = ManagerConfig()
    # 5 DTE and losing — should trigger (threshold is 7 DTE)
    pos = _make_position(pnl=Decimal("-100"), cost_basis=Decimal("1000"), dte_days=5)
    action = check_stop_rules(pos, config)
    assert action is not None
    assert action.close_all is True
    assert action.reason == CloseReason.TIME_STOP


def test_time_stop_does_not_trigger_when_winning():
    config = ManagerConfig()
    # 5 DTE but winning — should NOT trigger
    pos = _make_position(pnl=Decimal("200"), cost_basis=Decimal("1000"), dte_days=5)
    action = check_stop_rules(pos, config)
    assert action is None


# ---------------------------------------------------------------------------
# Profit target tests (3 from Rust)
# ---------------------------------------------------------------------------


def test_profit_target_1_sells_half():
    config = ManagerConfig()
    # +60% gain, 4 contracts → should sell 2
    pos = _make_position(
        pnl=Decimal("600"), cost_basis=Decimal("1000"), quantity=4, ticker="AAPL"
    )
    action = check_profit_targets(pos, config)
    assert action is not None
    assert action.close_all is False
    assert action.quantity == 2
    assert action.reason == CloseReason.PROFIT_TARGET


def test_profit_target_2_closes_all():
    config = ManagerConfig()
    # +120% gain → should close all
    pos = _make_position(
        pnl=Decimal("1200"), cost_basis=Decimal("1000"), quantity=2, ticker="AAPL"
    )
    action = check_profit_targets(pos, config)
    assert action is not None
    assert action.close_all is True
    assert action.reason == CloseReason.PROFIT_TARGET


def test_no_target_hit_below_threshold():
    config = ManagerConfig()
    # +30% gain — no target hit
    pos = _make_position(
        pnl=Decimal("300"), cost_basis=Decimal("1000"), quantity=4, ticker="AAPL"
    )
    action = check_profit_targets(pos, config)
    assert action is None


# ---------------------------------------------------------------------------
# Allocation tests (2 from Rust)
# ---------------------------------------------------------------------------


def test_allocation_approves_within_limit():
    config = ManagerConfig()  # 10% max
    approved, reason = check_allocation(
        new_position_usd=Decimal("2000"),
        current_options_total_usd=Decimal("5000"),
        account_equity=Decimal("200000"),
        config=config,
    )
    assert approved is True
    assert "Approved" in reason


def test_allocation_rejects_over_limit():
    config = ManagerConfig()  # 10% max = $20k on $200k
    approved, reason = check_allocation(
        new_position_usd=Decimal("5000"),
        current_options_total_usd=Decimal("18000"),  # $23k total > $20k limit
        account_equity=Decimal("200000"),
        config=config,
    )
    assert approved is False
    assert "Rejected" in reason
