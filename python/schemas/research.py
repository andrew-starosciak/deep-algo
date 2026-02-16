"""Research pipeline schemas."""

import datetime as _dt
from decimal import Decimal
from typing import Literal, Optional

from pydantic import BaseModel, Field


class TechnicalLevels(BaseModel):
    """Key technical analysis levels for an underlying."""

    price: Decimal
    ma_20: Decimal = Decimal("0")
    ma_50: Decimal = Decimal("0")
    ma_200: Decimal = Decimal("0")
    rsi_14: float = 50.0
    support: Decimal = Decimal("0")
    resistance: Decimal = Decimal("0")
    above_all_mas: bool = False
    trend: Literal["bullish", "bearish", "neutral"] = "neutral"


class OptionsFlowSummary(BaseModel):
    """Summary of unusual options activity."""

    unusual_activity: bool = False
    notable_trades: list[str] = Field(default_factory=list)
    put_call_ratio: float = 1.0
    total_premium_usd: Decimal = Decimal("0")


class Catalyst(BaseModel):
    """An upcoming catalyst event."""

    type: Literal["earnings", "fda", "fed", "macro", "other"]
    date: Optional[_dt.date] = None
    days_until: Optional[int] = None
    description: str


class NewsItem(BaseModel):
    """A single news item."""

    headline: str
    source: str
    timestamp: Optional[_dt.datetime] = None
    relevance_score: float = 0.5


class ResearchRequest(BaseModel):
    """Input to the research pipeline."""

    ticker: str
    mode: Literal["premarket", "midday", "postmarket", "weekly_deep_dive"] = "premarket"


class ResearchSummary(BaseModel):
    """Structured output from the researcher agent."""

    ticker: str
    timestamp: _dt.datetime
    news_summary: str = ""
    news_items: list[NewsItem] = Field(default_factory=list)
    technicals: TechnicalLevels
    options_flow: OptionsFlowSummary = Field(default_factory=OptionsFlowSummary)
    catalyst: Optional[Catalyst] = None
    macro_context: str = ""
    iv_rank: float = Field(default=50.0, ge=0.0, le=100.0)
    opportunity_score: int = Field(default=5, ge=1, le=10)
    key_observations: list[str] = Field(default_factory=list)
