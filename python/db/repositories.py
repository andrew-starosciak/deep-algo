"""Database repositories — query patterns for workflow state and trading data."""

from __future__ import annotations

import json
import os
from decimal import Decimal

import asyncpg


class Database:
    """Async Postgres access for workflow state, research, theses, and positions."""

    def __init__(self, pool: asyncpg.Pool):
        self.pool = pool

    @classmethod
    async def connect(cls, dsn: str | None = None) -> Database:
        dsn = dsn or os.environ.get("DATABASE_URL", "postgres://localhost/algo_trade")
        pool = await asyncpg.create_pool(dsn)
        return cls(pool)

    async def close(self):
        await self.pool.close()

    # --- Workflow tracking ---

    async def create_workflow_run(
        self, workflow_id: str, trigger: str, input_data: dict
    ) -> int:
        return await self.pool.fetchval(
            """
            INSERT INTO workflow_runs (workflow_id, trigger, input, status)
            VALUES ($1, $2, $3, 'running')
            RETURNING id
            """,
            workflow_id,
            trigger,
            json.dumps(input_data, default=str),
        )

    async def complete_workflow_run(
        self, run_id: int, status: str = "completed", result: dict | None = None
    ):
        await self.pool.execute(
            """
            UPDATE workflow_runs
            SET status = $2, result = $3, completed_at = NOW()
            WHERE id = $1
            """,
            run_id,
            status,
            json.dumps(result, default=str) if result else None,
        )

    async def log_step(
        self,
        run_id: int,
        step_id: str,
        agent: str,
        attempt: int,
        input_data: dict,
        output_data: dict | None,
        passed_gate: bool,
        duration_ms: int,
    ):
        await self.pool.execute(
            """
            INSERT INTO workflow_step_logs
                (run_id, step_id, agent, attempt, input, output, passed_gate, duration_ms)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            """,
            run_id,
            step_id,
            agent,
            attempt,
            json.dumps(input_data, default=str),
            json.dumps(output_data, default=str) if output_data else None,
            passed_gate,
            duration_ms,
        )

    # --- Watchlist ---

    async def get_watchlist(self) -> list[dict]:
        rows = await self.pool.fetch("SELECT * FROM options_watchlist ORDER BY ticker")
        return [dict(r) for r in rows]

    async def add_to_watchlist(self, ticker: str, sector: str, notes: str | None = None):
        await self.pool.execute(
            """
            INSERT INTO options_watchlist (ticker, sector, notes)
            VALUES ($1, $2, $3)
            ON CONFLICT (ticker) DO UPDATE SET sector = $2, notes = $3
            """,
            ticker.upper(),
            sector,
            notes,
        )

    # --- Theses ---

    async def save_thesis(self, run_id: int, thesis: dict) -> int:
        return await self.pool.fetchval(
            """
            INSERT INTO theses
                (run_id, ticker, direction, thesis_text, catalyst, scores,
                 supporting_evidence, risks, overall_score)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id
            """,
            run_id,
            thesis["ticker"],
            thesis["direction"],
            thesis["thesis_text"],
            json.dumps(thesis.get("catalyst"), default=str),
            json.dumps(thesis["scores"], default=str),
            json.dumps(thesis.get("supporting_evidence", []), default=str),
            json.dumps(thesis.get("risks", []), default=str),
            thesis["scores"]["overall"],
        )

    # --- Recommendations ---

    async def save_recommendation(self, thesis_id: int, run_id: int, rec: dict) -> int:
        contract = rec["contract"]
        return await self.pool.fetchval(
            """
            INSERT INTO trade_recommendations
                (thesis_id, run_id, ticker, right, strike, expiry,
                 entry_price_low, entry_price_high, position_size_pct,
                 position_size_usd, exit_targets, stop_loss, max_hold_days,
                 risk_verification, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, 'pending_review')
            RETURNING id
            """,
            thesis_id,
            run_id,
            contract["ticker"],
            contract["right"],
            float(contract["strike"]),
            contract["expiry"],
            float(contract["entry_price_low"]),
            float(contract["entry_price_high"]),
            float(rec["position_size_pct"]),
            float(rec["position_size_usd"]),
            json.dumps(rec.get("exit_targets", []), default=str),
            rec.get("stop_loss", ""),
            rec.get("max_hold_days", 30),
            json.dumps(rec.get("risk_verification"), default=str),
        )

    # --- Positions ---

    async def get_open_positions(self) -> list[dict]:
        rows = await self.pool.fetch(
            """
            SELECT * FROM options_positions
            WHERE status = 'open'
            ORDER BY opened_at ASC
            """
        )
        return [dict(r) for r in rows]

    async def insert_position(self, position: dict) -> int:
        return await self.pool.fetchval(
            """
            INSERT INTO options_positions
                (recommendation_id, ticker, right, strike, expiry,
                 quantity, avg_fill_price, current_price, cost_basis,
                 unrealized_pnl, status, opened_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'open', NOW())
            RETURNING id
            """,
            position["recommendation_id"],
            position["ticker"],
            position["right"],
            position["strike"],
            position["expiry"],
            position["quantity"],
            position["avg_fill_price"],
            position["current_price"],
            position["cost_basis"],
            position.get("unrealized_pnl", Decimal("0")),
        )

    async def update_position_price(
        self, position_id: int, current_price: Decimal, unrealized_pnl: Decimal
    ):
        await self.pool.execute(
            """
            UPDATE options_positions
            SET current_price = $2, unrealized_pnl = $3, updated_at = NOW()
            WHERE id = $1
            """,
            position_id,
            current_price,
            unrealized_pnl,
        )

    async def partial_close_position(
        self, position_id: int, new_quantity: int, realized_pnl: Decimal
    ):
        """Update a position after a partial close (reduce quantity, add realized P&L)."""
        await self.pool.execute(
            """
            UPDATE options_positions
            SET quantity = $2, realized_pnl = realized_pnl + $3, updated_at = NOW()
            WHERE id = $1
            """,
            position_id,
            new_quantity,
            realized_pnl,
        )

    async def close_position(self, position_id: int, reason: str, realized_pnl: Decimal):
        await self.pool.execute(
            """
            UPDATE options_positions
            SET status = 'closed', close_reason = $2, realized_pnl = $3, closed_at = NOW()
            WHERE id = $1
            """,
            position_id,
            reason,
            realized_pnl,
        )

    async def get_total_options_exposure(self) -> Decimal:
        val = await self.pool.fetchval(
            "SELECT COALESCE(SUM(cost_basis), 0) FROM options_positions WHERE status = 'open'"
        )
        return Decimal(str(val))

    # --- Recommendations (approval + status) ---

    async def get_approved_recommendations(self) -> list[dict]:
        rows = await self.pool.fetch(
            """
            SELECT * FROM trade_recommendations
            WHERE status = 'approved'
            ORDER BY approved_at ASC
            """
        )
        return [dict(r) for r in rows]

    async def update_recommendation_status(
        self, rec_id: int, status: str, reason: str | None = None
    ):
        await self.pool.execute(
            """
            UPDATE trade_recommendations
            SET status = $2, rejected_reason = $3
            WHERE id = $1
            """,
            rec_id,
            status,
            reason,
        )

    async def approve_recommendation(self, rec_id: int):
        await self.pool.execute(
            """
            UPDATE trade_recommendations
            SET status = 'approved', approved_at = NOW()
            WHERE id = $1
            """,
            rec_id,
        )

    # --- Recent workflow runs ---

    async def recent_runs(self, limit: int = 20) -> list[dict]:
        rows = await self.pool.fetch(
            """
            SELECT id, workflow_id, trigger, status, started_at, completed_at
            FROM workflow_runs
            ORDER BY started_at DESC
            LIMIT $1
            """,
            limit,
        )
        return [dict(r) for r in rows]
