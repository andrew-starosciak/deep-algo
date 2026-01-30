#!/bin/bash
set -e

# Fix permissions for /data volume (must run as root)
if [ "$(id -u)" = "0" ]; then
    echo "Fixing /data volume permissions..."
    chown -R algotrader:algotrader /data
    chmod -R 755 /data

    # Switch to algotrader user and re-exec this script
    echo "Switching to algotrader user..."
    exec gosu algotrader "$0" "$@"
fi

# If command arguments are passed (e.g., from docker-compose command:), execute them directly
if [ $# -gt 0 ]; then
    echo "Running command: algo-trade $@"
    exec algo-trade "$@"
fi

# Function to handle graceful shutdown
shutdown() {
    echo "Shutting down gracefully..."
    kill -TERM "$daemon_pid" 2>/dev/null || true
    kill -TERM "$ttyd_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
    wait "$ttyd_pid" 2>/dev/null || true
    exit 0
}

# Trap SIGTERM and SIGINT
trap shutdown TERM INT

# Start services based on mode
if [ "${BACKTEST_MANAGER_TUI:-false}" = "true" ]; then
    # Backtest manager mode - only run TUI (no daemon needed)
    echo "Starting backtest-manager-tui via ttyd on port 7681..."
    ttyd -p 7681 -W algo-trade backtest-manager-tui &
    ttyd_pid=$!
    daemon_pid=""

    echo "Services started:"
    echo "  - Backtest Manager TUI (PID: $ttyd_pid)"
    echo "Access TUI at http://localhost:7681"

    # Wait for TUI process to exit
    wait "$ttyd_pid"
else
    # Normal mode - run trading daemon + live bot TUI
    echo "Starting trading daemon as $(whoami)..."
    algo-trade run --config "${CONFIG_PATH:-/config/Config.toml}" &
    daemon_pid=$!

    # Wait for daemon to initialize
    sleep 2

    # Start ttyd for live bot TUI
    echo "Starting ttyd with live-bot-tui on port 7681..."
    ttyd -p 7681 -W algo-trade live-bot-tui &
    ttyd_pid=$!

    echo "Services started:"
    echo "  - Trading daemon (PID: $daemon_pid)"
    echo "  - ttyd web terminal (PID: $ttyd_pid)"
    echo "Access TUI at http://localhost:7681"

    # Wait for either process to exit
    wait -n "$daemon_pid" "$ttyd_pid"
fi

# If we reach here, one process exited - trigger shutdown
shutdown
