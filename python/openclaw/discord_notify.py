"""Discord notifications and human-in-the-loop approvals."""

from __future__ import annotations

import json
import logging
import os
from typing import Any

import httpx

logger = logging.getLogger(__name__)


class DiscordNotifier:
    """Send notifications and receive approvals via Discord webhook."""

    def __init__(
        self,
        webhook_url: str | None = None,
    ):
        self.webhook_url = webhook_url or os.environ.get("DISCORD_WEBHOOK_URL", "")

    async def send(self, message: str) -> None:
        """Send a message to Discord channel via webhook."""
        if not self.webhook_url:
            logger.warning("Discord not configured — message not sent: %s", message[:100])
            return

        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                await client.post(
                    self.webhook_url,
                    json={"content": message},
                )
        except Exception as e:
            logger.error("Failed to send Discord notification: %s", e)

    async def send_embed(self, title: str, description: str, fields: list[dict], color: int = 0x5865F2) -> None:
        """Send a rich embed to Discord."""
        if not self.webhook_url:
            logger.warning("Discord not configured — embed not sent")
            return

        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                await client.post(
                    self.webhook_url,
                    json={
                        "embeds": [
                            {
                                "title": title,
                                "description": description,
                                "color": color,
                                "fields": fields,
                            }
                        ]
                    },
                )
        except Exception as e:
            logger.error("Failed to send Discord embed: %s", e)

    async def send_recommendation(self, recommendation: dict) -> None:
        """Format and send a trade recommendation for human review."""
        ticker = recommendation.get("ticker", "?")
        direction = recommendation.get("direction", "?")
        contract = recommendation.get("contract", "?")
        size_usd = recommendation.get("position_size_usd", "?")
        score = recommendation.get("overall_score", "?")
        rec_id = recommendation.get("id", "")

        # Determine color based on direction
        color = 0x00FF00 if direction == "bullish" else 0xFF0000 if direction == "bearish" else 0xFFFF00

        fields = [
            {"name": "Ticker", "value": f"`{ticker}`", "inline": True},
            {"name": "Direction", "value": direction.capitalize(), "inline": True},
            {"name": "Score", "value": f"{score}/10", "inline": True},
            {"name": "Contract", "value": f"`{contract}`", "inline": False},
            {"name": "Position Size", "value": f"${size_usd}", "inline": True},
        ]

        await self.send_embed(
            title="🎯 New Trade Recommendation",
            description=f"Recommendation #{rec_id} is ready for review.",
            fields=fields,
            color=color,
        )

        # Send approval command hint
        await self.send(
            f"To approve: `!approve {rec_id}`\n"
            f"To reject: `!reject {rec_id}`\n"
            f"To view details: `!details {rec_id}`"
        )

    async def escalate(
        self,
        workflow_name: str,
        step_id: str,
        context: dict,
        error: str | None = None,
    ) -> None:
        """Escalate a workflow failure to human review."""
        fields = [
            {"name": "Workflow", "value": f"`{workflow_name}`", "inline": True},
            {"name": "Failed Step", "value": f"`{step_id}`", "inline": True},
            {"name": "Error", "value": error or "Validation gate failed", "inline": False},
        ]

        # Truncate context for Discord
        context_str = json.dumps(context, indent=2, default=str)[:500]
        fields.append({"name": "Context", "value": f"```json\n{context_str}\n```", "inline": False})

        await self.send_embed(
            title="⚠️ Workflow Escalation",
            description="A workflow step requires human intervention.",
            fields=fields,
            color=0xFFA500,  # Orange
        )

    async def send_battle_plan(self, plan: dict) -> None:
        """Send the weekly battle plan summary."""
        macro = plan.get("macro_view", "N/A")
        focus_tickers = plan.get("focus_tickers", [])
        top_ideas = plan.get("top_ideas", [])

        fields = [
            {"name": "Macro View", "value": macro, "inline": False},
            {"name": "Focus Tickers", "value": ", ".join(f"`{t}`" for t in focus_tickers), "inline": False},
            {"name": "Top Ideas", "value": f"{len(top_ideas)} trade ideas identified", "inline": False},
        ]

        await self.send_embed(
            title="📊 Weekly Battle Plan",
            description="Analysis complete for the week ahead.",
            fields=fields,
            color=0x0099FF,  # Blue
        )

    async def send_position_update(self, position: dict, action: str) -> None:
        """Send a position management update (entry, exit, adjustment)."""
        ticker = position.get("ticker", "?")
        contract = position.get("contract", "?")
        pnl = position.get("pnl", 0)
        pnl_pct = position.get("pnl_pct", 0)

        # Color based on action and P&L
        if action == "ENTRY":
            color = 0x0099FF  # Blue
            title = "📥 Position Entry"
        elif action == "EXIT":
            color = 0x00FF00 if pnl > 0 else 0xFF0000  # Green/Red
            title = "📤 Position Exit"
        else:
            color = 0xFFFF00  # Yellow
            title = "🔄 Position Adjustment"

        fields = [
            {"name": "Ticker", "value": f"`{ticker}`", "inline": True},
            {"name": "Contract", "value": f"`{contract}`", "inline": True},
            {"name": "Action", "value": action, "inline": True},
        ]

        if action == "EXIT":
            fields.append({"name": "P&L", "value": f"${pnl:.2f} ({pnl_pct:+.1f}%)", "inline": False})

        await self.send_embed(
            title=title,
            description=f"Position update for {ticker}",
            fields=fields,
            color=color,
        )
