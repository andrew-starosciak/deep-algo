"""System status and watchlist endpoints."""

from __future__ import annotations

from decimal import Decimal

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


@router.get("/research-memory")
async def research_memory(request: Request):
    db = request.app.state.db
    try:
        stats = await db.get_research_memory_stats()
        return {k: str(v) if isinstance(v, Decimal) else v for k, v in stats.items()}
    except Exception:
        return {
            "total_research": 0, "total_theses": 0, "theses_with_outcome": 0,
            "winning_theses": 0, "losing_theses": 0, "total_outcome_pnl": "0",
            "tickers_analyzed": 0, "total_recommendations": 0,
            "approved_recommendations": 0, "filled_recommendations": 0,
        }


@router.get("/workflows")
async def workflows(request: Request):
    db = request.app.state.db
    runs = await db.get_workflow_runs_with_steps(limit=20)

    total = len(runs)
    completed = sum(1 for r in runs if r["status"] == "completed")
    failed = sum(1 for r in runs if r["status"] == "failed")
    durations = [r["duration_ms"] for r in runs if r["duration_ms"] is not None]
    avg_duration = int(sum(durations) / len(durations)) if durations else 0

    from datetime import date, datetime

    today = date.today()
    runs_today = sum(
        1 for r in runs
        if r.get("started_at") and (
            r["started_at"].date() == today
            if isinstance(r["started_at"], datetime)
            else False
        )
    )

    # Get last equity snapshot timestamp for position manager health
    try:
        last_tick = await db.pool.fetchval(
            "SELECT MAX(timestamp) FROM equity_snapshots"
        )
    except Exception:
        last_tick = None

    serialized_runs = []
    for r in runs:
        sr = serialize_row(r)
        sr["steps"] = [serialize_row(s) for s in r.get("steps", [])]
        serialized_runs.append(sr)

    return {
        "runs": serialized_runs,
        "stats": {
            "total_runs": total,
            "completed": completed,
            "failed": failed,
            "avg_duration_ms": avg_duration,
            "runs_today": runs_today,
        },
        "last_equity_tick": last_tick.isoformat() if last_tick else None,
    }
