"""Research, theses, and recommendation endpoints."""

from __future__ import annotations

from fastapi import APIRouter, Depends, Query, Request

from dashboard.auth import verify_token
from dashboard.serialization import serialize_row

router = APIRouter(prefix="/api", tags=["research"], dependencies=[Depends(verify_token)])


@router.get("/theses")
async def list_theses(
    request: Request,
    ticker: str | None = None,
    limit: int = Query(default=50, ge=1, le=500),
):
    db = request.app.state.db
    rows = await db.get_theses(ticker=ticker, limit=limit)
    return {"count": len(rows), "theses": [serialize_row(r, parse_json=True) for r in rows]}


@router.get("/theses/{ticker}")
async def theses_for_ticker(request: Request, ticker: str):
    db = request.app.state.db
    rows = await db.get_theses(ticker=ticker.upper(), limit=100)
    return {
        "ticker": ticker.upper(),
        "count": len(rows),
        "theses": [serialize_row(r, parse_json=True) for r in rows],
    }


@router.get("/recommendations")
async def list_recommendations(request: Request, status: str = "all"):
    db = request.app.state.db
    rows = await db.get_all_recommendations(status=status)
    return {
        "count": len(rows),
        "recommendations": [serialize_row(r, parse_json=True) for r in rows],
    }
