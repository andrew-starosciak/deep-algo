"""Thin ib_async wrapper for real IB TWS/Gateway connections."""

from __future__ import annotations

import datetime as _dt
import logging
from dataclasses import dataclass
from decimal import Decimal

from ib.types import AccountSummary, Fill, IBPortfolioItem, OptionQuote
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

    async def connect(self, max_retries: int = 5, retry_delay: float = 5.0) -> None:
        """Connect to IB Gateway with retry logic.

        Known issue: ib_insync bug #303 causes first connect() to often fail with TimeoutError
        on fresh Gateway starts. This implements retry logic with backoff.

        Args:
            max_retries: Maximum connection attempts (default 5)
            retry_delay: Seconds to wait between retries (default 5.0)
        """
        import asyncio
        from ib_async import IB

        last_error = None
        for attempt in range(1, max_retries + 1):
            try:
                logger.info(
                    "Connecting to IB at %s:%d (client_id=%d, attempt %d/%d)",
                    self._config.host, self._config.port, self._config.client_id,
                    attempt, max_retries,
                )

                self._ib = IB()
                # 10s timeout per attempt (not 30s) to fail fast and retry
                await asyncio.wait_for(
                    self._ib.connectAsync(
                        host=self._config.host,
                        port=self._config.port,
                        clientId=self._config.client_id,
                        timeout=10,
                    ),
                    timeout=12.0,  # Slightly higher than internal timeout
                )

                # Wait for nextValidId callback (confirms handshake complete)
                await asyncio.sleep(1)

                # Subscribe to account updates so portfolio() has data.
                # Use a timeout — reqAccountUpdatesAsync can hang with empty account.
                try:
                    accounts = self._ib.managedAccounts()
                    acct = accounts[0] if accounts else ''
                    logger.info("Subscribing to account updates for: %s", acct or '(all)')
                    await asyncio.wait_for(
                        self._ib.reqAccountUpdatesAsync(account=acct),
                        timeout=10.0,
                    )
                except (asyncio.TimeoutError, Exception) as e:
                    logger.warning("reqAccountUpdatesAsync timed out or failed: %s (continuing)", e)

                logger.info(
                    "Successfully connected to IB at %s:%d (client_id=%d)",
                    self._config.host, self._config.port, self._config.client_id,
                )
                return  # Success!

            except (TimeoutError, asyncio.TimeoutError, OSError, ConnectionRefusedError) as e:
                last_error = e
                if self._ib:
                    try:
                        self._ib.disconnect()
                    except Exception:
                        pass
                    self._ib = None

                if attempt < max_retries:
                    logger.warning(
                        "Connection attempt %d/%d failed: %s — retrying in %.1fs",
                        attempt, max_retries, e, retry_delay,
                    )
                    await asyncio.sleep(retry_delay)
                else:
                    logger.error(
                        "Failed to connect after %d attempts. Last error: %s",
                        max_retries, e,
                    )

        # All retries exhausted
        raise RuntimeError(
            f"Could not connect to IB Gateway at {self._config.host}:{self._config.port} "
            f"after {max_retries} attempts. Last error: {last_error}"
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

    async def portfolio(self) -> list[IBPortfolioItem]:
        """Get all portfolio positions with P&L from IB."""
        self._require_connected()
        items = self._ib.portfolio()
        result = []
        for item in items:
            c = item.contract
            expiry = None
            if c.lastTradeDateOrContractMonth:
                try:
                    expiry = _dt.datetime.strptime(
                        c.lastTradeDateOrContractMonth, "%Y%m%d"
                    ).date()
                except ValueError:
                    pass
            result.append(IBPortfolioItem(
                con_id=c.conId,
                symbol=c.symbol,
                sec_type=c.secType,
                right={"C": "call", "P": "put"}.get(c.right),
                strike=Decimal(str(c.strike)) if c.strike else None,
                expiry=expiry,
                position=int(item.position),
                avg_cost=Decimal(str(item.averageCost)),
                market_price=Decimal(str(item.marketPrice)),
                market_value=Decimal(str(item.marketValue)),
                unrealized_pnl=Decimal(str(item.unrealizedPNL)),
                realized_pnl=Decimal(str(item.realizedPNL)),
                account=item.account,
            ))
        return result

    async def get_stock_price(self, ticker: str) -> Decimal:
        """Get the current market price for a stock (works after hours too)."""
        import math

        self._require_connected()
        from ib_async import Stock

        stock = Stock(ticker, "SMART", "USD")
        await self._ib.qualifyContractsAsync(stock)
        self._ib.reqMktData(stock, genericTickList="", snapshot=True)
        tickers = await self._ib.reqTickersAsync(stock)
        t = tickers[0] if tickers else None

        if t is None:
            raise ValueError(f"No price data for {ticker}")

        def _valid(v):
            return v is not None and not math.isnan(v) and v > 0

        # Try: market price → last → close → bid/ask midpoint
        price = t.marketPrice()
        if not _valid(price):
            price = t.last
        if not _valid(price):
            price = t.close
        if not _valid(price) and _valid(t.bid) and _valid(t.ask):
            price = (t.bid + t.ask) / 2
        if not _valid(price):
            # Last resort: use reqHistoricalData for most recent close
            bars = await self._ib.reqHistoricalDataAsync(
                stock, endDateTime="", durationStr="1 D",
                barSizeSetting="1 day", whatToShow="TRADES",
                useRTH=True, formatDate=1,
            )
            if bars:
                price = bars[-1].close
        if not _valid(price):
            raise ValueError(f"No valid price for {ticker} (market may be closed)")

        return Decimal(str(price))

    async def get_option_expirations(self, ticker: str) -> list[_dt.date]:
        """Get available option expiration dates for a ticker."""
        chain = await self.get_option_chain(ticker)
        return chain["expirations"]

    async def get_option_chain(self, ticker: str) -> dict:
        """Get available option expirations and strikes for a ticker.

        Returns:
            {"expirations": [date, ...], "strikes": [float, ...]}
        """
        self._require_connected()
        from ib_async import Stock

        stock = Stock(ticker, "SMART", "USD")
        await self._ib.qualifyContractsAsync(stock)
        chains = await self._ib.reqSecDefOptParamsAsync(
            stock.symbol, "", stock.secType, stock.conId
        )
        if not chains:
            raise ValueError(f"No option chains for {ticker}")

        # Merge expirations and strikes from all exchanges
        all_expirations: set[_dt.date] = set()
        all_strikes: set[float] = set()
        for chain in chains:
            for exp_str in chain.expirations:
                try:
                    all_expirations.add(_dt.datetime.strptime(exp_str, "%Y%m%d").date())
                except ValueError:
                    continue
            all_strikes.update(chain.strikes)

        return {
            "expirations": sorted(all_expirations),
            "strikes": sorted(all_strikes),
        }

    async def get_option_quote(self, contract: ContractSpec) -> OptionQuote:
        """Get a quote for an options contract (falls back to delayed data).

        Uses streaming mode instead of snapshot so delayed data ticks have
        time to arrive (paper accounts don't get real-time options data).
        """
        import asyncio
        import math

        self._require_connected()
        from ib_async import Option

        def _valid(v):
            return v is not None and not math.isnan(v) and v > 0

        def _safe_float(v, default=0.0):
            return v if v is not None and not math.isnan(v) else default

        # Request delayed data if real-time isn't available (paper accounts)
        self._ib.reqMarketDataType(4)  # 4 = delayed-frozen

        ib_contract = Option(
            symbol=contract.ticker,
            lastTradeDateOrContractMonth=contract.expiry.strftime("%Y%m%d"),
            strike=float(contract.strike),
            right=contract.right[0].upper(),  # "call" -> "C", "put" -> "P"
            exchange="SMART",
        )
        qualified = await self._ib.qualifyContractsAsync(ib_contract)
        if not qualified or not ib_contract.conId:
            raise ValueError(
                f"Contract not found: {contract.ticker} {contract.strike}"
                f"{contract.right[0].upper()} exp {contract.expiry}. "
                f"Strike may not exist or market data unavailable."
            )

        # Use streaming mode — snapshot returns before delayed ticks arrive
        self._ib.reqMktData(ib_contract, genericTickList="", snapshot=False)
        t = self._ib.ticker(ib_contract)

        # Wait up to 8s for any price data to arrive
        for i in range(16):
            await asyncio.sleep(0.5)
            if _valid(t.bid) or _valid(t.ask) or _valid(t.last) or _valid(t.close):
                # Give one more beat for bid+ask pair to both arrive
                await asyncio.sleep(0.5)
                break

        # Cancel streaming subscription
        self._ib.cancelMktData(ib_contract)

        logger.info(
            "Option quote %s %s%s exp %s: bid=%s ask=%s last=%s close=%s",
            contract.ticker, contract.strike, contract.right[0].upper(),
            contract.expiry, t.bid, t.ask, t.last, t.close,
        )

        bid = Decimal(str(t.bid)) if _valid(t.bid) else Decimal("0")
        ask = Decimal(str(t.ask)) if _valid(t.ask) else Decimal("0")
        last = Decimal(str(t.last)) if _valid(t.last) else Decimal("0")
        close = Decimal(str(t.close)) if _valid(t.close) else Decimal("0")

        # Also try marketPrice() which aggregates various price sources
        mp = t.marketPrice()

        if bid > 0 and ask > 0:
            mid = (bid + ask) / 2
        elif last > 0:
            mid = last
        elif _valid(mp):
            mid = Decimal(str(mp))
        elif close > 0:
            mid = close
        else:
            mid = Decimal("0")

        greeks = t.modelGreeks or t.lastGreeks
        return OptionQuote(
            bid=bid,
            ask=ask,
            last=last,
            mid=mid,
            volume=int(_safe_float(t.volume, 0)),
            open_interest=0,
            iv=_safe_float(greeks.impliedVol if greeks else 0.0),
            delta=_safe_float(greeks.delta if greeks else 0.0),
            gamma=_safe_float(greeks.gamma if greeks else 0.0),
            theta=_safe_float(greeks.theta if greeks else 0.0),
            vega=_safe_float(greeks.vega if greeks else 0.0),
        )

    async def place_order(
        self,
        contract: ContractSpec,
        side: str,
        quantity: int,
        order_type: str = "LMT",
        limit_price: Decimal | None = None,
    ) -> Fill:
        """Place an order and wait for fill.

        For after-hours limit orders, IB accepts them as PreSubmitted and they
        fill at market open. We return the order details immediately rather than
        blocking for 2 minutes.
        """
        import asyncio
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
        qualified = await self._ib.qualifyContractsAsync(ib_contract)
        if not qualified or not ib_contract.conId:
            raise ValueError(
                f"Contract not found: {contract.ticker} {contract.strike}"
                f"{contract.right[0].upper()} exp {contract.expiry}"
            )

        if order_type == "LMT" and limit_price is not None:
            # Options tick rules: $0.05 increments for premiums >= $3, else $0.01.
            # Round BUY up and SELL down to the nearest valid tick to improve fill odds.
            tick = Decimal("0.05") if limit_price >= 3 else Decimal("0.01")
            if side.upper() == "BUY":
                rounded = (limit_price / tick).to_integral_value(rounding="ROUND_CEILING") * tick
            else:
                rounded = (limit_price / tick).to_integral_value(rounding="ROUND_FLOOR") * tick
            if rounded != limit_price:
                logger.info("Rounded limit %s → %s (tick=%s)", limit_price, rounded, tick)
            order = LimitOrder(side, quantity, float(rounded))
        else:
            order = MarketOrder(side, quantity)

        trade = self._ib.placeOrder(ib_contract, order)

        # Wait for fill with timeout.
        # Use asyncio.sleep instead of waitOnUpdate to avoid
        # "event loop already running" error in async contexts.
        elapsed = 0
        while True:
            await asyncio.sleep(2)
            elapsed += 2

            status = trade.orderStatus.status
            logger.info(
                "Order %d status: %s (elapsed %ds)",
                trade.order.orderId, status, elapsed,
            )

            if status == "Filled":
                break

            # PreSubmitted = accepted by IB but waiting for market open.
            # Submitted = live order working in the market (after-hours or during market).
            # Return immediately with pending=True — the position will appear in
            # IB's portfolio once the order fills and _sync_positions will pick it up.
            if status in ("PreSubmitted", "Submitted"):
                logger.info(
                    "Order %d %s — accepted by IB (pending fill)",
                    trade.order.orderId, status,
                )
                fill_price = rounded if order_type == "LMT" and limit_price is not None else (limit_price or Decimal("0"))
                return Fill(
                    order_id=trade.order.orderId,
                    symbol=contract.ticker,
                    side=side,
                    quantity=quantity,
                    avg_fill_price=fill_price,
                    commission=Decimal("0"),
                    filled_at=_dt.datetime.now(_dt.UTC),
                    con_id=ib_contract.conId if ib_contract.conId else None,
                    pending=True,
                )

            # Error 10349 = IB changed TIF to DAY due to preset. The order is still
            # live — IB sends Cancelled then Submitted. Wait for the real status.
            if status == "Cancelled":
                has_tif_warning = any(
                    e.errorCode == 10349 for e in trade.log
                )
                if has_tif_warning and elapsed < 10:
                    logger.info(
                        "Order %d Cancelled with TIF preset warning (10349) — "
                        "waiting for real status", trade.order.orderId,
                    )
                    continue
                raise RuntimeError(
                    f"Order not filled: {trade.orderStatus.status} — "
                    f"{trade.orderStatus.whyHeld}"
                )

            if elapsed >= _ORDER_FILL_TIMEOUT_SECS:
                self._ib.cancelOrder(trade.order)
                raise TimeoutError(
                    f"Order not filled within {_ORDER_FILL_TIMEOUT_SECS}s — cancelled"
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
            con_id=ib_contract.conId if ib_contract.conId else None,
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
