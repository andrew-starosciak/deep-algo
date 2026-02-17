"""Interactive Discord bot with approve/reject/details buttons."""

from __future__ import annotations

import asyncio
import json
import logging
import os
from typing import Any

import discord
from discord import ButtonStyle, Interaction
from discord.ui import Button, View

logger = logging.getLogger(__name__)


class ApprovalView(View):
    """Interactive buttons for trade recommendation approval."""

    def __init__(self, rec_id: int, db_url: str):
        super().__init__(timeout=None)  # Buttons never expire
        self.rec_id = rec_id
        self.db_url = db_url

    @discord.ui.button(label="Approve", style=ButtonStyle.success, emoji="✅")
    async def approve_button(self, interaction: Interaction, button: Button):
        """Handle approve button click."""
        await interaction.response.defer()

        try:
            # Update database
            from db.repositories import Database

            db = await Database.connect(self.db_url)
            try:
                await db.approve_recommendation(self.rec_id)
            finally:
                await db.close()

            # Update message
            embed = interaction.message.embeds[0]
            embed.color = discord.Color.green()
            embed.add_field(
                name="Status",
                value=f"✅ Approved by {interaction.user.mention} at <t:{int(interaction.created_at.timestamp())}:t>",
                inline=False,
            )

            # Disable all buttons
            for child in self.children:
                child.disabled = True

            await interaction.message.edit(embed=embed, view=self)
            await interaction.followup.send(
                f"✅ Recommendation #{self.rec_id} approved! "
                f"Position manager will execute on next poll cycle.",
                ephemeral=True,
            )

        except Exception as e:
            logger.exception("Failed to approve recommendation")
            await interaction.followup.send(
                f"❌ Failed to approve: {e}", ephemeral=True
            )

    @discord.ui.button(label="Reject", style=ButtonStyle.danger, emoji="❌")
    async def reject_button(self, interaction: Interaction, button: Button):
        """Handle reject button click."""
        await interaction.response.defer()

        try:
            # Update database
            from db.repositories import Database

            db = await Database.connect(self.db_url)
            try:
                await db.reject_recommendation(self.rec_id)
            finally:
                await db.close()

            # Update message
            embed = interaction.message.embeds[0]
            embed.color = discord.Color.red()
            embed.add_field(
                name="Status",
                value=f"❌ Rejected by {interaction.user.mention} at <t:{int(interaction.created_at.timestamp())}:t>",
                inline=False,
            )

            # Disable all buttons
            for child in self.children:
                child.disabled = True

            await interaction.message.edit(embed=embed, view=self)
            await interaction.followup.send(
                f"❌ Recommendation #{self.rec_id} rejected.", ephemeral=True
            )

        except Exception as e:
            logger.exception("Failed to reject recommendation")
            await interaction.followup.send(
                f"❌ Failed to reject: {e}", ephemeral=True
            )

    @discord.ui.button(label="Details", style=ButtonStyle.primary, emoji="📋")
    async def details_button(self, interaction: Interaction, button: Button):
        """Handle details button click."""
        await interaction.response.defer(ephemeral=True)

        try:
            # Fetch full recommendation from database
            from db.repositories import Database

            db = await Database.connect(self.db_url)
            try:
                rec = await db.get_recommendation(self.rec_id)
            finally:
                await db.close()

            if not rec:
                await interaction.followup.send(
                    f"❌ Recommendation #{self.rec_id} not found.", ephemeral=True
                )
                return

            # Build detailed embed
            details_embed = discord.Embed(
                title=f"📋 Recommendation #{self.rec_id} — Full Details",
                color=discord.Color.blue(),
            )

            # Thesis details
            thesis = rec.get("thesis", {})
            details_embed.add_field(
                name="Thesis",
                value=thesis.get("description", "N/A")[:1024],
                inline=False,
            )

            # Supporting evidence
            evidence = thesis.get("supporting_evidence", [])
            if evidence:
                details_embed.add_field(
                    name="Supporting Evidence",
                    value="\n".join(f"• {e}" for e in evidence[:5])[:1024],
                    inline=False,
                )

            # Risks
            risks = thesis.get("risks", [])
            if risks:
                details_embed.add_field(
                    name="Risks",
                    value="\n".join(f"• {r}" for r in risks[:5])[:1024],
                    inline=False,
                )

            # Risk verification
            risk_check = rec.get("risk_verification", {})
            details_embed.add_field(
                name="Risk Management",
                value=(
                    f"Position Size: {risk_check.get('position_size_pct', '?')}% of account\n"
                    f"Max Loss: {risk_check.get('max_loss_usd', '?')}\n"
                    f"Approved: {risk_check.get('approved', False)}"
                ),
                inline=False,
            )

            # Exit plan
            exit_targets = rec.get("exit_targets", [])
            stop_loss = rec.get("stop_loss", "N/A")
            details_embed.add_field(
                name="Exit Plan",
                value=(
                    f"Targets: {', '.join(exit_targets)}\n"
                    f"Stop Loss: {stop_loss}"
                ),
                inline=False,
            )

            await interaction.followup.send(embed=details_embed, ephemeral=True)

        except Exception as e:
            logger.exception("Failed to fetch recommendation details")
            await interaction.followup.send(
                f"❌ Failed to load details: {e}", ephemeral=True
            )


class DiscordBot:
    """Full Discord bot with interactive buttons."""

    def __init__(
        self,
        bot_token: str | None = None,
        channel_id: int | None = None,
        db_url: str | None = None,
    ):
        self.bot_token = bot_token or os.environ.get("DISCORD_BOT_TOKEN", "")
        channel_id_str = channel_id or os.environ.get("DISCORD_CHANNEL_ID", "")
        self.db_url = db_url or os.environ.get("DATABASE_URL", "")

        # Parse channel ID (skip if empty or comment)
        self.channel_id = None
        if channel_id_str and not channel_id_str.strip().startswith("#"):
            try:
                self.channel_id = int(channel_id_str.strip())
            except ValueError:
                logger.warning(f"Invalid DISCORD_CHANNEL_ID: {channel_id_str}")

        # Discord client with minimal intents
        intents = discord.Intents.default()
        intents.message_content = True
        self.client = discord.Client(intents=intents)

        # Track if bot is ready
        self._ready = asyncio.Event()
        self._channel = None

        # Register event handlers
        @self.client.event
        async def on_ready():
            logger.info(f"Discord bot connected as {self.client.user}")
            self._ready.set()

    async def start_background(self):
        """Start the Discord bot in the background."""
        if not self.bot_token:
            logger.warning("DISCORD_BOT_TOKEN not set — bot will not start")
            return

        # Start bot in background task
        asyncio.create_task(self.client.start(self.bot_token))

        # Wait for ready
        await asyncio.wait_for(self._ready.wait(), timeout=30.0)

    async def get_channel(self) -> discord.TextChannel:
        """Get the target channel (auto-detect if not specified)."""
        if self._channel:
            return self._channel

        if self.channel_id:
            self._channel = self.client.get_channel(self.channel_id)
        else:
            # Auto-detect: use first text channel bot can see
            for guild in self.client.guilds:
                for channel in guild.text_channels:
                    if channel.permissions_for(guild.me).send_messages:
                        self._channel = channel
                        logger.info(f"Auto-detected channel: {channel.name} (ID: {channel.id})")
                        break
                if self._channel:
                    break

        if not self._channel:
            raise ValueError("No Discord channel available")

        return self._channel

    async def send(self, message: str) -> None:
        """Send a plain text message."""
        if not self.bot_token:
            logger.warning("Discord bot not configured — message not sent")
            return

        try:
            channel = await self.get_channel()
            await channel.send(message)
        except Exception as e:
            logger.error("Failed to send Discord message: %s", e)

    async def send_embed(
        self,
        title: str,
        description: str,
        fields: list[dict],
        color: int = 0x5865F2,
        view: View | None = None,
    ) -> None:
        """Send a rich embed."""
        if not self.bot_token:
            logger.warning("Discord bot not configured — embed not sent")
            return

        try:
            channel = await self.get_channel()
            embed = discord.Embed(title=title, description=description, color=color)

            for field in fields:
                embed.add_field(
                    name=field["name"],
                    value=field["value"],
                    inline=field.get("inline", False),
                )

            await channel.send(embed=embed, view=view)
        except Exception as e:
            logger.error("Failed to send Discord embed: %s", e)

    async def send_recommendation(self, recommendation: dict) -> None:
        """Send trade recommendation with interactive buttons."""
        ticker = recommendation.get("ticker", "?")
        direction = recommendation.get("direction", "?")
        contract = recommendation.get("contract", "?")
        size_usd = recommendation.get("position_size_usd", "?")
        score = recommendation.get("overall_score", "?")
        rec_id = recommendation.get("id", 0)

        # Determine color based on direction
        color = (
            discord.Color.green()
            if direction == "bullish"
            else discord.Color.red()
            if direction == "bearish"
            else discord.Color.gold()
        )

        fields = [
            {"name": "Ticker", "value": f"`{ticker}`", "inline": True},
            {"name": "Direction", "value": direction.capitalize(), "inline": True},
            {"name": "Score", "value": f"{score}/10", "inline": True},
            {"name": "Contract", "value": f"`{contract}`", "inline": False},
            {"name": "Position Size", "value": f"${size_usd}", "inline": True},
        ]

        # Create interactive buttons
        view = ApprovalView(rec_id=rec_id, db_url=self.db_url)

        await self.send_embed(
            title="🎯 New Trade Recommendation",
            description=f"Recommendation #{rec_id} is ready for review.",
            fields=fields,
            color=color.value,
            view=view,
        )

    async def escalate(
        self,
        workflow_name: str,
        step_id: str,
        context: dict,
        error: str | None = None,
    ) -> None:
        """Escalate a workflow failure."""
        fields = [
            {"name": "Workflow", "value": f"`{workflow_name}`", "inline": True},
            {"name": "Failed Step", "value": f"`{step_id}`", "inline": True},
            {"name": "Error", "value": error or "Validation gate failed", "inline": False},
        ]

        context_str = json.dumps(context, indent=2, default=str)[:500]
        fields.append({"name": "Context", "value": f"```json\n{context_str}\n```", "inline": False})

        await self.send_embed(
            title="⚠️ Workflow Escalation",
            description="A workflow step requires human intervention.",
            fields=fields,
            color=discord.Color.orange().value,
        )

    async def send_battle_plan(self, plan: dict) -> None:
        """Send weekly battle plan."""
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
            color=discord.Color.blue().value,
        )

    async def send_position_update(self, position: dict, action: str) -> None:
        """Send position management update."""
        ticker = position.get("ticker", "?")
        contract = position.get("contract", "?")
        pnl = position.get("pnl", 0)
        pnl_pct = position.get("pnl_pct", 0)

        if action == "ENTRY":
            color = discord.Color.blue()
            title = "📥 Position Entry"
        elif action == "EXIT":
            color = discord.Color.green() if pnl > 0 else discord.Color.red()
            title = "📤 Position Exit"
        else:
            color = discord.Color.gold()
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
            color=color.value,
        )

    async def close(self):
        """Shutdown the Discord bot."""
        if self.client:
            await self.client.close()
