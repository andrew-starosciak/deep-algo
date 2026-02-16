"""Position manager — poll DB, execute approved recs, monitor, enforce rules."""

from __future__ import annotations

import asyncio
import logging
from decimal import Decimal

from ib.rules import check_allocation, check_profit_targets, check_stop_rules
from ib.types import ManagerConfig, OptionsPosition, StopAction
from schemas.thesis import ContractSpec

logger = logging.getLogger(__name__)


class PositionManager:
    """Main service loop for options position management.

    Polls the database for:
    1. Approved recommendations → execute orders
    2. Open positions → update prices, enforce stop/target rules
    """

    def __init__(self, db, ib_client, config: ManagerConfig | None = None, notifier=None):
        self.db = db
        self.ib = ib_client
        self.config = config or ManagerConfig()
        self.notifier = notifier

    async def run(self) -> None:
        """Run the service loop until cancelled."""
        logger.info(
            "PositionManager started (poll every %ds)", self.config.poll_interval_secs
        )
        await self.ib.connect()

        try:
            while True:
                try:
                    await self._tick()
                except Exception:
                    logger.exception("Error in position manager tick")
                await asyncio.sleep(self.config.poll_interval_secs)
        finally:
            await self.ib.disconnect()

    async def _tick(self) -> None:
        """One poll cycle: execute recs → update prices → check rules."""
        # 1. Execute approved recommendations
        approved = await self.db.get_approved_recommendations()
        for rec in approved:
            await self._execute_recommendation(rec)

        # 2. Fetch open positions and update prices
        positions = await self.db.get_open_positions()
        for pos_dict in positions:
            pos = OptionsPosition(**pos_dict)
            await self._update_and_check(pos)

    async def _execute_recommendation(self, rec: dict) -> None:
        """Execute an approved recommendation: place order, record position."""
        rec_id = rec["id"]
        ticker = rec["ticker"]

        try:
            await self.db.update_recommendation_status(rec_id, "executing")

            # Check allocation before executing
            account = await self.ib.account_summary()
            exposure = await self.db.get_total_options_exposure()
            position_usd = Decimal(str(rec["position_size_usd"]))

            approved, reason = check_allocation(
                position_usd, exposure, account.net_liquidation, self.config
            )
            if not approved:
                logger.warning("Allocation check failed for rec %d: %s", rec_id, reason)
                await self.db.update_recommendation_status(rec_id, "failed", reason)
                return

            # Build contract spec
            contract = ContractSpec(
                ticker=ticker,
                right=rec["right"],
                strike=Decimal(str(rec["strike"])),
                expiry=rec["expiry"],
                entry_price_low=Decimal(str(rec["entry_price_low"])),
                entry_price_high=Decimal(str(rec["entry_price_high"])),
            )

            # Get quote and calculate quantity
            quote = await self.ib.get_option_quote(contract)
            price_per_contract = quote.mid * 100  # Options are 100 shares
            if price_per_contract <= 0:
                await self.db.update_recommendation_status(
                    rec_id, "failed", "Zero or negative quote"
                )
                return

            quantity = max(1, int(position_usd / price_per_contract))

            # Place order
            fill = await self.ib.place_order(
                contract=contract,
                side="BUY",
                quantity=quantity,
                order_type="LMT",
                limit_price=quote.mid,
            )

            # Record position in DB — keep Decimal throughout
            cost_basis = fill.avg_fill_price * fill.quantity * 100
            await self.db.insert_position({
                "recommendation_id": rec_id,
                "ticker": ticker,
                "right": rec["right"],
                "strike": Decimal(str(rec["strike"])),
                "expiry": rec["expiry"],
                "quantity": fill.quantity,
                "avg_fill_price": fill.avg_fill_price,
                "current_price": fill.avg_fill_price,
                "cost_basis": cost_basis,
                "unrealized_pnl": Decimal("0"),
                "status": "open",
            })

            await self.db.update_recommendation_status(rec_id, "filled")
            logger.info(
                "Executed rec %d: BUY %d %s %s %s @ %s",
                rec_id, fill.quantity, ticker,
                rec["right"], rec["strike"], fill.avg_fill_price,
            )

            if self.notifier:
                await self.notifier.send(
                    f"*Filled* rec #{rec_id}: {fill.quantity}x {ticker} "
                    f"{rec['right']} {rec['strike']} @ ${fill.avg_fill_price}"
                )

        except Exception as e:
            logger.exception("Failed to execute rec %d", rec_id)
            await self.db.update_recommendation_status(rec_id, "failed", str(e))

    async def _update_and_check(self, pos: OptionsPosition) -> None:
        """Update a position's price and check stop/target rules."""
        # Build a ContractSpec to query the quote
        contract = ContractSpec(
            ticker=pos.ticker,
            right=pos.right,
            strike=pos.strike,
            expiry=pos.expiry,
            entry_price_low=pos.avg_fill_price,
            entry_price_high=pos.avg_fill_price,
        )

        try:
            quote = await self.ib.get_option_quote(contract)
            new_price = quote.mid
            unrealized = (new_price - pos.avg_fill_price) * pos.quantity * 100
            await self.db.update_position_price(pos.id, new_price, unrealized)

            # Update the in-memory position for rule checks
            pos = pos.model_copy(update={
                "current_price": new_price,
                "unrealized_pnl": unrealized,
            })
        except Exception:
            logger.warning("Failed to get quote for %s — skipping price update", pos.ticker)

        # Check stop rules (hard stop, time stop)
        action = check_stop_rules(pos, self.config)
        if action is not None:
            await self._execute_action(pos, action)
            return

        # Check profit targets
        action = check_profit_targets(pos, self.config)
        if action is not None:
            await self._execute_action(pos, action)

    async def _execute_action(self, pos: OptionsPosition, action: StopAction) -> None:
        """Execute a close action on a position."""
        contract = ContractSpec(
            ticker=pos.ticker,
            right=pos.right,
            strike=pos.strike,
            expiry=pos.expiry,
            entry_price_low=pos.avg_fill_price,
            entry_price_high=pos.avg_fill_price,
        )

        qty = pos.quantity if action.close_all else action.quantity
        try:
            fill = await self.ib.place_order(
                contract=contract,
                side="SELL",
                quantity=qty,
                order_type="MKT",
            )

            realized = (fill.avg_fill_price - pos.avg_fill_price) * qty * 100

            if action.close_all:
                await self.db.close_position(pos.id, action.reason.value, realized)
            else:
                # Partial close — persist new quantity and realized P&L
                remaining = pos.quantity - qty
                await self.db.partial_close_position(pos.id, remaining, realized)

            logger.info(
                "%s: %s %d %s (reason: %s, realized: $%s)",
                "CLOSED" if action.close_all else "PARTIAL CLOSE",
                pos.ticker, qty, pos.right, action.reason.value, realized,
            )

            if self.notifier:
                await self.notifier.send(
                    f"*{'Closed' if action.close_all else 'Partial close'}* "
                    f"{pos.ticker} {qty}x {pos.right} {pos.strike} "
                    f"— {action.reason.value} (${realized})"
                )

        except Exception:
            logger.exception("Failed to execute %s on %s", action.reason.value, pos.ticker)
