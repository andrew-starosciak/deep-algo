"""System status and watchlist endpoints."""

from __future__ import annotations

from fastapi import APIRouter, Depends, Request

from dashboard.auth import verify_token
from dashboard.serialization import serialize_row

router = APIRouter(prefix="/api", tags=["system"], dependencies=[Depends(verify_token)])


@router.get("/status")
async def system_status(request: Request):
    db = request.app.state.db
    try:
        await db.pool.fetchval("SELECT 1")
        db_connected = True
    except Exception:
        db_connected = False

    recent = await db.recent_runs(limit=5)
    return {
        "db_connected": db_connected,
        "recent_workflows": [serialize_row(r) for r in recent],
    }


@router.get("/watchlist")
async def watchlist(request: Request):
    db = request.app.state.db
    rows = await db.get_watchlist()
    return {"count": len(rows), "watchlist": [serialize_row(r) for r in rows]}
