"""Risk verification schemas."""

from decimal import Decimal
from typing import Optional

from pydantic import BaseModel, Field


class RiskVerification(BaseModel):
    """Output of the risk checker agent."""

    approved: bool
    position_size_pct: Decimal = Field(description="Possibly adjusted down")
    total_exposure_pct: Decimal = Field(description="Total swing options / account equity")
    correlated_positions: int = Field(description="Open positions in same sector")
    cross_platform_notes: list[str] = Field(
        default_factory=list,
        description="Correlation flags with HyperLiquid/Polymarket positions",
    )
    rejection_reason: Optional[str] = None
