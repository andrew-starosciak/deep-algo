"""Cron-driven workflow scheduling via APScheduler.

Runs three types of jobs:
1. Research workflows — trade-thesis for each watchlist ticker
2. Position monitoring — position manager tick cycle
3. Position reviews — LLM-driven review of open positions (future)
"""

from __future__ import annotations

import asyncio
import datetime as _dt
import logging
from decimal import Decimal
from typing import Any

from apscheduler import AsyncScheduler
from apscheduler.triggers.cron import CronTrigger

logger = logging.getLogger(__name__)

# All times in US Eastern
TIMEZONE = "America/New_York"


class WorkflowScheduler:
    """Schedule and run workflows + position management on cron triggers."""

    def __init__(
        self,
        engine: Any,
        db: Any,
        ib_client: Any,
        notifier: Any = None,
        auto_approve: bool = False,
    ):
        self.engine = engine
        self.db = db
        self.ib_client = ib_client
        self.notifier = notifier
        self.auto_approve = auto_approve
        self.scheduler = AsyncScheduler()
        self._position_manager = None

    async def setup(self) -> None:
        """Register all scheduled jobs."""
        # Pre-market: run trade-thesis for each watchlist ticker
        await self.scheduler.add_schedule(
            self._premarket_research,
            CronTrigger(hour=8, minute=0, day_of_week="mon-fri", timezone=TIMEZONE),
            id="premarket_research",
        )
        logger.info("Scheduled premarket_research: 8:00 AM ET Mon-Fri")

        # Midday: monitor positions (tick cycle)
        await self.scheduler.add_schedule(
            self._position_tick,
            CronTrigger(hour=12, minute=30, day_of_week="mon-fri", timezone=TIMEZONE),
            id="midday_position_check",
        )
        logger.info("Scheduled midday_position_check: 12:30 PM ET Mon-Fri")

        # Post-market: monitor positions
        await self.scheduler.add_schedule(
            self._position_tick,
            CronTrigger(hour=16, minute=30, day_of_week="mon-fri", timezone=TIMEZONE),
            id="postmarket_position_check",
        )
        logger.info("Scheduled postmarket_position_check: 4:30 PM ET Mon-Fri")

        # Weekend: deep dive research for all watchlist tickers
        await self.scheduler.add_schedule(
            self._weekly_deep_dive,
            CronTrigger(hour=10, minute=0, day_of_week="sat", timezone=TIMEZONE),
            id="weekly_deep_dive",
        )
        logger.info("Scheduled weekly_deep_dive: 10:00 AM ET Saturday")

    async def start(self) -> None:
        """Start Discord, connect IB, then run scheduler + position loop."""
        from ib.position_manager import PositionManager

        # Start Discord bot FIRST — ib_async conflicts with discord.py
        # if IB connects before the Discord event loop is running.
        if self.notifier:
            self.notifier.set_context(
                db=self.db, engine=self.engine, auto_approve=self.auto_approve,
            )
            await self.notifier.start()
            logger.info("Discord bot started, now connecting to IB...")

        await self.ib_client.connect()
        self._position_manager = PositionManager(
            db=self.db, ib_client=self.ib_client, notifier=self.notifier,
        )

        # Wire position manager into Discord bot (after IB is connected)
        if self.notifier:
            self.notifier.set_context(position_manager=self._position_manager)

        # Start scheduler as async context manager before adding schedules
        async with self.scheduler:
            await self.setup()

            # Run position manager tick loop as a background task
            tick_task = asyncio.create_task(self._position_tick_loop())
            logger.info("Scheduler + position manager started — waiting for triggers")

            try:
                await self.scheduler.start_in_background()
                await asyncio.Event().wait()
            except (KeyboardInterrupt, asyncio.CancelledError):
                logger.info("Scheduler shutting down")
            finally:
                tick_task.cancel()
                await self.ib_client.disconnect()

    async def _position_tick_loop(self) -> None:
        """Continuous position monitoring during market hours (every 60s)."""
        import zoneinfo

        et = zoneinfo.ZoneInfo("America/New_York")
        poll_interval = self._position_manager.config.poll_interval_secs

        logger.info("Position tick loop started (every %ds during market hours)", poll_interval)

        while True:
            try:
                now = _dt.datetime.now(et)
                market_open = now.replace(hour=9, minute=30, second=0, microsecond=0)
                market_close = now.replace(hour=16, minute=0, second=0, microsecond=0)
                is_weekday = now.weekday() < 5

                if is_weekday and market_open <= now <= market_close:
                    await self._position_manager._tick()
                    await self._record_equity_snapshot()
                elif is_weekday and now < market_open:
                    # Sleep until market open
                    wait_secs = (market_open - now).total_seconds()
                    logger.info("Pre-market — next tick at 9:30 AM ET (%.0fm)", wait_secs / 60)
                    await asyncio.sleep(min(wait_secs, 300))
                    continue
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.exception("Position tick loop error")

            await asyncio.sleep(poll_interval)

    async def _record_equity_snapshot(self) -> None:
        """Record an equity snapshot after each position tick."""
        try:
            account = await self.ib_client.account_summary()
            positions = await self.db.get_open_positions()
            total_unrealized = sum(Decimal(str(p.get("unrealized_pnl", 0))) for p in positions)
            total_exposure = await self.db.get_total_options_exposure()
            total_realized = await self.db.get_total_realized_pnl()

            await self.db.insert_equity_snapshot(
                net_liquidation=account.net_liquidation,
                total_unrealized_pnl=total_unrealized,
                total_realized_pnl=total_realized,
                open_positions_count=len(positions),
                total_options_exposure=total_exposure,
            )
        except Exception:
            logger.warning("Failed to record equity snapshot", exc_info=True)

    async def _premarket_research(self) -> None:
        """Run trade-thesis workflow for each watchlist ticker."""
        logger.info("Pre-market research starting")
        tickers = await self._get_watchlist_tickers()

        for ticker in tickers:
            await self._run_thesis_for_ticker(ticker)

    async def _weekly_deep_dive(self) -> None:
        """Saturday deep dive — research all watchlist tickers."""
        logger.info("Weekly deep dive starting")
        tickers = await self._get_watchlist_tickers()

        for ticker in tickers:
            await self._run_thesis_for_ticker(ticker)

    async def _run_thesis_for_ticker(self, ticker: str) -> None:
        """Run trade-thesis or position-review depending on open positions."""
        # Check for open positions — route to position-review if found
        try:
            open_positions = await self.db.get_open_positions_for_ticker(ticker)
        except Exception:
            logger.warning("Failed to check positions for %s, falling back to trade-thesis", ticker)
            open_positions = []

        if open_positions:
            logger.info(
                "Found %d open position(s) for %s — running position-review",
                len(open_positions), ticker,
            )
            await self._run_position_review(ticker, open_positions)
            return

        from openclaw.workflows import get_workflow
        from schemas.research import ResearchRequest
        from schemas.risk import RiskVerification
        from schemas.thesis import Thesis

        workflow = get_workflow("trade-thesis")
        initial_input = ResearchRequest(ticker=ticker)

        logger.info("Running trade-thesis for %s", ticker)
        try:
            result = await self.engine.run(workflow, initial_input)
        except Exception:
            logger.exception("Workflow failed for %s", ticker)
            return

        # Persist research summary (valuable even if later gates fail)
        try:
            research_output = result.step_outputs.get("research") if result else None
            if research_output is not None:
                await self.db.save_research_summary(
                    run_id=result.run_id, ticker=ticker, mode="premarket",
                    summary=research_output.model_dump(),
                    opportunity_score=research_output.opportunity_score,
                )
        except Exception:
            logger.warning("Failed to save research summary for %s", ticker, exc_info=True)

        if result is None:
            logger.info("Workflow aborted for %s (did not pass gates)", ticker)
            return

        # Save recommendation if thesis passed risk verification
        thesis = result.step_outputs.get("evaluate")
        verification = result.step_outputs.get("verify")

        if (
            not isinstance(thesis, Thesis)
            or not isinstance(verification, RiskVerification)
            or not verification.approved
        ):
            logger.info("No actionable recommendation for %s", ticker)
            return

        # Programmatic contract selection using real IB data
        if thesis.recommended_contract is None:
            try:
                from ib.contract_selector import ContractSelector

                selector = ContractSelector(self.ib_client)
                selected = await selector.select(thesis)
                if not selected:
                    logger.info("No suitable contract for %s", ticker)
                    return
                thesis.recommended_contract = selected
            except Exception as e:
                logger.warning("Contract selection failed for %s: %s", ticker, e)
                return

        try:
            # Get equity from IB for accurate sizing
            account = await self.ib_client.account_summary()
            equity = account.net_liquidation
            position_size_usd = verification.position_size_pct * equity / Decimal("100")

            thesis_id = await self.db.save_thesis(result.run_id, thesis.model_dump())
            rec_data = {
                "contract": thesis.recommended_contract.model_dump(),
                "position_size_pct": str(verification.position_size_pct),
                "position_size_usd": str(position_size_usd),
                "exit_targets": ["+50% sell half", "+100% close"],
                "stop_loss": "-50% hard stop",
                "max_hold_days": 30,
                "risk_verification": verification.model_dump(),
            }
            rec_id = await self.db.save_recommendation(thesis_id, result.run_id, rec_data)

            logger.info(
                "Recommendation #%d saved for %s (size: $%s)",
                rec_id, ticker, position_size_usd,
            )

            if self.notifier:
                await self.notifier.send_recommendation({
                    "id": rec_id,
                    "ticker": ticker,
                    "direction": thesis.direction,
                    "contract": str(thesis.recommended_contract),
                    "position_size_usd": str(position_size_usd),
                    "overall_score": thesis.scores.overall,
                })

            # Auto-approve: skip human gate, approve immediately for execution
            if self.auto_approve:
                await self._auto_approve_recommendation(rec_id, ticker)

        except Exception:
            logger.exception("Failed to save recommendation for %s", ticker)

    async def _auto_approve_recommendation(self, rec_id: int, ticker: str) -> None:
        """Auto-approve a recommendation and trigger immediate execution."""
        try:
            await self.db.approve_recommendation(rec_id)
            logger.info("Auto-approved recommendation #%d for %s", rec_id, ticker)

            if self.notifier:
                await self.notifier.send(
                    f"Auto-approved recommendation #{rec_id} for **{ticker}** — "
                    f"executing on next position manager tick."
                )

            # Trigger immediate execution instead of waiting for next tick
            if self._position_manager:
                rec = await self.db.get_recommendation(rec_id)
                if rec:
                    await self._position_manager._execute_recommendation(rec)

        except Exception:
            logger.exception("Failed to auto-approve recommendation #%d", rec_id)

    async def _run_position_review(self, ticker: str, positions: list[dict]) -> None:
        """Run position-review workflow for each open position on this ticker."""
        from agents.reviewer import ReviewerAgent
        from openclaw.workflows import get_workflow
        from schemas.research import ResearchRequest
        from schemas.review import PositionReview

        workflow = get_workflow("position-review")

        for pos in positions:
            position_id = pos.get("id")
            logger.info("Running position-review for %s (position #%s)", ticker, position_id)

            # Create a fresh reviewer per position to avoid shared mutable state
            reviewer = ReviewerAgent(llm=self.engine.llm, db=self.db)
            reviewer.set_review_context(pos)
            original_reviewer = self.engine.agents.get("reviewer")
            self.engine.agents["reviewer"] = reviewer

            initial_input = ResearchRequest(ticker=ticker, mode="premarket")

            try:
                result = await self.engine.run(workflow, initial_input)
            except Exception:
                logger.exception("Position-review workflow failed for %s #%s", ticker, position_id)
                continue
            finally:
                # Restore the original reviewer agent
                if original_reviewer is not None:
                    self.engine.agents["reviewer"] = original_reviewer

            if result is None:
                logger.info("Position-review aborted for %s #%s", ticker, position_id)
                continue

            # Save research summary (valuable even if review step fails)
            try:
                research_output = result.step_outputs.get("research")
                if research_output is not None:
                    await self.db.save_research_summary(
                        run_id=result.run_id, ticker=ticker, mode="position_review",
                        summary=research_output.model_dump(),
                        opportunity_score=research_output.opportunity_score,
                    )
            except Exception:
                logger.warning("Failed to save research summary for %s review", ticker, exc_info=True)

            # Save review to position_reviews table
            review_output = result.step_outputs.get("review")
            if not isinstance(review_output, PositionReview):
                logger.info("No review output for %s #%s", ticker, position_id)
                continue

            try:
                review_id = await self.db.save_position_review(
                    run_id=result.run_id,
                    position_id=position_id,
                    review_type="premarket",
                    review_data=review_output.model_dump(),
                )
                logger.info(
                    "Position review #%d saved for %s #%s: %s",
                    review_id, ticker, position_id, review_output.recommended_action,
                )
            except Exception:
                logger.exception("Failed to save position review for %s #%s", ticker, position_id)
                continue

            # Notify Discord
            if self.notifier:
                action = review_output.recommended_action
                urgent = action in ("close", "reduce")
                prefix = "URGENT " if urgent else ""
                msg = (
                    f"{prefix}**Position Review: {ticker}** (#{position_id})\n"
                    f"Action: **{action}**\n"
                    f"Thesis valid: {review_output.thesis_still_valid}\n"
                    f"P&L: {review_output.pnl_pct:+.1f}%\n"
                    f"Reasoning: {review_output.reasoning[:300]}"
                )
                try:
                    await self.notifier.send(msg)
                except Exception:
                    logger.warning("Failed to send review notification", exc_info=True)

    async def _position_tick(self) -> None:
        """Run one position manager tick: update prices, check rules."""
        if self._position_manager is None:
            logger.warning("Position manager not initialized, skipping tick")
            return

        logger.info("Running position check")
        try:
            await self._position_manager._tick()
        except Exception:
            logger.exception("Position check failed")

    async def _get_watchlist_tickers(self) -> list[str]:
        """Get tickers from the watchlist."""
        try:
            watchlist = await self.db.get_watchlist()
            tickers = [row["ticker"] for row in watchlist]
            logger.info("Watchlist: %s", ", ".join(tickers))
            return tickers
        except Exception:
            logger.exception("Failed to load watchlist")
            return []
