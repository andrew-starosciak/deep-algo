"""Tests for PositionManager with mocked DB and IB client.

Covers: tick cycle, recommendation execution, allocation rejection,
hard stop, time stop, profit target 1 (partial), profit target 2 (full),
zero-quote rejection, notifier integration, and position sync.
"""

from __future__ import annotations

import datetime as _dt
from decimal import Decimal
from unittest.mock import AsyncMock, call

import pytest

from ib.position_manager import PositionManager
from ib.types import AccountSummary, Fill, IBPortfolioItem, ManagerConfig, OptionQuote

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


def _make_quote(mid: Decimal) -> OptionQuote:
    spread = mid * Decimal("0.05")
    return OptionQuote(
        bid=mid - spread, ask=mid + spread, last=mid, mid=mid,
        volume=100, open_interest=500,
    )


def _make_fill(side: str, qty: int, price: Decimal, order_id: int = 1) -> Fill:
    return Fill(
        order_id=order_id, symbol="NVDA", side=side, quantity=qty,
        avg_fill_price=price, commission=Decimal("0.65") * qty,
        filled_at=_dt.datetime.now(_dt.UTC),
    )


def _make_rec(
    rec_id: int = 1,
    ticker: str = "NVDA",
    strike: Decimal = Decimal("140"),
    position_size_usd: Decimal = Decimal("2000"),
) -> dict:
    return {
        "id": rec_id,
        "ticker": ticker,
        "right": "call",
        "strike": strike,
        "expiry": _dt.date.today() + _dt.timedelta(days=30),
        "entry_price_low": Decimal("8.00"),
        "entry_price_high": Decimal("10.00"),
        "position_size_usd": position_size_usd,
    }


def _make_position(
    pos_id: int = 1,
    ticker: str = "NVDA",
    pnl: Decimal = Decimal("0"),
    cost_basis: Decimal = Decimal("1800"),
    quantity: int = 2,
    avg_fill_price: Decimal = Decimal("9.00"),
    dte_days: int = 30,
    ib_con_id: int | None = None,
) -> dict:
    """Return a position dict as it would come from the DB."""
    return {
        "id": pos_id,
        "recommendation_id": 1,
        "ticker": ticker,
        "right": "call",
        "strike": Decimal("140"),
        "expiry": _dt.date.today() + _dt.timedelta(days=dte_days),
        "quantity": quantity,
        "avg_fill_price": avg_fill_price,
        "current_price": avg_fill_price,
        "cost_basis": cost_basis,
        "unrealized_pnl": pnl,
        "realized_pnl": Decimal("0"),
        "status": "open",
        "ib_con_id": ib_con_id,
        "opened_at": _dt.datetime.now(_dt.UTC),
    }


def _make_ib_portfolio_item(
    con_id: int = 12345,
    symbol: str = "NVDA",
    right: str = "call",
    strike: Decimal = Decimal("140"),
    position: int = 2,
    avg_cost: Decimal = Decimal("9.00"),
    market_price: Decimal = Decimal("10.00"),
    unrealized_pnl: Decimal = Decimal("200"),
) -> IBPortfolioItem:
    return IBPortfolioItem(
        con_id=con_id,
        symbol=symbol,
        sec_type="OPT",
        right=right,
        strike=strike,
        expiry=_dt.date.today() + _dt.timedelta(days=30),
        position=position,
        avg_cost=avg_cost,
        market_price=market_price,
        market_value=market_price * position * 100,
        unrealized_pnl=unrealized_pnl,
        realized_pnl=Decimal("0"),
        account="U1234567",
    )


@pytest.fixture
def mock_db():
    db = AsyncMock()
    db.get_approved_recommendations.return_value = []
    db.get_open_positions.return_value = []
    db.get_total_options_exposure.return_value = Decimal("0")
    db.insert_position.return_value = 1
    return db


@pytest.fixture
def mock_ib():
    ib = AsyncMock()
    ib.account_summary.return_value = AccountSummary(
        net_liquidation=Decimal("200000"),
        buying_power=Decimal("800000"),
        available_funds=Decimal("200000"),
    )
    ib.portfolio.return_value = []
    return ib


@pytest.fixture
def config():
    return ManagerConfig()


@pytest.fixture
def manager(mock_db, mock_ib, config):
    return PositionManager(db=mock_db, ib_client=mock_ib, config=config)


# ---------------------------------------------------------------------------
# Tick cycle
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_tick_empty_db(manager, mock_db):
    """Tick with no approved recs and no open positions is a no-op."""
    await manager._tick()

    mock_db.get_approved_recommendations.assert_awaited_once()
    # get_open_positions called twice: once by _sync_positions, once by _tick
    assert mock_db.get_open_positions.await_count == 2
    mock_db.insert_position.assert_not_awaited()
    mock_db.close_position.assert_not_awaited()


# ---------------------------------------------------------------------------
# Recommendation execution
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_execute_recommendation_success(manager, mock_db, mock_ib):
    """Happy path: approved rec → quote → order → position inserted → status filled."""
    rec = _make_rec()
    mock_db.get_approved_recommendations.return_value = [rec]

    mid = Decimal("9.00")
    mock_ib.get_option_quote.return_value = _make_quote(mid)
    mock_ib.place_order.return_value = _make_fill("BUY", 2, mid)

    await manager._tick()

    # Status transitions: executing → filled
    assert mock_db.update_recommendation_status.await_count == 2
    calls = mock_db.update_recommendation_status.await_args_list
    assert calls[0] == call(1, "executing")
    assert calls[1] == call(1, "filled")

    # Position inserted with correct values
    mock_db.insert_position.assert_awaited_once()
    pos = mock_db.insert_position.await_args[0][0]
    assert pos["ticker"] == "NVDA"
    assert pos["quantity"] == 2
    assert pos["avg_fill_price"] == mid
    assert pos["cost_basis"] == mid * 2 * 100  # 9.00 * 2 * 100 = 1800


@pytest.mark.asyncio
async def test_execute_recommendation_allocation_rejected(manager, mock_db, mock_ib):
    """Rec rejected when allocation limit would be exceeded."""
    rec = _make_rec(position_size_usd=Decimal("15000"))
    mock_db.get_approved_recommendations.return_value = [rec]
    # Current exposure already at $8000, new $15000 → $23000 > $20000 (10% of $200k)
    mock_db.get_total_options_exposure.return_value = Decimal("8000")

    await manager._tick()

    # Should be marked failed, not filled
    calls = mock_db.update_recommendation_status.await_args_list
    assert calls[-1][0][1] == "failed"
    assert "Rejected" in calls[-1][0][2]
    mock_ib.place_order.assert_not_awaited()


@pytest.mark.asyncio
async def test_execute_recommendation_zero_quote(manager, mock_db, mock_ib):
    """Rec rejected when IB returns a zero mid price."""
    rec = _make_rec()
    mock_db.get_approved_recommendations.return_value = [rec]
    mock_ib.get_option_quote.return_value = _make_quote(Decimal("0"))

    await manager._tick()

    calls = mock_db.update_recommendation_status.await_args_list
    assert calls[-1][0][1] == "failed"
    assert "Zero" in calls[-1][0][2]
    mock_ib.place_order.assert_not_awaited()


@pytest.mark.asyncio
async def test_execute_recommendation_order_failure(manager, mock_db, mock_ib):
    """Rec marked failed when place_order raises."""
    rec = _make_rec()
    mock_db.get_approved_recommendations.return_value = [rec]
    mock_ib.get_option_quote.return_value = _make_quote(Decimal("9.00"))
    mock_ib.place_order.side_effect = RuntimeError("Order rejected by exchange")

    await manager._tick()

    calls = mock_db.update_recommendation_status.await_args_list
    assert calls[-1][0][1] == "failed"
    assert "Order rejected" in calls[-1][0][2]


# ---------------------------------------------------------------------------
# Hard stop
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_hard_stop_closes_position(manager, mock_db, mock_ib):
    """Position at -60% triggers hard stop → close all."""
    pos = _make_position(pnl=Decimal("-1080"), cost_basis=Decimal("1800"))
    mock_db.get_open_positions.return_value = [pos]

    # Quote returns a low price (doesn't matter, pnl comes from DB values
    # but _update_and_check recalculates from quote)
    new_mid = Decimal("3.60")  # 9.00 → 3.60 = -60% per contract
    mock_ib.get_option_quote.return_value = _make_quote(new_mid)
    mock_ib.place_order.return_value = _make_fill("SELL", 2, new_mid)

    await manager._tick()

    mock_ib.place_order.assert_awaited_once()
    sell_call = mock_ib.place_order.await_args
    assert sell_call.kwargs["side"] == "SELL"
    assert sell_call.kwargs["quantity"] == 2
    assert sell_call.kwargs["order_type"] == "MKT"

    mock_db.close_position.assert_awaited_once()
    close_args = mock_db.close_position.await_args[0]
    assert close_args[0] == 1  # position id
    assert close_args[1] == "stop_loss"


# ---------------------------------------------------------------------------
# Time stop
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_time_stop_closes_losing_position_near_expiry(manager, mock_db, mock_ib):
    """Losing position at 5 DTE triggers time stop."""
    pos = _make_position(pnl=Decimal("-200"), cost_basis=Decimal("1800"), dte_days=5)
    mock_db.get_open_positions.return_value = [pos]

    # Quote still shows a loss
    new_mid = Decimal("8.00")  # 9.00 → 8.00 = losing
    mock_ib.get_option_quote.return_value = _make_quote(new_mid)
    mock_ib.place_order.return_value = _make_fill("SELL", 2, new_mid)

    await manager._tick()

    mock_db.close_position.assert_awaited_once()
    close_args = mock_db.close_position.await_args[0]
    assert close_args[1] == "time_stop"


@pytest.mark.asyncio
async def test_time_stop_does_not_trigger_when_profitable(manager, mock_db, mock_ib):
    """Winning position at 5 DTE should NOT trigger time stop."""
    pos = _make_position(pnl=Decimal("200"), cost_basis=Decimal("1800"), dte_days=5)
    mock_db.get_open_positions.return_value = [pos]

    new_mid = Decimal("10.00")  # 9.00 → 10.00 = profitable
    mock_ib.get_option_quote.return_value = _make_quote(new_mid)

    await manager._tick()

    mock_db.close_position.assert_not_awaited()
    mock_db.partial_close_position.assert_not_awaited()
    mock_ib.place_order.assert_not_awaited()


# ---------------------------------------------------------------------------
# Profit targets
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_profit_target_1_partial_close(manager, mock_db, mock_ib):
    """At +60% with 4 contracts, sells half (2)."""
    pos = _make_position(
        pnl=Decimal("2160"), cost_basis=Decimal("3600"), quantity=4,
        avg_fill_price=Decimal("9.00"),
    )
    mock_db.get_open_positions.return_value = [pos]

    new_mid = Decimal("14.40")  # 9.00 → 14.40 = +60%
    mock_ib.get_option_quote.return_value = _make_quote(new_mid)
    mock_ib.place_order.return_value = _make_fill("SELL", 2, new_mid)

    await manager._tick()

    # Should sell 2 (half of 4)
    sell_call = mock_ib.place_order.await_args
    assert sell_call.kwargs["quantity"] == 2
    assert sell_call.kwargs["side"] == "SELL"

    # Should call partial_close_position, NOT close_position
    mock_db.partial_close_position.assert_awaited_once()
    partial_args = mock_db.partial_close_position.await_args[0]
    assert partial_args[0] == 1  # position id
    assert partial_args[1] == 2  # remaining quantity
    mock_db.close_position.assert_not_awaited()


@pytest.mark.asyncio
async def test_profit_target_2_full_close(manager, mock_db, mock_ib):
    """At +120%, closes all remaining."""
    pos = _make_position(
        pnl=Decimal("2160"), cost_basis=Decimal("1800"), quantity=2,
        avg_fill_price=Decimal("9.00"),
    )
    mock_db.get_open_positions.return_value = [pos]

    new_mid = Decimal("19.80")  # 9.00 → 19.80 = +120%
    mock_ib.get_option_quote.return_value = _make_quote(new_mid)
    mock_ib.place_order.return_value = _make_fill("SELL", 2, new_mid)

    await manager._tick()

    mock_db.close_position.assert_awaited_once()
    close_args = mock_db.close_position.await_args[0]
    assert close_args[1] == "profit_target"
    mock_db.partial_close_position.assert_not_awaited()


# ---------------------------------------------------------------------------
# Price update
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_price_update_no_action(manager, mock_db, mock_ib):
    """Position with small gain — updates price but takes no action."""
    pos = _make_position(pnl=Decimal("100"), cost_basis=Decimal("1800"))
    mock_db.get_open_positions.return_value = [pos]

    new_mid = Decimal("9.50")  # small gain, no threshold hit
    mock_ib.get_option_quote.return_value = _make_quote(new_mid)

    await manager._tick()

    # Price should be updated in DB
    mock_db.update_position_price.assert_awaited_once()
    price_args = mock_db.update_position_price.await_args[0]
    assert price_args[0] == 1  # position id
    assert price_args[1] == new_mid

    # No close actions
    mock_db.close_position.assert_not_awaited()
    mock_db.partial_close_position.assert_not_awaited()
    mock_ib.place_order.assert_not_awaited()


@pytest.mark.asyncio
async def test_quote_failure_skips_price_update(manager, mock_db, mock_ib):
    """If quote fails, position is not closed (uses stale values)."""
    pos = _make_position(pnl=Decimal("100"), cost_basis=Decimal("1800"))
    mock_db.get_open_positions.return_value = [pos]
    mock_ib.get_option_quote.side_effect = ValueError("No quote available")

    await manager._tick()

    mock_db.update_position_price.assert_not_awaited()
    mock_db.close_position.assert_not_awaited()
    mock_ib.place_order.assert_not_awaited()


# ---------------------------------------------------------------------------
# Notifier
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_notifier_called_on_fill(mock_db, mock_ib, config):
    """Telegram notifier is called when a recommendation fills."""
    notifier = AsyncMock()
    manager = PositionManager(db=mock_db, ib_client=mock_ib, config=config, notifier=notifier)

    rec = _make_rec()
    mock_db.get_approved_recommendations.return_value = [rec]
    mock_ib.get_option_quote.return_value = _make_quote(Decimal("9.00"))
    mock_ib.place_order.return_value = _make_fill("BUY", 2, Decimal("9.00"))

    await manager._tick()

    notifier.send.assert_awaited_once()
    msg = notifier.send.await_args[0][0]
    assert "Filled" in msg
    assert "NVDA" in msg


@pytest.mark.asyncio
async def test_notifier_called_on_close(mock_db, mock_ib, config):
    """Telegram notifier is called when a position is closed."""
    notifier = AsyncMock()
    manager = PositionManager(db=mock_db, ib_client=mock_ib, config=config, notifier=notifier)

    pos = _make_position(pnl=Decimal("-1080"), cost_basis=Decimal("1800"))
    mock_db.get_open_positions.return_value = [pos]

    new_mid = Decimal("3.60")
    mock_ib.get_option_quote.return_value = _make_quote(new_mid)
    mock_ib.place_order.return_value = _make_fill("SELL", 2, new_mid)

    await manager._tick()

    notifier.send.assert_awaited_once()
    msg = notifier.send.await_args[0][0]
    assert "Closed" in msg
    assert "stop_loss" in msg


@pytest.mark.asyncio
async def test_no_notifier_is_fine(mock_db, mock_ib, config):
    """Manager works without a notifier (notifier=None)."""
    manager = PositionManager(db=mock_db, ib_client=mock_ib, config=config, notifier=None)

    rec = _make_rec()
    mock_db.get_approved_recommendations.return_value = [rec]
    mock_ib.get_option_quote.return_value = _make_quote(Decimal("9.00"))
    mock_ib.place_order.return_value = _make_fill("BUY", 2, Decimal("9.00"))

    # Should not raise
    await manager._tick()
    mock_db.update_recommendation_status.assert_awaited()


# ---------------------------------------------------------------------------
# Multiple positions in one tick
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_tick_processes_multiple_positions(manager, mock_db, mock_ib):
    """Multiple open positions are each checked independently."""
    pos_ok = _make_position(pos_id=1, pnl=Decimal("100"), cost_basis=Decimal("1800"))
    pos_stop = _make_position(
        pos_id=2, ticker="AAPL", pnl=Decimal("-1080"), cost_basis=Decimal("1800"),
    )
    mock_db.get_open_positions.return_value = [pos_ok, pos_stop]

    # First position gets a healthy quote, second gets a bad one
    healthy_quote = _make_quote(Decimal("9.50"))
    losing_quote = _make_quote(Decimal("3.60"))
    mock_ib.get_option_quote.side_effect = [healthy_quote, losing_quote]
    mock_ib.place_order.return_value = _make_fill("SELL", 2, Decimal("3.60"))

    await manager._tick()

    # Only position 2 should be closed
    mock_db.close_position.assert_awaited_once()
    close_args = mock_db.close_position.await_args[0]
    assert close_args[0] == 2  # position id for AAPL

    # Both positions should get price updates
    assert mock_db.update_position_price.await_count == 2


# ---------------------------------------------------------------------------
# Position sync from IB
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_sync_discovers_new_ib_position(manager, mock_db, mock_ib):
    """IB has a position not in DB → inserted as external."""
    ib_item = _make_ib_portfolio_item(con_id=99999, symbol="AAPL")
    mock_ib.portfolio.return_value = [ib_item]
    mock_db.get_open_positions.return_value = []
    mock_db.find_position_by_contract.return_value = None

    await manager._sync_positions()

    mock_db.insert_position.assert_awaited_once()
    pos = mock_db.insert_position.await_args[0][0]
    assert pos["ticker"] == "AAPL"
    assert pos["ib_con_id"] == 99999
    assert pos["recommendation_id"] is None


@pytest.mark.asyncio
async def test_sync_updates_existing_position(manager, mock_db, mock_ib):
    """Both IB and DB have the position → price/pnl updated."""
    ib_item = _make_ib_portfolio_item(
        con_id=12345, market_price=Decimal("11.00"), unrealized_pnl=Decimal("400")
    )
    mock_ib.portfolio.return_value = [ib_item]

    db_pos = _make_position(pos_id=1, ib_con_id=12345)
    mock_db.get_open_positions.return_value = [db_pos]

    await manager._sync_positions()

    mock_db.update_position_price.assert_awaited_once_with(
        1, Decimal("11.00"), Decimal("400")
    )
    mock_db.insert_position.assert_not_awaited()


@pytest.mark.asyncio
async def test_sync_closes_stale_position(manager, mock_db, mock_ib):
    """DB has a position with con_id, IB doesn't → marked closed."""
    mock_ib.portfolio.return_value = []

    db_pos = _make_position(pos_id=5, ib_con_id=55555)
    mock_db.get_open_positions.return_value = [db_pos]

    await manager._sync_positions()

    mock_db.close_position.assert_awaited_once_with(5, "external", Decimal("0"))


@pytest.mark.asyncio
async def test_sync_backfills_con_id(manager, mock_db, mock_ib):
    """Matched by contract details → con_id backfilled."""
    ib_item = _make_ib_portfolio_item(con_id=77777)
    mock_ib.portfolio.return_value = [ib_item]

    # No match by con_id (no positions have ib_con_id set)
    db_pos = _make_position(pos_id=3, ib_con_id=None)
    mock_db.get_open_positions.return_value = [db_pos]

    # But find_position_by_contract returns a match
    mock_db.find_position_by_contract.return_value = db_pos

    await manager._sync_positions()

    mock_db.update_position_con_id.assert_awaited_once_with(3, 77777)
    mock_db.update_position_price.assert_awaited_once()


@pytest.mark.asyncio
async def test_sync_skips_non_options(manager, mock_db, mock_ib):
    """Stocks in IB portfolio are ignored."""
    stock = IBPortfolioItem(
        con_id=11111, symbol="AAPL", sec_type="STK", position=100,
        avg_cost=Decimal("150"), market_price=Decimal("155"),
        market_value=Decimal("15500"), unrealized_pnl=Decimal("500"),
        realized_pnl=Decimal("0"), account="U1234567",
    )
    mock_ib.portfolio.return_value = [stock]
    mock_db.get_open_positions.return_value = []

    await manager._sync_positions()

    mock_db.insert_position.assert_not_awaited()
    mock_db.update_position_price.assert_not_awaited()


@pytest.mark.asyncio
async def test_sync_empty_portfolio(manager, mock_db, mock_ib):
    """Empty portfolio, no DB positions — no changes."""
    mock_ib.portfolio.return_value = []
    mock_db.get_open_positions.return_value = []

    await manager._sync_positions()

    mock_db.insert_position.assert_not_awaited()
    mock_db.update_position_price.assert_not_awaited()
    mock_db.close_position.assert_not_awaited()
