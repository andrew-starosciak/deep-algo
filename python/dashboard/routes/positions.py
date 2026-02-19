"""Position endpoints."""

from __future__ import annotations

from typing import Literal

from fastapi import APIRouter, Depends, HTTPException, Request

from dashboard.auth import verify_token
from dashboard.serialization import serialize_row

router = APIRouter(prefix="/api", tags=["positions"], dependencies=[Depends(verify_token)])


@router.get("/positions")
async def list_positions(
    request: Request,
    status: Literal["open", "closed", "all"] = "open",
):
    db = request.app.state.db
    s = status if status != "all" else None
    rows = await db.get_all_positions(status=s)
    return {"count": len(rows), "positions": [serialize_row(r) for r in rows]}


@router.get("/positions/{position_id}")
async def get_position(request: Request, position_id: int):
    db = request.app.state.db
    row = await db.get_position_by_id(position_id)
    if not row:
        raise HTTPException(status_code=404, detail="Position not found")
    return serialize_row(row)
