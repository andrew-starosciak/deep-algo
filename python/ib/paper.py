"""Paper trading shim — same interface as IBClient, no real IB connection."""

from __future__ import annotations

import datetime as _dt
import logging
from decimal import Decimal

from ib.types import AccountSummary, Fill, IBPortfolioItem, OptionQuote
from schemas.thesis import ContractSpec

logger = logging.getLogger(__name__)


class PaperClient:
    """Simulated IB client for paper trading.

    Uses the same interface as IBClient so the PositionManager doesn't
    know the difference.
    """

    def __init__(self, equity: Decimal = Decimal("200000")):
        self._equity = equity
        self._order_counter = 0

    async def connect(self) -> None:
        logger.info("PaperClient connected (simulated)")

    async def disconnect(self) -> None:
        logger.info("PaperClient disconnected")

    async def account_summary(self) -> AccountSummary:
        return AccountSummary(
            net_liquidation=self._equity,
            buying_power=self._equity * 4,
            available_funds=self._equity,
        )

    async def portfolio(self) -> list[IBPortfolioItem]:
        """No real portfolio in sim mode."""
        return []

    async def get_stock_price(self, ticker: str) -> Decimal:
        """Return a simulated stock price."""
        # Rough lookup for common tickers; fallback to $150
        prices = {
            "AAPL": 230, "MSFT": 420, "GOOG": 175, "AMZN": 195,
            "META": 580, "TSLA": 340, "NVDA": 130, "SPY": 595,
        }
        price = prices.get(ticker.upper(), 150)
        return Decimal(str(price))

    async def get_option_expirations(self, ticker: str) -> list[_dt.date]:
        """Return simulated option expiration dates (monthly, next 3 months)."""
        import calendar
        today = _dt.date.today()
        expirations = []
        for month_offset in range(1, 4):
            m = (today.month + month_offset - 1) % 12 + 1
            y = today.year + (today.month + month_offset - 1) // 12
            # Third Friday of the month
            cal = calendar.monthcalendar(y, m)
            fridays = [week[calendar.FRIDAY] for week in cal if week[calendar.FRIDAY] != 0]
            third_friday = _dt.date(y, m, fridays[2])
            expirations.append(third_friday)
        return expirations

    async def get_option_quote(self, contract: ContractSpec) -> OptionQuote:
        """Return a quote using the contract's entry price range."""
        mid = (contract.entry_price_low + contract.entry_price_high) / 2
        spread = (contract.entry_price_high - contract.entry_price_low) / 2
        bid = mid - spread if spread > 0 else mid * Decimal("0.95")
        ask = mid + spread if spread > 0 else mid * Decimal("1.05")
        return OptionQuote(
            bid=bid,
            ask=ask,
            last=mid,
            mid=mid,
            volume=500,
            open_interest=2000,
            iv=0.35,
            delta=0.45,
            gamma=0.02,
            theta=-0.15,
            vega=0.20,
        )

    async def place_order(
        self,
        contract: ContractSpec,
        side: str,
        quantity: int,
        order_type: str = "LMT",
        limit_price: Decimal | None = None,
    ) -> Fill:
        """Simulate a fill at the limit price (or mid)."""
        self._order_counter += 1
        fill_price = limit_price or (contract.entry_price_low + contract.entry_price_high) / 2
        commission = Decimal("0.65") * quantity

        logger.info(
            "Paper %s %d %s %s %.2f @ %.2f (commission $%.2f)",
            side, quantity, contract.ticker,
            f"{contract.strike}{contract.right[0].upper()} {contract.expiry}",
            contract.strike, fill_price, commission,
        )

        return Fill(
            order_id=self._order_counter,
            symbol=contract.ticker,
            side=side,
            quantity=quantity,
            avg_fill_price=fill_price,
            commission=commission,
            filled_at=_dt.datetime.now(_dt.UTC),
        )

    async def cancel_order(self, order_id: int) -> None:
        logger.info("Paper cancel order %d (no-op)", order_id)
