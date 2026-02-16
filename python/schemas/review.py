"""Position review schemas."""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, Field


class PositionReview(BaseModel):
    """Review of a single open position."""

    position_id: int
    ticker: str
    thesis_still_valid: bool
    pnl_pct: float
    recommended_action: Literal["hold", "add", "reduce", "close", "roll"]
    reasoning: str


class PortfolioReview(BaseModel):
    """Review of all open positions."""

    review_type: Literal["midday", "postmarket", "weekly"]
    position_reviews: list[PositionReview] = Field(default_factory=list)
    summary: str = ""
    urgent_actions: list[str] = Field(default_factory=list)


class WeeklyBattlePlan(BaseModel):
    """Saturday deep-dive output."""

    macro_view: str
    sector_analysis: list[dict] = Field(default_factory=list)
    performance_summary: str = ""
    top_ideas: list[dict] = Field(default_factory=list)
    focus_tickers: list[str] = Field(default_factory=list)
    lessons_learned: list[str] = Field(default_factory=list)
