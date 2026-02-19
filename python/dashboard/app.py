"""FastAPI dashboard — readonly view of IB options portfolio."""

from __future__ import annotations

import os
from contextlib import asynccontextmanager

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from db.repositories import Database

from .routes import portfolio, positions, research, system


@asynccontextmanager
async def lifespan(app: FastAPI):
    dsn = os.environ.get("DATABASE_URL", "postgres://localhost/algo_trade")
    app.state.db = await Database.connect(dsn)
    yield
    await app.state.db.close()


app = FastAPI(title="OpenClaw Dashboard", lifespan=lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["GET"],
    allow_headers=["Authorization"],
)

app.include_router(portfolio.router)
app.include_router(positions.router)
app.include_router(research.router)
app.include_router(system.router)


@app.get("/api/health")
async def health():
    return {"status": "ok"}
