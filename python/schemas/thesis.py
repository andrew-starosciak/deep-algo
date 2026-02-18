"""Thesis evaluation schemas."""

import datetime as _dt
from decimal import Decimal
from typing import Literal, Optional

from pydantic import BaseModel, Field, model_validator

from schemas.research import Catalyst


class ThesisScore(BaseModel):
    """Scoring dimensions for a trade thesis."""

    information_edge: int = Field(ge=1, le=10, description="1-10: Do we see what others don't?")
    volatility_pricing: int = Field(ge=1, le=10, description="1-10: Is vol cheap (low IV rank)?")
    technical_alignment: int = Field(ge=1, le=10, description="1-10: Does chart support thesis?")
    catalyst_clarity: int = Field(ge=1, le=10, description="1-10: How specific is the catalyst?")
    risk_reward_ratio: float = Field(ge=0.0, description="Expected gain / max loss")
    overall: float = Field(default=0.0, description="Weighted composite score")

    @model_validator(mode="after")
    def compute_overall(self):
        """Weighted: info_edge 30%, vol 20%, tech 20%, catalyst 30%."""
        self.overall = round(
            self.information_edge * 0.30
            + self.volatility_pricing * 0.20
            + self.technical_alignment * 0.20
            + self.catalyst_clarity * 0.30,
            2,
        )
        return self


class ContractSpec(BaseModel):
    """Specific options contract recommendation."""

    ticker: str
    right: Literal["call", "put"]
    strike: Decimal
    expiry: _dt.date
    strategy: Literal["naked", "debit_spread"] = "naked"
    entry_price_low: Decimal
    entry_price_high: Decimal

    def __str__(self) -> str:
        side = self.right[0].upper()
        base = (
            f"{self.ticker} {self.strike}{side} "
            f"{self.expiry.strftime('%b %d')} "
            f"${self.entry_price_low}-${self.entry_price_high}"
        )
        if self.strategy != "naked":
            base += f" ({self.strategy.replace('_', ' ')})"
        return base


class Thesis(BaseModel):
    """A scored trade thesis with optional contract recommendation."""

    ticker: str
    direction: Literal["bullish", "bearish"]
    thesis_text: str
    catalyst: Catalyst
    scores: ThesisScore
    supporting_evidence: list[str] = Field(default_factory=list)
    risks: list[str] = Field(default_factory=list)
    recommended_contract: Optional[ContractSpec] = None
