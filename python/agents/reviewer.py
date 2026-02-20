"""Reviewer agent — evaluates open positions and recommends actions."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel

from agents.base import BaseAgent
from openclaw.llm import LLMClient


class ReviewerAgent(BaseAgent):
    """Reviews open positions: thesis validity, P&L, recommended actions."""

    def __init__(self, llm: LLMClient, db: Any = None):
        super().__init__(llm, db)
        self._position: dict | None = None

    def set_review_context(self, position: dict) -> None:
        """Stash position details before the workflow runs."""
        self._position = position

    @property
    def role(self) -> str:
        return (
            "portfolio manager reviewing open options positions. "
            "For each position, you evaluate whether the original thesis is still valid, "
            "assess P&L trajectory, and recommend one of: hold, add, reduce, close, or roll. "
            "You consider new information, technical changes, and theta decay impact."
        )

    @property
    def prompt_file(self) -> str:
        return "position_review.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Pull position details, original thesis, and previous reviews from DB."""
        if self.db is None or self._position is None:
            return {
                "positions": "No position context available.",
                "original_thesis": "N/A",
                "previous_reviews": "None",
            }

        pos = self._position
        position_id = pos.get("id")

        # Format current position details
        days_held = ""
        if pos.get("opened_at"):
            import datetime as _dt

            opened = pos["opened_at"]
            if hasattr(opened, "date"):
                delta = _dt.datetime.now(_dt.timezone.utc) - opened
                days_held = f", held {delta.days} days"

        # Compute P&L percentage for the LLM
        cost_basis = pos.get("cost_basis")
        unrealized = pos.get("unrealized_pnl")
        pnl_pct_str = "N/A"
        if cost_basis and unrealized:
            try:
                from decimal import Decimal as D
                cb = D(str(cost_basis))
                ur = D(str(unrealized))
                if cb:
                    pnl_pct_str = f"{float(ur / cb * 100):+.1f}%"
            except Exception:
                pass

        position_text = (
            f"Position ID: {position_id} | Ticker: {pos.get('ticker')}\n"
            f"**{pos.get('ticker')}** {pos.get('right', '?')} "
            f"${pos.get('strike', '?')} exp {pos.get('expiry', '?')}\n"
            f"  Qty: {pos.get('quantity', '?')} | "
            f"Cost basis: ${pos.get('cost_basis', '?')} | "
            f"Current: ${pos.get('current_price', '?')} | "
            f"Unrealized P&L: ${pos.get('unrealized_pnl', '?')} ({pnl_pct_str})"
            f"{days_held}"
        )

        # Fetch original thesis
        original_thesis = "No linked thesis found."
        if position_id:
            thesis_row = await self.db.get_thesis_for_position(position_id)
            if thesis_row:
                original_thesis = (
                    f"**Direction:** {thesis_row.get('direction', '?')}\n"
                    f"**Score:** {thesis_row.get('overall_score', '?')}/10\n"
                    f"**Thesis:** {thesis_row.get('thesis_text', 'N/A')}"
                )

        # Fetch previous reviews for trend tracking
        previous_reviews = "No previous reviews."
        if position_id:
            reviews = await self.db.get_recent_reviews(position_id, limit=3)
            if reviews:
                lines = []
                for r in reviews:
                    lines.append(
                        f"- [{r.get('created_at', '?')}] "
                        f"Action: **{r.get('recommended_action', '?')}** | "
                        f"Thesis valid: {r.get('thesis_still_valid', '?')} | "
                        f"{r.get('reasoning', '')[:200]}"
                    )
                previous_reviews = "\n".join(lines)

        return {
            "positions": position_text,
            "original_thesis": original_thesis,
            "previous_reviews": previous_reviews,
        }
