"""Risk checker agent — validates exposure, sizing, correlations."""

from __future__ import annotations

import logging
from collections import Counter
from decimal import Decimal
from typing import Any

from pydantic import BaseModel

from agents.base import BaseAgent
from openclaw.llm import LLMClient

logger = logging.getLogger(__name__)


class RiskCheckerAgent(BaseAgent):
    """Independently verifies risk parameters. Does NOT trust the analyst's assessment."""

    def __init__(self, llm: LLMClient, db: Any = None, ib_client: Any = None):
        super().__init__(llm, db)
        self.ib_client = ib_client

    @property
    def role(self) -> str:
        return (
            "risk management specialist for a multi-platform trading operation. "
            "You verify position sizing (max 2% per trade), total allocation (max 10%), "
            "sector correlation (max 3 correlated positions), and cross-platform exposure "
            "with HyperLiquid and Polymarket positions. You are conservative and independent — "
            "you do NOT defer to the analyst's recommendation."
        )

    @property
    def prompt_file(self) -> str:
        return "risk_verification.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Pull current portfolio state from DB + IB for risk checks."""
        if self.db is None:
            return {"portfolio_state": "Database not connected. Use conservative defaults."}

        # 1. Account equity from IB (or fallback default)
        #    Timeout required: accountSummaryAsync can deadlock when the position
        #    manager tick loop is competing for the same ib_async connection.
        account_equity = Decimal("200000")
        if self.ib_client:
            try:
                import asyncio
                summary = await asyncio.wait_for(
                    self.ib_client.account_summary(), timeout=10.0
                )
                account_equity = summary.net_liquidation
            except Exception as e:
                logger.warning("IB account_summary failed, using default $200k: %s", e)

        # 2. Open positions from DB
        positions = await self.db.get_open_positions()
        total_exposure = await self.db.get_total_options_exposure()

        # 3. Sector map from watchlist for correlation counting
        watchlist = await self.db.get_watchlist()
        sector_map = {w["ticker"]: w.get("sector", "unknown") for w in watchlist}

        # 4. Build human-readable summary for the LLM
        exposure_pct = (
            (total_exposure / account_equity * 100) if account_equity > 0 else Decimal("0")
        )

        lines = [
            f"Account equity: ${account_equity:,.0f}",
            f"Open positions: {len(positions)}",
            f"Total options exposure: ${total_exposure:,.0f} ({exposure_pct:.1f}%)",
        ]

        if positions:
            lines.append("\nCurrent positions:")
            sector_counts: Counter = Counter()
            for p in positions:
                ticker = p.get("ticker", "?")
                right = p.get("right", "?")
                strike = p.get("strike", "?")
                expiry = p.get("expiry", "?")
                cost = p.get("cost_basis", Decimal("0"))
                pnl = p.get("unrealized_pnl", Decimal("0"))
                sector = sector_map.get(ticker, "unknown")
                sector_counts[sector] += 1
                lines.append(
                    f"  - {ticker} {strike}{right} exp {expiry} | "
                    f"cost ${cost:,.0f} | P&L ${pnl:+,.0f} | sector: {sector}"
                )

            lines.append("\nSector concentration:")
            for sector, count in sector_counts.most_common():
                lines.append(f"  - {sector}: {count} position(s)")
        else:
            lines.append("\nNo open positions.")

        ctx = {"portfolio_state": "\n".join(lines), "risk_feedback": ""}

        # Cross-ticker risk feedback
        if hasattr(self.db, "pool"):
            try:
                from db.feedback import FeedbackAggregator

                aggregator = FeedbackAggregator(self.db.pool)
                ctx["risk_feedback"] = await aggregator.build_risk_feedback()
            except Exception:
                logger.debug("Could not fetch risk feedback", exc_info=True)

        return ctx
