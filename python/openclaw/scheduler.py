"""Cron-driven workflow scheduling via APScheduler.

Runs three types of jobs:
1. Research workflows — trade-thesis for each watchlist ticker
2. Position monitoring — position manager tick cycle
3. Position reviews — LLM-driven review of open positions (future)
"""

from __future__ import annotations

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

    def __init__(self, engine: Any, db: Any, ib_client: Any, notifier: Any = None):
        self.engine = engine
        self.db = db
        self.ib_client = ib_client
        self.notifier = notifier
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
        """Connect IB and start the scheduler."""
        from ib.position_manager import PositionManager

        await self.ib_client.connect()
        self._position_manager = PositionManager(
            db=self.db, ib_client=self.ib_client, notifier=self.notifier,
        )

        await self.setup()
        logger.info("Scheduler started — waiting for triggers")

        try:
            await self.scheduler.run_until_stopped()
        finally:
            await self.ib_client.disconnect()

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
        """Run the trade-thesis workflow for a single ticker and save recommendation."""
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
            or thesis.recommended_contract is None
        ):
            logger.info("No actionable recommendation for %s", ticker)
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

        except Exception:
            logger.exception("Failed to save recommendation for %s", ticker)

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
