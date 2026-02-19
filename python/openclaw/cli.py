"""CLI entrypoint for OpenClaw."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys

logger = logging.getLogger(__name__)


def main():
    parser = argparse.ArgumentParser(
        prog="openclaw",
        description="LLM-driven options trading orchestrator",
    )
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="Enable debug logging"
    )
    parser.add_argument(
        "--db-url", help="Postgres connection URL (omit for in-memory mode)"
    )

    subparsers = parser.add_subparsers(dest="command")

    # run <workflow> [--ticker TICKER]
    run_parser = subparsers.add_parser("run", help="Run a workflow")
    run_parser.add_argument("workflow", help="Workflow name (e.g., trade-thesis)")
    run_parser.add_argument("--ticker", help="Ticker symbol (for trade-thesis workflow)")
    run_parser.add_argument(
        "--model", default="claude-sonnet-4-5-20250929",
        help="Claude model to use",
    )
    run_parser.add_argument(
        "--equity", type=float, default=None,
        help="Account equity for position sizing (queries IB if omitted)",
    )

    # research <ticker> — quick research without full workflow
    research_parser = subparsers.add_parser("research", help="Run research pipeline only")
    research_parser.add_argument("ticker", help="Ticker symbol")

    # approve <rec_id> — approve a pending recommendation
    approve_parser = subparsers.add_parser("approve", help="Approve a trade recommendation")
    approve_parser.add_argument("rec_id", type=int, help="Recommendation ID to approve")

    # position-manager — run the position management service loop
    pm_parser = subparsers.add_parser(
        "position-manager", help="Run the position manager service"
    )
    pm_parser.add_argument(
        "--mode", choices=["sim", "paper", "live"], default="sim",
        help="sim=local shim, paper=IB paper(4002), live=IB live(4001) (default: sim)",
    )
    pm_parser.add_argument(
        "--host", default="127.0.0.1",
        help="IB Gateway host (default: 127.0.0.1)",
    )
    pm_parser.add_argument(
        "--port", type=int, default=None,
        help="IB Gateway port (default: auto by mode — paper=4002, live=4001)",
    )
    pm_parser.add_argument(
        "--poll-interval", type=int, default=30,
        help="Seconds between poll cycles (default: 30)",
    )

    # status
    subparsers.add_parser("status", help="Show running/recent workflows")

    # watchlist
    wl_parser = subparsers.add_parser("watchlist", help="Manage watchlist")
    wl_sub = wl_parser.add_subparsers(dest="wl_command")
    wl_add = wl_sub.add_parser("add", help="Add ticker to watchlist")
    wl_add.add_argument("ticker")
    wl_add.add_argument("--sector", required=True)
    wl_sub.add_parser("show", help="Show current watchlist")

    # scheduler
    sched_parser = subparsers.add_parser("scheduler", help="Start the cron scheduler daemon")
    sched_parser.add_argument(
        "--mode", choices=["sim", "paper", "live"], default="sim",
        help="sim=local shim, paper=IB paper(4002), live=IB live(4001) (default: sim)",
    )
    sched_parser.add_argument(
        "--host", default="127.0.0.1",
        help="IB Gateway host (default: 127.0.0.1)",
    )
    sched_parser.add_argument(
        "--port", type=int, default=None,
        help="IB Gateway port (default: auto by mode — paper=4002, live=4001)",
    )
    sched_parser.add_argument(
        "--model", default="claude-sonnet-4-5-20250929",
        help="Claude model to use for workflows",
    )
    sched_parser.add_argument(
        "--auto-approve", action="store_true",
        help="Auto-approve recommendations (skip human review gate)",
    )

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    )
    # Quiet noisy libraries
    logging.getLogger("httpx").setLevel(logging.WARNING)
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    logging.getLogger("yfinance").setLevel(logging.WARNING)

    if args.command is None:
        parser.print_help()
        sys.exit(1)

    asyncio.run(_dispatch(args))


async def _init_engine(args, ib_client=None):
    """Initialize the workflow engine with agents and DB."""
    from agents.analyst import AnalystAgent
    from agents.researcher import ResearcherAgent
    from agents.reviewer import ReviewerAgent
    from agents.risk_checker import RiskCheckerAgent
    from openclaw.engine import WorkflowEngine
    from openclaw.llm import LLMClient
    from openclaw.notify import MultiNotifier

    # DB: Postgres if URL provided, else in-memory
    db_url = getattr(args, "db_url", None)
    if db_url:
        from db.repositories import Database
        db = await Database.connect(db_url)
    else:
        from db.memory import MemoryDatabase
        db = MemoryDatabase()
        logger.info("Using in-memory database (no --db-url provided)")

    model = getattr(args, "model", "claude-sonnet-4-5-20250929")
    llm = LLMClient(model=model)
    notifier = MultiNotifier()  # Auto-detects Discord/Telegram from env vars

    engine = WorkflowEngine(db=db, llm=llm, notifier=notifier)

    # Register all agents
    engine.register_agent("researcher", ResearcherAgent(llm=llm, db=db))
    engine.register_agent("analyst", AnalystAgent(llm=llm, db=db))
    engine.register_agent("risk_checker", RiskCheckerAgent(llm=llm, db=db, ib_client=ib_client))
    engine.register_agent("reviewer", ReviewerAgent(llm=llm, db=db))

    return engine


async def _init_db(args):
    """Initialize just the DB connection (no engine needed)."""
    db_url = getattr(args, "db_url", None)
    if not db_url:
        print("Error: --db-url required for this command")
        sys.exit(1)
    from db.repositories import Database
    return await Database.connect(db_url)


async def _dispatch(args):
    """Route CLI commands to handlers."""
    if args.command == "run":
        await _cmd_run(args)
    elif args.command == "research":
        await _cmd_research(args)
    elif args.command == "approve":
        await _cmd_approve(args)
    elif args.command == "position-manager":
        await _cmd_position_manager(args)
    elif args.command == "status":
        print("Status: not yet connected to database")
    elif args.command == "watchlist":
        await _cmd_watchlist(args)
    elif args.command == "scheduler":
        await _cmd_scheduler(args)


async def _cmd_run(args):
    """Run a named workflow."""
    from openclaw.workflows import get_workflow
    from schemas.research import ResearchRequest

    workflow = get_workflow(args.workflow)
    engine = await _init_engine(args)

    # Build initial input based on workflow type
    if args.workflow == "trade-thesis":
        if not args.ticker:
            print("Error: --ticker required for trade-thesis workflow")
            sys.exit(1)
        initial_input = ResearchRequest(ticker=args.ticker.upper())
    else:
        print(f"Error: No input builder for workflow '{args.workflow}'")
        sys.exit(1)

    print(f"\n{'='*60}")
    print(f"  Workflow: {workflow.name}")
    print(f"  Input:    {initial_input.model_dump()}")
    print(f"  Steps:    {' → '.join(s.id for s in workflow.steps)}")
    print(f"{'='*60}\n")

    result = await engine.run(workflow, initial_input)

    # Persist research summary (valuable even if later gates fail)
    try:
        research_output = result.step_outputs.get("research") if result else None
        if research_output is not None:
            await engine.db.save_research_summary(
                run_id=result.run_id, ticker=args.ticker.upper(), mode="cli",
                summary=research_output.model_dump(),
                opportunity_score=research_output.opportunity_score,
            )
    except Exception:
        logger.warning("Failed to save research summary", exc_info=True)

    if result is None:
        print("\nWorkflow did not produce a final result (aborted or escalated).")
        return

    print(f"\n{'='*60}")
    print("  RESULT")
    print(f"{'='*60}")
    print(json.dumps(result.final_output.model_dump(), indent=2, default=str))

    # If workflow produced an approved risk verification with a recommended contract,
    # save a TradeRecommendation to DB for manual approval.
    await _maybe_save_recommendation(engine, result, args)


async def _get_equity(args):
    """Resolve account equity: CLI flag > IB paper query > default."""
    from decimal import Decimal

    equity_flag = getattr(args, "equity", None)
    if equity_flag is not None:
        return Decimal(str(equity_flag))

    # Default — position manager will re-check with real IB data before executing
    logger.info("No --equity specified, using default $200,000 for recommendation sizing")
    return Decimal("200000")


async def _maybe_save_recommendation(engine, result, args):
    """Save a TradeRecommendation if the workflow produced one."""
    from decimal import Decimal

    from schemas.risk import RiskVerification
    from schemas.thesis import Thesis

    # Need both the thesis (with contract) and the risk verification (approved)
    thesis = result.step_outputs.get("evaluate")
    verification = result.step_outputs.get("verify")

    if not isinstance(thesis, Thesis) or not isinstance(verification, RiskVerification):
        return
    if not verification.approved:
        return
    if thesis.recommended_contract is None:
        print(
            "\nNo contract selected (CLI mode has no IB connection for contract selection).\n"
            "Use the scheduler or Discord bot with IB connected for automatic contract selection."
        )
        return

    # Check if we have a real DB (not in-memory)
    if not hasattr(engine.db, "save_thesis"):
        return

    try:
        equity = await _get_equity(args)
        position_size_usd = verification.position_size_pct * equity / Decimal("100")

        # Save thesis first
        thesis_id = await engine.db.save_thesis(result.run_id, thesis.model_dump())

        # Build and save recommendation
        rec_data = {
            "contract": thesis.recommended_contract.model_dump(),
            "position_size_pct": str(verification.position_size_pct),
            "position_size_usd": str(position_size_usd),
            "exit_targets": ["+50% sell half", "+100% close"],
            "stop_loss": "-50% hard stop",
            "max_hold_days": 30,
            "risk_verification": verification.model_dump(),
        }
        rec_id = await engine.db.save_recommendation(thesis_id, result.run_id, rec_data)

        print(f"\nRecommendation #{rec_id} saved (status: pending_review).")
        pct = verification.position_size_pct
        print(f"  Equity: ${equity:,.0f} | Size: {pct}% = ${position_size_usd:,.0f}")
        print(f"  Run 'openclaw approve {rec_id} --db-url ...' to approve for execution.")
    except Exception:
        logger.exception("Failed to save recommendation")


async def _cmd_research(args):
    """Run just the research pipeline for a ticker (no LLM, no workflow)."""
    from research.pipeline import ResearchPipeline

    pipeline = ResearchPipeline()
    ticker = args.ticker.upper()

    print(f"Gathering research data for {ticker}...\n")
    raw = await pipeline.gather(ticker)
    print(raw)


async def _cmd_approve(args):
    """Approve a pending trade recommendation."""
    db = await _init_db(args)

    try:
        await db.approve_recommendation(args.rec_id)
        print(f"Recommendation #{args.rec_id} approved.")
        print("The position manager will pick it up on the next poll cycle.")
    finally:
        await db.close()


async def _cmd_watchlist(args):
    """Manage the options trading watchlist."""
    db = await _init_db(args)

    try:
        if args.wl_command == "add":
            await db.add_to_watchlist(
                ticker=args.ticker.upper(),
                sector=args.sector,
                notes=None,
            )
            print(f"Added {args.ticker.upper()} ({args.sector}) to watchlist")

        elif args.wl_command == "show":
            watchlist = await db.get_watchlist()
            if not watchlist:
                print("Watchlist is empty")
            else:
                print(f"\nWatchlist ({len(watchlist)} tickers):")
                print("-" * 60)
                for item in watchlist:
                    ticker = item["ticker"]
                    sector = item.get("sector", "Unknown")
                    notes = item.get("notes", "")
                    note_str = f" — {notes}" if notes else ""
                    print(f"  {ticker:6s} {sector:20s}{note_str}")
                print()

        else:
            print("Usage: openclaw watchlist {add|show}")

    finally:
        await db.close()


def _resolve_port(args) -> int:
    """Resolve the IB Gateway port from explicit flag or mode default."""
    if args.port is not None:
        return args.port
    return 4002 if args.mode == "paper" else 4001


def _confirm_live_mode(port: int) -> None:
    """Require explicit confirmation before connecting to IB live gateway."""
    print(f"\n  WARNING: You are about to connect to IB LIVE gateway (port {port}).")
    print("  Real money orders will be placed.\n")
    answer = input("  Type 'YES' to confirm: ").strip()
    if answer != "YES":
        print("Aborted.")
        sys.exit(0)


def _build_ib_client(args):
    """Build the appropriate IB client based on --mode, --host, --port."""
    mode = args.mode

    if mode == "sim":
        from ib.paper import PaperClient
        print("Starting in SIM mode (local shim, no IB connection)")
        return PaperClient()

    from ib.client import IBClient, IBConfig
    host = args.host
    port = _resolve_port(args)

    if mode == "live":
        _confirm_live_mode(port)

    label = "PAPER" if mode == "paper" else "LIVE"
    print(f"Starting in {label} mode (IB Gateway at {host}:{port})")
    return IBClient(config=IBConfig(host=host, port=port))


async def _cmd_position_manager(args):
    """Run the position manager service loop."""
    from ib.position_manager import PositionManager
    from ib.types import ManagerConfig
    from openclaw.notify import MultiNotifier

    db = await _init_db(args)
    notifier = MultiNotifier()
    config = ManagerConfig(poll_interval_secs=args.poll_interval)
    ib_client = _build_ib_client(args)

    manager = PositionManager(db=db, ib_client=ib_client, config=config, notifier=notifier)

    try:
        await manager.run()
    except KeyboardInterrupt:
        print("\nPosition manager stopped.")
    finally:
        await db.close()


async def _cmd_scheduler(args):
    """Start the cron scheduler daemon with workflows + position management."""
    from openclaw.notify import MultiNotifier
    from openclaw.scheduler import WorkflowScheduler

    db = await _init_db(args)
    ib_client = _build_ib_client(args)
    engine = await _init_engine(args, ib_client=ib_client)
    notifier = MultiNotifier()

    auto_approve = getattr(args, "auto_approve", False)
    scheduler = WorkflowScheduler(
        engine=engine, db=db, ib_client=ib_client, notifier=notifier,
        auto_approve=auto_approve,
    )

    print("Registered schedules:")
    print("  - 8:00 AM ET Mon-Fri: Pre-market research (trade-thesis for watchlist)")
    print("  - 12:30 PM ET Mon-Fri: Midday position check")
    print("  - 4:30 PM ET Mon-Fri: Post-market position check")
    print("  - 10:00 AM ET Saturday: Weekly deep dive")
    if auto_approve:
        print("  AUTO-APPROVE: ON — recommendations will be executed without human review")
    print()

    try:
        await scheduler.start()
    except KeyboardInterrupt:
        print("\nScheduler stopped.")
    finally:
        await db.close()


if __name__ == "__main__":
    main()
