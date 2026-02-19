"""Types for IB options position management."""

from __future__ import annotations

import datetime as _dt
from dataclasses import dataclass
from decimal import Decimal
from enum import StrEnum
from typing import Literal

from pydantic import BaseModel, Field


class OptionQuote(BaseModel):
    """Live option quote from IB."""

    bid: Decimal
    ask: Decimal
    last: Decimal
    mid: Decimal
    volume: int = 0
    open_interest: int = 0
    iv: float = 0.0
    delta: float = 0.0
    gamma: float = 0.0
    theta: float = 0.0
    vega: float = 0.0


class Fill(BaseModel):
    """Execution fill report."""

    order_id: int
    symbol: str
    side: Literal["BUY", "SELL"]
    quantity: int
    avg_fill_price: Decimal
    commission: Decimal
    filled_at: _dt.datetime
    con_id: int | None = None
    pending: bool = False  # True when order is Submitted but not yet Filled


class IBPortfolioItem(BaseModel):
    """A position as reported by IB Gateway."""

    con_id: int
    symbol: str
    sec_type: str
    right: str | None = None
    strike: Decimal | None = None
    expiry: _dt.date | None = None
    position: int
    avg_cost: Decimal
    market_price: Decimal
    market_value: Decimal
    unrealized_pnl: Decimal
    realized_pnl: Decimal
    account: str


class AccountSummary(BaseModel):
    """IB account summary snapshot."""

    net_liquidation: Decimal
    buying_power: Decimal
    available_funds: Decimal


class OptionsPosition(BaseModel):
    """An open options position tracked by the manager."""

    id: int
    recommendation_id: int | None = None
    ticker: str
    right: Literal["call", "put"]
    strike: Decimal
    expiry: _dt.date
    quantity: int
    avg_fill_price: Decimal
    current_price: Decimal
    cost_basis: Decimal
    unrealized_pnl: Decimal
    realized_pnl: Decimal = Decimal("0")
    status: str = "open"
    ib_con_id: int | None = None
    opened_at: _dt.datetime = Field(default_factory=lambda: _dt.datetime.now(_dt.UTC))

    def pnl_pct(self) -> Decimal:
        """Current P&L as percentage of cost basis."""
        if self.cost_basis == 0:
            return Decimal("0")
        return (self.unrealized_pnl / self.cost_basis) * 100

    def days_to_expiry(self) -> int:
        """Days until expiration."""
        today = _dt.date.today()
        return (self.expiry - today).days


class CloseReason(StrEnum):
    HARD_STOP = "stop_loss"
    PROFIT_TARGET = "profit_target"
    TIME_STOP = "time_stop"
    ALLOCATION_EXCEEDED = "allocation_exceeded"
    MANUAL = "manual"
    THESIS_INVALIDATED = "thesis_invalid"


@dataclass
class StopAction:
    """Action the manager should take on a position."""

    close_all: bool
    quantity: int = 0  # Only used when close_all=False (partial close)
    reason: CloseReason = CloseReason.MANUAL


@dataclass
class ManagerConfig:
    """Position manager configuration."""

    poll_interval_secs: int = 30
    hard_stop_pct: Decimal = Decimal("50")
    profit_target_1_pct: Decimal = Decimal("50")
    profit_target_2_pct: Decimal = Decimal("100")
    time_stop_dte: int = 7
    max_allocation_pct: Decimal = Decimal("10")
    max_correlated: int = 3
