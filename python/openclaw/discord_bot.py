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

        # Discord client — need message_content intent for ! commands
        intents = discord.Intents.default()
        intents.message_content = True
        intents.members = True  # Required for guild.me to resolve
        self.client = discord.Client(intents=intents)

        # Track if bot is ready
        self._ready = asyncio.Event()
        self._channel = None

        # Dependencies wired in later via set_context()
        self._db = None
        self._engine = None
        self._position_manager = None

        # Register event handlers
        @self.client.event
        async def on_ready():
            logger.info(f"Discord bot connected as {self.client.user}")
            self._ready.set()

        @self.client.event
        async def on_message(message):
            if message.author == self.client.user:
                return
            if not message.content.startswith("!"):
                return

            parts = message.content.strip().split()
            cmd = parts[0].lower()
            args = parts[1:]

            handlers = {
                "!status": self._cmd_status,
                "!watchlist": self._cmd_watchlist,
                "!analyze": self._cmd_analyze,
                "!portfolio": self._cmd_portfolio,
                "!tick": self._cmd_tick,
                "!help": self._cmd_help,
            }
            handler = handlers.get(cmd)
            if handler:
                try:
                    await handler(message.channel, args)
                except Exception as e:
                    logger.exception("Command %s failed", cmd)
                    await message.channel.send(f"Error running `{cmd}`: {e}")

    def set_context(self, db: Any = None, engine: Any = None, position_manager: Any = None) -> None:
        """Wire in database, workflow engine, and position manager after init."""
        if db is not None:
            self._db = db
        if engine is not None:
            self._engine = engine
        if position_manager is not None:
            self._position_manager = position_manager

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
            # Fallback to fetch if cache miss
            if self._channel is None:
                try:
                    self._channel = await self.client.fetch_channel(self.channel_id)
                except Exception:
                    logger.warning("Could not fetch channel ID %s", self.channel_id)

        if not self._channel:
            # Auto-detect: use first text channel bot can access.
            # Try cached guilds first, then fetch from API as fallback.
            guilds = self.client.guilds
            if not guilds:
                try:
                    guilds = [g async for g in self.client.fetch_guilds()]
                    logger.info("Fetched %d guild(s) from API", len(guilds))
                except Exception:
                    logger.exception("Failed to fetch guilds from API")
                    guilds = []

            for guild in guilds:
                # Ensure we have the full guild object with channels
                try:
                    full_guild = self.client.get_guild(guild.id)
                    if full_guild is None:
                        full_guild = await self.client.fetch_guild(guild.id)
                    channels = full_guild.text_channels
                except Exception:
                    logger.warning("Could not fetch guild %s, skipping", guild.id)
                    continue

                for channel in channels:
                    # Try permission check, but fall back to just picking
                    # the first text channel if guild.me is unavailable
                    me = full_guild.me
                    if me is not None:
                        perms = channel.permissions_for(me)
                        if not perms.send_messages:
                            continue
                    self._channel = channel
                    logger.info(
                        "Auto-detected channel: %s (ID: %s) in guild: %s",
                        channel.name, channel.id, full_guild.name,
                    )
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

    # --- Chat commands ---

    async def _cmd_help(self, channel, args: list[str]) -> None:
        embed = discord.Embed(
            title="OpenClaw Commands",
            description="Available chat commands:",
            color=discord.Color.blue(),
        )
        for name, desc in [
            ("`!status`", "Open positions, pending recommendations, next jobs"),
            ("`!watchlist`", "Current watchlist tickers"),
            ("`!analyze <TICKER>`", "Run on-demand trade thesis (~2 min)"),
            ("`!portfolio`", "Account summary, exposure, recent P&L"),
            ("`!tick`", "Force a position manager tick now"),
            ("`!help`", "Show this message"),
        ]:
            embed.add_field(name=name, value=desc, inline=False)
        await channel.send(embed=embed)

    async def _cmd_status(self, channel, args: list[str]) -> None:
        if not self._db:
            await channel.send("Database not connected.")
            return

        positions = await self._db.get_open_positions()
        pending = await self._db.get_pending_recommendations()
        approved = await self._db.get_approved_recommendations()

        embed = discord.Embed(title="System Status", color=discord.Color.blue())

        if positions:
            lines = []
            for p in positions[:10]:
                pnl = p.get("unrealized_pnl", 0)
                pnl_str = f"+${pnl}" if pnl >= 0 else f"-${abs(pnl)}"
                lines.append(
                    f"`{p['ticker']}` {p.get('right', '?')}"
                    f" ${p.get('strike', '?')} {p.get('expiry', '?')}"
                    f" — {pnl_str}"
                )
            embed.add_field(
                name=f"Open Positions ({len(positions)})",
                value="\n".join(lines),
                inline=False,
            )
        else:
            embed.add_field(name="Open Positions", value="None", inline=False)

        if pending:
            lines = [
                f"#{r.get('id', '?')} `{r.get('ticker', '?')}`"
                f" {r.get('right', '?')} ${r.get('strike', '?')}"
                for r in pending[:5]
            ]
            embed.add_field(
                name=f"Pending Review ({len(pending)})",
                value="\n".join(lines),
                inline=False,
            )
        else:
            embed.add_field(name="Pending Review", value="None", inline=False)

        if approved:
            lines = [
                f"#{r.get('id', '?')} `{r.get('ticker', '?')}`"
                for r in approved[:5]
            ]
            embed.add_field(
                name=f"Approved / Awaiting Exec ({len(approved)})",
                value="\n".join(lines),
                inline=False,
            )

        embed.add_field(
            name="Scheduled Jobs",
            value=(
                "Pre-market research: **8:00 AM ET** Mon-Fri\n"
                "Midday position check: **12:30 PM ET** Mon-Fri\n"
                "Post-market check: **4:30 PM ET** Mon-Fri\n"
                "Weekly deep dive: **10:00 AM ET** Saturday"
            ),
            inline=False,
        )

        await channel.send(embed=embed)

    async def _cmd_watchlist(self, channel, args: list[str]) -> None:
        if not self._db:
            await channel.send("Database not connected.")
            return

        watchlist = await self._db.get_watchlist()
        embed = discord.Embed(title="Watchlist", color=discord.Color.blue())

        if watchlist:
            lines = []
            for w in watchlist:
                notes = w.get("notes") or ""
                sector = w.get("sector", "")
                line = f"`{w['ticker']}`"
                if sector:
                    line += f" — {sector}"
                if notes:
                    line += f" ({notes})"
                lines.append(line)
            embed.description = "\n".join(lines)
        else:
            embed.description = "Watchlist is empty."

        await channel.send(embed=embed)

    async def _cmd_analyze(self, channel, args: list[str]) -> None:
        if not self._db or not self._engine:
            await channel.send("Engine not connected. Cannot run analysis.")
            return
        if not args:
            await channel.send("Usage: `!analyze <TICKER>`")
            return

        ticker = args[0].upper()
        await channel.send(f"Analyzing **{ticker}**... this may take ~2 minutes.")
        asyncio.create_task(self._run_analyze(channel, ticker))

    async def _run_analyze(self, channel, ticker: str) -> None:
        try:
            from openclaw.workflows import get_workflow
            from schemas.research import ResearchRequest

            workflow = get_workflow("trade-thesis")
            result = await self._engine.run(workflow, ResearchRequest(ticker=ticker))

            if result is None:
                await channel.send(f"Analysis for **{ticker}** — no actionable opportunity found.")
                return

            thesis = result.step_outputs.get("evaluate")
            if thesis is None:
                await channel.send(f"Analysis for **{ticker}** — did not pass evaluation gate.")
                return

            embed = discord.Embed(
                title=f"Analysis: {ticker}",
                color=(
                    discord.Color.green() if getattr(thesis, "direction", "") == "bullish"
                    else discord.Color.red() if getattr(thesis, "direction", "") == "bearish"
                    else discord.Color.gold()
                ),
            )
            embed.add_field(name="Direction", value=getattr(thesis, "direction", "N/A").capitalize(), inline=True)
            scores = getattr(thesis, "scores", None)
            if scores:
                embed.add_field(name="Score", value=f"{getattr(scores, 'overall', '?')}/10", inline=True)
            contract = getattr(thesis, "recommended_contract", None)
            if contract:
                embed.add_field(name="Contract", value=str(contract), inline=False)
            thesis_text = getattr(thesis, "thesis_text", "")
            if thesis_text:
                embed.add_field(name="Thesis", value=thesis_text[:1024], inline=False)
            await channel.send(embed=embed)

        except Exception as e:
            logger.exception("!analyze failed for %s", ticker)
            await channel.send(f"Analysis for **{ticker}** failed: {e}")

    async def _cmd_portfolio(self, channel, args: list[str]) -> None:
        if not self._db:
            await channel.send("Database not connected.")
            return

        positions = await self._db.get_open_positions()
        exposure = await self._db.get_total_options_exposure()

        embed = discord.Embed(title="Portfolio Summary", color=discord.Color.blue())
        embed.add_field(name="Total Exposure", value=f"${exposure:,.2f}", inline=True)
        embed.add_field(name="Open Positions", value=str(len(positions)), inline=True)

        total_pnl = sum(float(p.get("unrealized_pnl", 0) or 0) for p in positions)
        pnl_prefix = "+" if total_pnl >= 0 else ""
        embed.add_field(name="Unrealized P&L", value=f"{pnl_prefix}${total_pnl:,.2f}", inline=True)

        if positions:
            lines = []
            for p in positions[:15]:
                pnl = float(p.get("unrealized_pnl", 0) or 0)
                pnl_str = f"+${pnl:,.2f}" if pnl >= 0 else f"-${abs(pnl):,.2f}"
                cost = float(p.get("cost_basis", 0) or 0)
                lines.append(
                    f"`{p['ticker']}` {p.get('right', '?')}"
                    f" ${p.get('strike', '?')} {p.get('expiry', '?')}"
                    f" | cost ${cost:,.0f} | P&L {pnl_str}"
                )
            embed.add_field(name="Positions", value="\n".join(lines), inline=False)

        await channel.send(embed=embed)

    async def _cmd_tick(self, channel, args: list[str]) -> None:
        if not self._position_manager:
            await channel.send("Position manager not connected.")
            return

        await channel.send("Running position manager tick...")
        try:
            await self._position_manager._tick()
            await channel.send("Position tick complete.")
        except Exception as e:
            logger.exception("!tick failed")
            await channel.send(f"Position tick failed: {e}")

    async def close(self):
        """Shutdown the Discord bot."""
        if self.client:
            await self.client.close()
