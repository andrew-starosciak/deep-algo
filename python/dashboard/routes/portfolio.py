"""Portfolio overview and equity history endpoints."""

from __future__ import annotations

import asyncio
from decimal import Decimal

from fastapi import APIRouter, Depends, Query, Request

from dashboard.auth import verify_token
from dashboard.serialization import serialize_row

router = APIRouter(prefix="/api", tags=["portfolio"], dependencies=[Depends(verify_token)])


def _dec(v) -> str:
    if v is None:
        return "0"
    return str(v)


@router.get("/portfolio")
async def portfolio_overview(request: Request):
    db = request.app.state.db

    positions, total_exposure, total_realized, closed_count = await asyncio.gather(
        db.get_open_positions(),
        db.get_total_options_exposure(),
        db.get_total_realized_pnl(),
        db.get_closed_positions_count(),
    )

    total_unrealized = sum(Decimal(str(p.get("unrealized_pnl", 0))) for p in positions)
    total_cost = sum(Decimal(str(p.get("cost_basis", 0))) for p in positions)

    calls = [p for p in positions if p.get("right") == "call"]
    puts = [p for p in positions if p.get("right") == "put"]

    return {
        "open_positions": len(positions),
        "closed_trades": closed_count,
        "total_unrealized_pnl": _dec(total_unrealized),
        "total_realized_pnl": _dec(total_realized),
        "total_options_exposure": _dec(total_exposure),
        "total_cost_basis": _dec(total_cost),
        "calls_count": len(calls),
        "puts_count": len(puts),
        "positions": [serialize_row(p) for p in positions],
    }


@router.get("/portfolio/history")
async def portfolio_history(
    request: Request,
    days: int = Query(default=30, ge=1, le=365),
):
    db = request.app.state.db
    rows = await db.get_equity_history(days=days)
    return {"days": days, "snapshots": [serialize_row(r) for r in rows]}
