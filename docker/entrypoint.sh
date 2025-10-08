#!/bin/bash
set -e

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

# Start trading daemon in background
echo "Starting trading daemon..."
algo-trade run --config "${CONFIG_PATH:-/config/Config.toml}" &
daemon_pid=$!

# Wait for daemon to initialize
sleep 2

# Start ttyd for TUI access
echo "Starting ttyd on port 7681..."
ttyd -p 7681 -W algo-trade live-bot-tui &
ttyd_pid=$!

echo "Services started:"
echo "  - Trading daemon (PID: $daemon_pid)"
echo "  - ttyd web terminal (PID: $ttyd_pid)"
echo "Access TUI at http://localhost:7681"

# Wait for either process to exit
wait -n "$daemon_pid" "$ttyd_pid"

# If we reach here, one process exited - trigger shutdown
shutdown
