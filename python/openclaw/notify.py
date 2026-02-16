"""Telegram notifications and human-in-the-loop approvals."""

from __future__ import annotations

import asyncio
import json
import logging
import os
from typing import Any

logger = logging.getLogger(__name__)


class TelegramNotifier:
    """Send notifications and receive approvals via Telegram."""

    def __init__(
        self,
        bot_token: str | None = None,
        chat_id: str | None = None,
    ):
        self.bot_token = bot_token or os.environ.get("TELEGRAM_BOT_TOKEN", "")
        self.chat_id = chat_id or os.environ.get("TELEGRAM_CHAT_ID", "")
        self._bot = None

    async def _get_bot(self):
        if self._bot is None:
            from telegram import Bot

            self._bot = Bot(token=self.bot_token)
        return self._bot

    async def send(self, message: str) -> None:
        """Send a message to the configured chat."""
        if not self.bot_token or not self.chat_id:
            logger.warning("Telegram not configured — message not sent: %s", message[:100])
            return

        bot = await self._get_bot()
        await bot.send_message(chat_id=self.chat_id, text=message, parse_mode="Markdown")

    async def send_recommendation(self, recommendation: dict) -> None:
        """Format and send a trade recommendation for human review."""
        msg = (
            f"*New Trade Recommendation*\n\n"
            f"Ticker: `{recommendation.get('ticker', '?')}`\n"
            f"Direction: {recommendation.get('direction', '?')}\n"
            f"Contract: `{recommendation.get('contract', '?')}`\n"
            f"Size: ${recommendation.get('position_size_usd', '?')}\n"
            f"Score: {recommendation.get('overall_score', '?')}/10\n\n"
            f"Reply `/approve {recommendation.get('id', '')}` or `/reject {recommendation.get('id', '')}`"
        )
        await self.send(msg)

    async def escalate(
        self,
        workflow_name: str,
        step_id: str,
        context: dict,
        error: str | None = None,
    ) -> None:
        """Escalate a workflow failure to human review."""
        msg = (
            f"*Workflow Escalation*\n\n"
            f"Workflow: `{workflow_name}`\n"
            f"Failed step: `{step_id}`\n"
            f"Error: {error or 'Validation gate failed'}\n\n"
            f"Context: ```{json.dumps(context, indent=2, default=str)[:500]}```"
        )
        await self.send(msg)

    async def send_battle_plan(self, plan: dict) -> None:
        """Send the weekly battle plan summary."""
        msg = (
            f"*Weekly Battle Plan*\n\n"
            f"Macro: {plan.get('macro_view', 'N/A')}\n\n"
            f"Focus tickers: {', '.join(plan.get('focus_tickers', []))}\n\n"
            f"Top ideas: {len(plan.get('top_ideas', []))}"
        )
        await self.send(msg)
