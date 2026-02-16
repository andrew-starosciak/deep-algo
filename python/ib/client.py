"""Thin ib_async wrapper for real IB TWS/Gateway connections."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from decimal import Decimal

from ib.types import AccountSummary, Fill, OptionQuote
from schemas.thesis import ContractSpec

logger = logging.getLogger(__name__)

# Maximum seconds to wait for an order fill before giving up
_ORDER_FILL_TIMEOUT_SECS = 120


@dataclass
class IBConfig:
    """IB connection configuration."""

    host: str = "127.0.0.1"  # NOT localhost — TWS blocks IPv6
    port: int = 4002  # 4002=paper gateway, 4001=live gateway
    client_id: int = 100


class IBClient:
    """Async IB client using ib_async."""

    def __init__(self, config: IBConfig | None = None):
        self._config = config or IBConfig()
        self._ib = None

    def _require_connected(self):
        if self._ib is None:
            raise RuntimeError("IBClient is not connected. Call connect() first.")

    async def connect(self) -> None:
        from ib_async import IB

        self._ib = IB()
        await self._ib.connectAsync(
            host=self._config.host,
            port=self._config.port,
            clientId=self._config.client_id,
        )
        logger.info(
            "Connected to IB at %s:%d (client_id=%d)",
            self._config.host, self._config.port, self._config.client_id,
        )

    async def disconnect(self) -> None:
        if self._ib:
            self._ib.disconnect()
            logger.info("Disconnected from IB")

    async def account_summary(self) -> AccountSummary:
        """Fetch account summary values."""
        self._require_connected()
        tags = await self._ib.accountSummaryAsync()
        values = {t.tag: t.value for t in tags}
        return AccountSummary(
            net_liquidation=Decimal(values.get("NetLiquidation", "0")),
            buying_power=Decimal(values.get("BuyingPower", "0")),
            available_funds=Decimal(values.get("AvailableFunds", "0")),
        )

    async def get_option_quote(self, contract: ContractSpec) -> OptionQuote:
        """Get a live quote for an options contract."""
        self._require_connected()
        from ib_async import Option

        ib_contract = Option(
            symbol=contract.ticker,
            lastTradeDateOrContractMonth=contract.expiry.strftime("%Y%m%d"),
            strike=float(contract.strike),
            right=contract.right[0].upper(),  # "call" -> "C", "put" -> "P"
            exchange="SMART",
        )
        await self._ib.qualifyContractsAsync(ib_contract)
        self._ib.reqMktData(ib_contract, genericTickList="", snapshot=True)
        ticker = await self._ib.reqTickersAsync(ib_contract)
        t = ticker[0] if ticker else None

        if t is None:
            raise ValueError(f"No quote for {contract.ticker} {contract.strike}{contract.right}")

        bid = Decimal(str(t.bid)) if t.bid and t.bid > 0 else Decimal("0")
        ask = Decimal(str(t.ask)) if t.ask and t.ask > 0 else Decimal("0")
        last = Decimal(str(t.last)) if t.last and t.last > 0 else Decimal("0")
        mid = (bid + ask) / 2 if bid > 0 and ask > 0 else last

        greeks = t.modelGreeks or t.lastGreeks
        return OptionQuote(
            bid=bid,
            ask=ask,
            last=last,
            mid=mid,
            volume=int(t.volume or 0),
            open_interest=0,
            iv=greeks.impliedVol if greeks else 0.0,
            delta=greeks.delta if greeks else 0.0,
            gamma=greeks.gamma if greeks else 0.0,
            theta=greeks.theta if greeks else 0.0,
            vega=greeks.vega if greeks else 0.0,
        )

    async def place_order(
        self,
        contract: ContractSpec,
        side: str,
        quantity: int,
        order_type: str = "LMT",
        limit_price: Decimal | None = None,
    ) -> Fill:
        """Place an order and wait for fill."""
        import datetime as _dt

        self._require_connected()
        from ib_async import LimitOrder, MarketOrder, Option

        ib_contract = Option(
            symbol=contract.ticker,
            lastTradeDateOrContractMonth=contract.expiry.strftime("%Y%m%d"),
            strike=float(contract.strike),
            right=contract.right[0].upper(),
            exchange="SMART",
        )
        await self._ib.qualifyContractsAsync(ib_contract)

        if order_type == "LMT" and limit_price is not None:
            order = LimitOrder(side, quantity, float(limit_price))
        else:
            order = MarketOrder(side, quantity)

        trade = self._ib.placeOrder(ib_contract, order)

        # Wait for fill with timeout
        elapsed = 0
        while not trade.isDone():
            await self._ib.waitOnUpdate(timeout=5)
            elapsed += 5
            if elapsed >= _ORDER_FILL_TIMEOUT_SECS:
                self._ib.cancelOrder(trade.order)
                raise TimeoutError(
                    f"Order not filled within {_ORDER_FILL_TIMEOUT_SECS}s — cancelled"
                )

        if trade.orderStatus.status != "Filled":
            raise RuntimeError(
                f"Order not filled: {trade.orderStatus.status} — {trade.orderStatus.whyHeld}"
            )

        avg_price = Decimal(str(trade.orderStatus.avgFillPrice))
        commission = sum(Decimal(str(f.commission)) for f in trade.fills if f.commission)

        logger.info(
            "Filled: %s %d %s @ %s (commission $%s)",
            side, quantity, contract.ticker, avg_price, commission,
        )

        return Fill(
            order_id=trade.order.orderId,
            symbol=contract.ticker,
            side=side,
            quantity=quantity,
            avg_fill_price=avg_price,
            commission=commission,
            filled_at=_dt.datetime.now(_dt.UTC),
        )

    async def cancel_order(self, order_id: int) -> None:
        """Cancel an open order by ID."""
        self._require_connected()
        for trade in self._ib.openTrades():
            if trade.order.orderId == order_id:
                self._ib.cancelOrder(trade.order)
                logger.info("Cancelled order %d", order_id)
                return
        logger.warning("Order %d not found in open trades", order_id)
