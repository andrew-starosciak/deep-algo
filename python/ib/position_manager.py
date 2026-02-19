"""Position manager — poll DB, execute approved recs, monitor, enforce rules."""

from __future__ import annotations

import asyncio
import logging
import math
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
            "PositionManager starting (poll every %ds)", self.config.poll_interval_secs
        )

        # Connect to IB Gateway with retry logic (handles ib_insync bug #303)
        logger.info("Connecting to IB Gateway (may take 30-60s on fresh starts)...")
        try:
            await self.ib.connect(max_retries=5, retry_delay=5.0)
        except RuntimeError as e:
            logger.error("Failed to connect to IB Gateway: %s", e)
            logger.error(
                "Troubleshooting:\n"
                "  1. Check IB Gateway container is running: docker ps | grep ib-gateway\n"
                "  2. Check IB Gateway logs: docker logs ib-gateway\n"
                "  3. Verify 2FA approval: Check IBKR Mobile app for push notification\n"
                "  4. VNC into Gateway UI: vncviewer localhost:5900 (password: ibgateway)\n"
                "  5. Verify API enabled in IBKR account settings"
            )
            raise

        logger.info("Successfully connected to IB Gateway — position manager running")

        try:
            while True:
                try:
                    await self._tick()
                except Exception:
                    logger.exception("Error in position manager tick")
                await asyncio.sleep(self.config.poll_interval_secs)
        finally:
            await self.ib.disconnect()
            logger.info("Position manager stopped")

    async def _tick(self) -> None:
        """One poll cycle: sync IB → execute recs → check rules."""
        # 1. Sync positions from IB (IB is source of truth)
        await self._sync_positions()

        # 2. Execute approved recommendations
        approved = await self.db.get_approved_recommendations()
        for rec in approved:
            await self._execute_recommendation(rec)

        # 3. Fetch open positions and check stop/target rules
        positions = await self.db.get_open_positions()
        for pos_dict in positions:
            pos = OptionsPosition(**pos_dict)
            await self._update_and_check(pos)

    async def _sync_positions(self) -> None:
        """Reconcile DB positions with IB portfolio (IB is source of truth)."""
        try:
            ib_items = await self.ib.portfolio()
        except Exception:
            logger.warning("Failed to fetch IB portfolio — skipping sync")
            return

        # Filter to long options only
        ib_options = [i for i in ib_items if i.sec_type == "OPT" and i.position > 0]

        db_positions = await self.db.get_open_positions()
        db_by_con_id = {
            p["ib_con_id"]: p for p in db_positions if p.get("ib_con_id")
        }

        seen_db_ids: set[int] = set()

        for ib_item in ib_options:
            # Try to match by con_id first
            db_pos = db_by_con_id.get(ib_item.con_id)

            if db_pos is None:
                # Fallback: match by contract details
                db_pos = await self.db.find_position_by_contract(
                    ib_item.symbol, ib_item.right, ib_item.strike, ib_item.expiry
                )
                if db_pos:
                    await self.db.update_position_con_id(db_pos["id"], ib_item.con_id)

            if db_pos:
                # Known position — update price from IB
                seen_db_ids.add(db_pos["id"])
                await self.db.update_position_price(
                    db_pos["id"], ib_item.market_price, ib_item.unrealized_pnl
                )
            else:
                # New position not in our DB — insert as external
                cost_basis = ib_item.avg_cost * abs(ib_item.position) * 100
                pos_id = await self.db.insert_position({
                    "recommendation_id": None,
                    "ib_con_id": ib_item.con_id,
                    "ticker": ib_item.symbol,
                    "right": ib_item.right,
                    "strike": ib_item.strike,
                    "expiry": ib_item.expiry,
                    "quantity": abs(ib_item.position),
                    "avg_fill_price": ib_item.avg_cost,
                    "current_price": ib_item.market_price,
                    "cost_basis": cost_basis,
                    "unrealized_pnl": ib_item.unrealized_pnl,
                })
                logger.info(
                    "Synced external position: %s (con_id=%d, pos_id=%d)",
                    ib_item.symbol, ib_item.con_id, pos_id,
                )

        # Positions in DB but gone from IB → mark closed
        ib_con_ids = {i.con_id for i in ib_options}
        for db_pos in db_positions:
            if db_pos["id"] not in seen_db_ids:
                con_id = db_pos.get("ib_con_id")
                if con_id and con_id not in ib_con_ids:
                    await self.db.close_position(
                        db_pos["id"], "external", Decimal("0")
                    )
                    logger.info(
                        "Closed stale position %d (no longer in IB)", db_pos["id"]
                    )
                    # Record outcome on the originating thesis
                    try:
                        thesis_id = await self.db.get_thesis_id_for_position(db_pos["id"])
                        if thesis_id:
                            await self.db.update_thesis_outcome(
                                thesis_id, realized_pnl=Decimal("0"),
                                close_reason="external", position_id=db_pos["id"],
                            )
                    except Exception:
                        logger.warning("Failed to update thesis outcome for position %d", db_pos["id"])

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
            limit_price = quote.mid
            if limit_price <= 0:
                # Fallback: use thesis entry price for limit order
                entry_mid = (contract.entry_price_low + contract.entry_price_high) / 2
                if entry_mid > 0:
                    logger.warning(
                        "Quote returned $0 for rec %d — using thesis entry $%s as limit",
                        rec_id, entry_mid,
                    )
                    limit_price = entry_mid
                else:
                    await self.db.update_recommendation_status(
                        rec_id, "failed", "Zero or negative quote"
                    )
                    return

            price_per_contract = limit_price * 100  # Options are 100 shares
            quantity = max(1, int(position_usd / price_per_contract))

            # Place order
            fill = await self.ib.place_order(
                contract=contract,
                side="BUY",
                quantity=quantity,
                order_type="LMT",
                limit_price=limit_price,
            )

            # Record position in DB — keep Decimal throughout
            cost_basis = fill.avg_fill_price * fill.quantity * 100
            await self.db.insert_position({
                "recommendation_id": rec_id,
                "ib_con_id": fill.con_id,
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
            # Guard against bad quotes (NaN, zero, negative) — do NOT update
            # price or run rules with garbage data (would trigger false stop-loss)
            if new_price is None or new_price <= 0 or math.isnan(float(new_price)):
                logger.warning(
                    "Invalid quote for %s (mid=%s) — skipping price update and rule checks",
                    pos.ticker, new_price,
                )
                return

            unrealized = (new_price - pos.avg_fill_price) * pos.quantity * 100
            await self.db.update_position_price(pos.id, new_price, unrealized)

            # Update the in-memory position for rule checks
            pos = pos.model_copy(update={
                "current_price": new_price,
                "unrealized_pnl": unrealized,
            })
        except Exception:
            logger.warning("Failed to get quote for %s — skipping price update and rule checks", pos.ticker)
            return

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
                # Record outcome on the originating thesis
                # Include accumulated P&L from prior partial closes
                try:
                    total_realized = (pos.realized_pnl or Decimal("0")) + realized
                    thesis_id = await self.db.get_thesis_id_for_position(pos.id)
                    if thesis_id:
                        await self.db.update_thesis_outcome(
                            thesis_id, realized_pnl=total_realized,
                            close_reason=action.reason.value, position_id=pos.id,
                        )
                except Exception:
                    logger.warning("Failed to update thesis outcome for position %d", pos.id)
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
