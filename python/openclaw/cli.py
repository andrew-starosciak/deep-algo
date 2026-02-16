"""CLI entrypoint for OpenClaw."""

from __future__ import annotations

import argparse
import asyncio
import logging
import sys


def main():
    parser = argparse.ArgumentParser(
        prog="openclaw",
        description="LLM-driven options trading orchestrator",
    )
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="Enable debug logging"
    )

    subparsers = parser.add_subparsers(dest="command")

    # run <workflow> [--ticker TICKER]
    run_parser = subparsers.add_parser("run", help="Run a workflow")
    run_parser.add_argument("workflow", help="Workflow name (e.g., trade-thesis, premarket-prep)")
    run_parser.add_argument("--ticker", help="Ticker symbol (for trade-thesis workflow)")

    # status
    subparsers.add_parser("status", help="Show running/recent workflows")

    # history
    subparsers.add_parser("history", help="Past workflow runs and outcomes")

    # watchlist
    wl_parser = subparsers.add_parser("watchlist", help="Manage watchlist")
    wl_sub = wl_parser.add_subparsers(dest="wl_command")

    wl_add = wl_sub.add_parser("add", help="Add ticker to watchlist")
    wl_add.add_argument("ticker", help="Ticker symbol")
    wl_add.add_argument("--sector", required=True, help="Sector classification")
    wl_add.add_argument("--notes", help="Optional notes")

    wl_sub.add_parser("show", help="Show current watchlist")

    wl_rm = wl_sub.add_parser("remove", help="Remove ticker from watchlist")
    wl_rm.add_argument("ticker", help="Ticker symbol")

    # scheduler
    subparsers.add_parser("scheduler", help="Start the cron scheduler daemon")

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    )

    if args.command is None:
        parser.print_help()
        sys.exit(1)

    asyncio.run(_dispatch(args))


async def _dispatch(args):
    """Route CLI commands to the appropriate handler."""
    if args.command == "run":
        print(f"Running workflow: {args.workflow}")
        if args.ticker:
            print(f"  Ticker: {args.ticker}")
        # TODO: Initialize engine, load workflow, execute
        print("  (not yet implemented — scaffolding only)")

    elif args.command == "status":
        print("Recent workflow runs:")
        # TODO: Query workflow_runs table
        print("  (not yet implemented)")

    elif args.command == "history":
        print("Workflow history:")
        # TODO: Query workflow_runs + step_logs
        print("  (not yet implemented)")

    elif args.command == "watchlist":
        if args.wl_command == "add":
            print(f"Adding {args.ticker} to watchlist (sector: {args.sector})")
            # TODO: INSERT INTO options_watchlist
        elif args.wl_command == "show":
            print("Current watchlist:")
            # TODO: SELECT FROM options_watchlist
        elif args.wl_command == "remove":
            print(f"Removing {args.ticker} from watchlist")
            # TODO: DELETE FROM options_watchlist
        else:
            print("Usage: openclaw watchlist {add|show|remove}")

    elif args.command == "scheduler":
        print("Starting scheduler daemon...")
        # TODO: Initialize engine + scheduler, run
        print("  (not yet implemented)")


if __name__ == "__main__":
    main()
