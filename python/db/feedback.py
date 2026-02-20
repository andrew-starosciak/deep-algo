"""Cross-ticker feedback aggregator — extracts patterns from historical outcomes."""

from __future__ import annotations

import logging

import asyncpg

logger = logging.getLogger(__name__)

MIN_CLOSED_POSITIONS = 5


class FeedbackAggregator:
    """Extract cross-ticker patterns from historical outcomes for prompt injection."""

    def __init__(self, pool: asyncpg.Pool):
        self.pool = pool

    async def _has_enough_data(self) -> bool:
        count = await self.pool.fetchval(
            "SELECT COUNT(*) FROM theses WHERE outcome_realized_pnl IS NOT NULL"
        )
        return (count or 0) >= MIN_CLOSED_POSITIONS

    async def _holding_periods(self) -> dict | None:
        """Avg holding period (days) for winners vs losers."""
        return await self.pool.fetchrow(
            """
            SELECT
                AVG(CASE WHEN t.outcome_realized_pnl > 0
                    THEN EXTRACT(EPOCH FROM t.outcome_closed_at - p.opened_at) / 86400 END) AS win_days,
                AVG(CASE WHEN t.outcome_realized_pnl <= 0
                    THEN EXTRACT(EPOCH FROM t.outcome_closed_at - p.opened_at) / 86400 END) AS lose_days
            FROM theses t
            JOIN trade_recommendations r ON r.thesis_id = t.id
            JOIN options_positions p ON p.recommendation_id = r.id
            WHERE t.outcome_realized_pnl IS NOT NULL
            """
        )

    @staticmethod
    def _append_holding_period(sections: list[str], row) -> None:
        if row and (row["win_days"] is not None or row["lose_days"] is not None):
            lines = ["**Holding period (avg days):**"]
            if row["win_days"] is not None:
                lines.append(f"  - Winners: {row['win_days']:.1f} days")
            if row["lose_days"] is not None:
                lines.append(f"  - Losers: {row['lose_days']:.1f} days")
            sections.append("\n".join(lines))

    async def build_analyst_feedback(self) -> str:
        """Aggregate cross-ticker patterns for the analyst prompt."""
        if not await self._has_enough_data():
            return ""

        sections = []

        # 1. Win rate by score bucket
        rows = await self.pool.fetch(
            """
            SELECT
                FLOOR(overall_score)::int AS score_bucket,
                COUNT(*) AS total,
                COUNT(*) FILTER (WHERE outcome_realized_pnl > 0) AS wins
            FROM theses
            WHERE outcome_realized_pnl IS NOT NULL
            GROUP BY 1
            ORDER BY 1
            """
        )
        if rows:
            lines = ["**Win rate by score bucket:**"]
            for r in rows:
                bucket = r["score_bucket"]
                total = r["total"]
                wins = r["wins"]
                rate = wins / total * 100 if total else 0
                lines.append(f"  - Score {bucket}.x: {wins}/{total} ({rate:.0f}% win rate)")
            sections.append("\n".join(lines))

        # 2. Win rate by direction
        rows = await self.pool.fetch(
            """
            SELECT
                direction,
                COUNT(*) AS total,
                COUNT(*) FILTER (WHERE outcome_realized_pnl > 0) AS wins
            FROM theses
            WHERE outcome_realized_pnl IS NOT NULL
            GROUP BY 1
            """
        )
        if rows:
            lines = ["**Win rate by direction:**"]
            for r in rows:
                total = r["total"]
                wins = r["wins"]
                rate = wins / total * 100 if total else 0
                lines.append(f"  - {r['direction']}: {wins}/{total} ({rate:.0f}%)")
            sections.append("\n".join(lines))

        # 3. Score dimension accuracy (avg per dimension for winners vs losers)
        row = await self.pool.fetchrow(
            """
            SELECT
                AVG(CASE WHEN outcome_realized_pnl > 0
                    THEN (scores->>'information_edge')::float END) AS win_info,
                AVG(CASE WHEN outcome_realized_pnl <= 0
                    THEN (scores->>'information_edge')::float END) AS lose_info,
                AVG(CASE WHEN outcome_realized_pnl > 0
                    THEN (scores->>'catalyst_clarity')::float END) AS win_catalyst,
                AVG(CASE WHEN outcome_realized_pnl <= 0
                    THEN (scores->>'catalyst_clarity')::float END) AS lose_catalyst,
                AVG(CASE WHEN outcome_realized_pnl > 0
                    THEN (scores->>'volatility_pricing')::float END) AS win_vol,
                AVG(CASE WHEN outcome_realized_pnl <= 0
                    THEN (scores->>'volatility_pricing')::float END) AS lose_vol,
                AVG(CASE WHEN outcome_realized_pnl > 0
                    THEN (scores->>'technical_alignment')::float END) AS win_tech,
                AVG(CASE WHEN outcome_realized_pnl <= 0
                    THEN (scores->>'technical_alignment')::float END) AS lose_tech
            FROM theses
            WHERE outcome_realized_pnl IS NOT NULL AND scores IS NOT NULL
            """
        )
        if row and row["win_info"] is not None:
            lines = ["**Score dimensions — winners vs losers (avg):**"]
            dims = [
                ("Information Edge", row["win_info"], row["lose_info"]),
                ("Catalyst Clarity", row["win_catalyst"], row["lose_catalyst"]),
                ("Volatility Pricing", row["win_vol"], row["lose_vol"]),
                ("Technical Alignment", row["win_tech"], row["lose_tech"]),
            ]
            for name, w, l in dims:
                if w is not None and l is not None:
                    gap = w - l
                    lines.append(f"  - {name}: winners {w:.1f} vs losers {l:.1f} (gap: {gap:+.1f})")
            sections.append("\n".join(lines))

        # 4. Common loss reasons
        rows = await self.pool.fetch(
            """
            SELECT outcome_close_reason, COUNT(*) AS cnt
            FROM theses
            WHERE outcome_realized_pnl IS NOT NULL
              AND outcome_close_reason IS NOT NULL
            GROUP BY 1
            ORDER BY 2 DESC
            LIMIT 5
            """
        )
        if rows:
            lines = ["**Common close reasons:**"]
            for r in rows:
                lines.append(f"  - {r['outcome_close_reason']}: {r['cnt']} trades")
            sections.append("\n".join(lines))

        # 5. Holding period for winners vs losers
        self._append_holding_period(sections, await self._holding_periods())

        if not sections:
            return ""

        header = "## System Feedback (Cross-Ticker Patterns)\n\nBased on all historical trades:"
        return header + "\n\n" + "\n\n".join(sections)

    async def build_risk_feedback(self) -> str:
        """Aggregate sizing and risk patterns for the risk checker prompt."""
        if not await self._has_enough_data():
            return ""

        sections = []

        # 1. P&L by position size bucket
        rows = await self.pool.fetch(
            """
            SELECT
                CASE
                    WHEN rv.position_size_pct <= 0.5 THEN '0-0.5%'
                    WHEN rv.position_size_pct <= 1.0 THEN '0.5-1%'
                    WHEN rv.position_size_pct <= 1.5 THEN '1-1.5%'
                    ELSE '1.5%+'
                END AS size_bucket,
                COUNT(*) AS total,
                AVG(t.outcome_realized_pnl) AS avg_pnl,
                COUNT(*) FILTER (WHERE t.outcome_realized_pnl > 0) AS wins
            FROM theses t
            JOIN trade_recommendations r ON r.thesis_id = t.id
            CROSS JOIN LATERAL (
                SELECT COALESCE(
                    (r.risk_verification->>'position_size_pct')::float, 0
                ) AS position_size_pct
            ) rv
            WHERE t.outcome_realized_pnl IS NOT NULL
              AND r.risk_verification->>'position_size_pct' IS NOT NULL
            GROUP BY 1
            ORDER BY 1
            """
        )
        if rows:
            lines = ["**P&L by position size:**"]
            for r in rows:
                total = r["total"]
                wins = r["wins"]
                avg = r["avg_pnl"]
                rate = wins / total * 100 if total else 0
                lines.append(
                    f"  - {r['size_bucket']}: {total} trades, "
                    f"{rate:.0f}% win rate, avg P&L ${avg:+,.0f}"
                )
            sections.append("\n".join(lines))

        # 2. Common loss reasons (same query, different framing)
        rows = await self.pool.fetch(
            """
            SELECT outcome_close_reason, COUNT(*) AS cnt
            FROM theses
            WHERE outcome_realized_pnl <= 0
              AND outcome_close_reason IS NOT NULL
            GROUP BY 1
            ORDER BY 2 DESC
            LIMIT 5
            """
        )
        if rows:
            lines = ["**Most common loss reasons:**"]
            for r in rows:
                lines.append(f"  - {r['outcome_close_reason']}: {r['cnt']} losing trades")
            sections.append("\n".join(lines))

        # 3. Holding period
        self._append_holding_period(sections, await self._holding_periods())

        if not sections:
            return ""

        header = "## Historical Risk Patterns\n\nBased on all closed positions:"
        return header + "\n\n" + "\n\n".join(sections)

    async def build_reviewer_feedback(self) -> str:
        """Aggregate review action patterns for the reviewer prompt."""
        if not await self._has_enough_data():
            return ""

        sections = []

        # 1. Recent review action distribution (last 30 days)
        rows = await self.pool.fetch(
            """
            SELECT
                recommended_action,
                COUNT(*) AS cnt
            FROM position_reviews
            WHERE created_at > NOW() - INTERVAL '30 days'
            GROUP BY 1
            ORDER BY 2 DESC
            """
        )
        if rows:
            total = sum(r["cnt"] for r in rows)
            lines = ["**Review action distribution (last 30 days):**"]
            for r in rows:
                pct = r["cnt"] / total * 100 if total else 0
                lines.append(f"  - {r['recommended_action']}: {r['cnt']} ({pct:.0f}%)")
            sections.append("\n".join(lines))

        # 2. Avg P&L by recommended action (for closed positions)
        rows = await self.pool.fetch(
            """
            SELECT
                pr.recommended_action,
                COUNT(DISTINCT p.id) AS positions,
                AVG(p.realized_pnl) AS avg_pnl
            FROM position_reviews pr
            JOIN options_positions p ON p.id = pr.position_id
            WHERE p.status = 'closed' AND p.realized_pnl IS NOT NULL
            GROUP BY 1
            ORDER BY 3 DESC
            """
        )
        if rows:
            lines = ["**Avg P&L by last recommended action (closed positions):**"]
            for r in rows:
                avg = r["avg_pnl"]
                if avg is not None:
                    lines.append(
                        f"  - {r['recommended_action']}: "
                        f"{r['positions']} positions, avg P&L ${avg:+,.0f}"
                    )
            sections.append("\n".join(lines))

        if not sections:
            return ""

        header = "## Review Patterns\n\nBased on recent position reviews:"
        return header + "\n\n" + "\n\n".join(sections)
