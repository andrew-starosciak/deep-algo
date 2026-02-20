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

    # --- Research summaries ---

    async def save_research_summary(
        self, run_id: int, ticker: str, mode: str, summary: dict, opportunity_score: int
    ) -> int:
        """Persist a research summary to the research_summaries table."""
        return await self.pool.fetchval(
            """
            INSERT INTO research_summaries (run_id, ticker, mode, summary, opportunity_score)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            """,
            run_id,
            ticker.upper(),
            mode,
            json.dumps(summary, default=str),
            opportunity_score,
        )

    # --- Theses ---

    async def save_thesis(self, run_id: int, thesis: dict) -> int:
        return await self.pool.fetchval(
            """
            INSERT INTO theses
                (run_id, ticker, direction, thesis_text, catalyst, scores,
                 supporting_evidence, risks, overall_score,
                 analyst_reasoning, critic_reasoning)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
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
            thesis.get("analyst_reasoning"),
            thesis.get("critic_reasoning"),
        )

    # --- Recommendations ---

    async def save_recommendation(self, thesis_id: int, run_id: int, rec: dict) -> int:
        import datetime as _dt

        contract = rec["contract"]
        expiry = contract["expiry"]
        if isinstance(expiry, str):
            expiry = _dt.date.fromisoformat(expiry)
        return await self.pool.fetchval(
            """
            INSERT INTO trade_recommendations
                (thesis_id, run_id, ticker, "right", strike, expiry,
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
            expiry,
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
                (recommendation_id, ticker, "right", strike, expiry,
                 quantity, avg_fill_price, current_price, cost_basis,
                 unrealized_pnl, ib_con_id, status, opened_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'open', NOW())
            RETURNING id
            """,
            position.get("recommendation_id"),
            position["ticker"],
            position["right"],
            position["strike"],
            position["expiry"],
            position["quantity"],
            position["avg_fill_price"],
            position["current_price"],
            position["cost_basis"],
            position.get("unrealized_pnl", Decimal("0")),
            position.get("ib_con_id"),
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

    # --- Position sync ---

    async def get_position_by_con_id(self, con_id: int) -> dict | None:
        """Find an open position by IB contract ID."""
        row = await self.pool.fetchrow(
            """
            SELECT * FROM options_positions
            WHERE ib_con_id = $1 AND status = 'open'
            """,
            con_id,
        )
        return dict(row) if row else None

    async def find_position_by_contract(
        self, ticker: str, right: str, strike, expiry
    ) -> dict | None:
        """Fallback: match by contract details when con_id not available."""
        row = await self.pool.fetchrow(
            """
            SELECT * FROM options_positions
            WHERE ticker = $1 AND "right" = $2 AND strike = $3 AND expiry = $4
              AND status = 'open'
            ORDER BY opened_at DESC
            LIMIT 1
            """,
            ticker,
            right,
            float(strike),
            expiry,
        )
        return dict(row) if row else None

    async def update_position_con_id(self, position_id: int, con_id: int):
        """Backfill ib_con_id on an existing position."""
        await self.pool.execute(
            "UPDATE options_positions SET ib_con_id = $2 WHERE id = $1",
            position_id,
            con_id,
        )

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

    async def reject_recommendation(self, rec_id: int, reason: str | None = None):
        await self.pool.execute(
            """
            UPDATE trade_recommendations
            SET status = 'rejected', rejected_reason = $2
            WHERE id = $1
            """,
            rec_id,
            reason,
        )

    async def get_recommendation(self, rec_id: int) -> dict | None:
        row = await self.pool.fetchrow(
            "SELECT * FROM trade_recommendations WHERE id = $1",
            rec_id,
        )
        if not row:
            return None
        rec = dict(row)
        # Parse JSON fields
        for key in ("exit_targets", "risk_verification"):
            if isinstance(rec.get(key), str):
                try:
                    rec[key] = json.loads(rec[key])
                except (json.JSONDecodeError, TypeError):
                    pass
        return rec

    async def get_pending_recommendations(self) -> list[dict]:
        rows = await self.pool.fetch(
            """
            SELECT * FROM trade_recommendations
            WHERE status = 'pending_review'
            ORDER BY created_at DESC
            """
        )
        return [dict(r) for r in rows]

    async def get_total_realized_pnl(self) -> Decimal:
        """Sum realized P&L across all closed positions."""
        val = await self.pool.fetchval(
            "SELECT COALESCE(SUM(realized_pnl), 0) FROM options_positions WHERE status = 'closed'"
        )
        return Decimal(str(val))

    async def get_closed_positions_count(self) -> int:
        val = await self.pool.fetchval(
            "SELECT COUNT(*) FROM options_positions WHERE status = 'closed'"
        )
        return int(val)

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

    async def get_workflow_runs_with_steps(self, limit: int = 20) -> list[dict]:
        """Fetch recent workflow runs with their step logs grouped under each run."""
        rows = await self.pool.fetch(
            """
            SELECT
                r.id, r.workflow_id, r.trigger, r.status,
                r.started_at, r.completed_at,
                s.step_id, s.agent, s.passed_gate, s.duration_ms AS step_duration_ms,
                s.attempt
            FROM workflow_runs r
            LEFT JOIN workflow_step_logs s ON s.run_id = r.id
            ORDER BY r.started_at DESC, s.id ASC
            """,
        )
        # Group rows by run id, preserving order
        from collections import OrderedDict

        runs_map: OrderedDict[int, dict] = OrderedDict()
        for row in rows:
            rid = row["id"]
            if rid not in runs_map:
                dur = row["completed_at"] and row["started_at"] and (
                    row["completed_at"] - row["started_at"]
                )
                runs_map[rid] = {
                    "id": rid,
                    "workflow_id": row["workflow_id"],
                    "trigger": row["trigger"],
                    "status": row["status"],
                    "started_at": row["started_at"],
                    "completed_at": row["completed_at"],
                    "duration_ms": int(dur.total_seconds() * 1000) if dur else None,
                    "steps": [],
                }
            if row["step_id"] is not None:
                runs_map[rid]["steps"].append({
                    "step_id": row["step_id"],
                    "agent": row["agent"],
                    "passed_gate": row["passed_gate"],
                    "duration_ms": int(row["step_duration_ms"]) if row["step_duration_ms"] is not None else None,
                    "attempt": row["attempt"],
                })

        result = list(runs_map.values())
        if len(result) > limit:
            result = result[:limit]
        return result

    # --- Equity snapshots ---

    async def insert_equity_snapshot(
        self,
        net_liquidation: Decimal,
        total_unrealized_pnl: Decimal,
        total_realized_pnl: Decimal,
        open_positions_count: int,
        total_options_exposure: Decimal,
    ):
        await self.pool.execute(
            """
            INSERT INTO equity_snapshots
                (timestamp, net_liquidation, total_unrealized_pnl, total_realized_pnl,
                 open_positions_count, total_options_exposure)
            VALUES (NOW(), $1, $2, $3, $4, $5)
            ON CONFLICT (timestamp) DO NOTHING
            """,
            net_liquidation,
            total_unrealized_pnl,
            total_realized_pnl,
            open_positions_count,
            total_options_exposure,
        )

    async def get_equity_history(self, days: int = 30) -> list[dict]:
        rows = await self.pool.fetch(
            """
            SELECT timestamp, net_liquidation, total_unrealized_pnl,
                   total_realized_pnl, open_positions_count, total_options_exposure
            FROM equity_snapshots
            WHERE timestamp > NOW() - make_interval(days => $1)
            ORDER BY timestamp ASC
            """,
            days,
        )
        return [dict(r) for r in rows]

    # --- Dashboard queries ---

    async def get_all_positions(self, status: str | None = None) -> list[dict]:
        if status:
            rows = await self.pool.fetch(
                "SELECT * FROM options_positions WHERE status = $1 ORDER BY opened_at DESC",
                status,
            )
        else:
            rows = await self.pool.fetch(
                "SELECT * FROM options_positions ORDER BY opened_at DESC"
            )
        return [dict(r) for r in rows]

    async def get_position_by_id(self, position_id: int) -> dict | None:
        row = await self.pool.fetchrow(
            "SELECT * FROM options_positions WHERE id = $1", position_id
        )
        return dict(row) if row else None

    async def get_theses(
        self, ticker: str | None = None, limit: int = 50
    ) -> list[dict]:
        if ticker:
            rows = await self.pool.fetch(
                """
                SELECT * FROM theses
                WHERE ticker = $1
                ORDER BY created_at DESC
                LIMIT $2
                """,
                ticker.upper(),
                limit,
            )
        else:
            rows = await self.pool.fetch(
                "SELECT * FROM theses ORDER BY created_at DESC LIMIT $1", limit
            )
        return [dict(r) for r in rows]

    async def get_all_recommendations(self, status: str | None = None) -> list[dict]:
        if status and status != "all":
            rows = await self.pool.fetch(
                """
                SELECT * FROM trade_recommendations
                WHERE status = $1
                ORDER BY created_at DESC
                """,
                status,
            )
        else:
            rows = await self.pool.fetch(
                "SELECT * FROM trade_recommendations ORDER BY created_at DESC"
            )
        return [dict(r) for r in rows]

    # --- Thesis outcomes ---

    async def get_thesis_history_with_outcomes(
        self, ticker: str, limit: int = 5
    ) -> list[dict]:
        """Fetch recent theses for a ticker, including outcome columns from V016."""
        rows = await self.pool.fetch(
            """
            SELECT id, ticker, direction, thesis_text, overall_score,
                   outcome_realized_pnl, outcome_close_reason, outcome_closed_at,
                   created_at
            FROM theses
            WHERE ticker = $1
            ORDER BY created_at DESC
            LIMIT $2
            """,
            ticker.upper(),
            limit,
        )
        return [dict(r) for r in rows]

    async def get_thesis_id_for_position(self, position_id: int) -> int | None:
        """Walk FK chain: positions → recommendations → theses."""
        return await self.pool.fetchval(
            """
            SELECT t.id
            FROM options_positions p
            JOIN trade_recommendations r ON r.id = p.recommendation_id
            JOIN theses t ON t.id = r.thesis_id
            WHERE p.id = $1
            """,
            position_id,
        )

    async def update_thesis_outcome(
        self,
        thesis_id: int,
        realized_pnl: Decimal,
        close_reason: str,
        position_id: int,
    ):
        """Record the outcome of a thesis when its position closes."""
        await self.pool.execute(
            """
            UPDATE theses
            SET outcome_realized_pnl = $2,
                outcome_close_reason = $3,
                outcome_closed_at = NOW(),
                outcome_position_id = $4
            WHERE id = $1
            """,
            thesis_id,
            realized_pnl,
            close_reason,
            position_id,
        )

    async def get_open_positions_for_ticker(self, ticker: str) -> list[dict]:
        """Get all open positions for a specific ticker."""
        rows = await self.pool.fetch(
            """
            SELECT * FROM options_positions
            WHERE ticker = $1 AND status = 'open'
            ORDER BY opened_at ASC
            """,
            ticker.upper(),
        )
        return [dict(r) for r in rows]

    async def save_position_review(
        self,
        run_id: int,
        position_id: int,
        review_type: str,
        review_data: dict,
    ) -> int:
        """Persist a position review to the position_reviews table."""
        return await self.pool.fetchval(
            """
            INSERT INTO position_reviews
                (run_id, position_id, review_type, thesis_still_valid,
                 recommended_action, reasoning)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            """,
            run_id,
            position_id,
            review_type,
            review_data.get("thesis_still_valid"),
            review_data.get("recommended_action"),
            review_data.get("reasoning"),
        )

    async def get_thesis_for_position(self, position_id: int) -> dict | None:
        """Walk FK chain: positions → recommendations → theses. Returns full thesis row."""
        row = await self.pool.fetchrow(
            """
            SELECT t.*
            FROM options_positions p
            JOIN trade_recommendations r ON r.id = p.recommendation_id
            JOIN theses t ON t.id = r.thesis_id
            WHERE p.id = $1
            """,
            position_id,
        )
        return dict(row) if row else None

    async def get_recent_reviews(self, position_id: int, limit: int = 5) -> list[dict]:
        """Get recent position reviews for trend tracking."""
        rows = await self.pool.fetch(
            """
            SELECT * FROM position_reviews
            WHERE position_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            """,
            position_id,
            limit,
        )
        return [dict(r) for r in rows]

    async def get_research_memory_stats(self) -> dict:
        """Get aggregate stats about the research memory / feedback loop."""
        row = await self.pool.fetchrow(
            """
            SELECT
                (SELECT COUNT(*) FROM research_summaries) AS total_research,
                (SELECT COUNT(*) FROM theses) AS total_theses,
                (SELECT COUNT(*) FROM theses WHERE outcome_realized_pnl IS NOT NULL) AS theses_with_outcome,
                (SELECT COUNT(*) FROM theses WHERE outcome_realized_pnl > 0) AS winning_theses,
                (SELECT COUNT(*) FROM theses WHERE outcome_realized_pnl <= 0) AS losing_theses,
                (SELECT COALESCE(SUM(outcome_realized_pnl), 0) FROM theses WHERE outcome_realized_pnl IS NOT NULL) AS total_outcome_pnl,
                (SELECT COUNT(DISTINCT ticker) FROM theses) AS tickers_analyzed,
                (SELECT COUNT(*) FROM trade_recommendations) AS total_recommendations,
                (SELECT COUNT(*) FROM trade_recommendations WHERE status = 'approved') AS approved_recommendations,
                (SELECT COUNT(*) FROM trade_recommendations WHERE status = 'filled') AS filled_recommendations
            """
        )
        return dict(row) if row else {}
