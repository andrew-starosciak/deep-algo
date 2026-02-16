"""Trade recommendation schemas — the bridge between Python (LLM) and Rust (execution)."""

from __future__ import annotations

from datetime import date
from decimal import Decimal
from typing import Literal

from pydantic import BaseModel, Field

from schemas.thesis import ContractSpec


class TradeRecommendation(BaseModel):
    """A recommendation ready for human review and eventual execution."""

    thesis_id: int
    contract: ContractSpec
    position_size_pct: Decimal = Field(le=Decimal("2.0"), description="Max 2% of account")
    position_size_usd: Decimal
    exit_targets: list[str] = Field(default_factory=list)
    stop_loss: str = ""
    max_hold_days: int = 30
    status: Literal[
        "pending_review", "approved", "rejected", "executing", "filled", "failed"
    ] = "pending_review"
