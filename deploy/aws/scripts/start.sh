#!/bin/bash
# Start/stop/restart trading bot
set -e

ACTION="${1:-start}"
BOT_DIR="/opt/trading-bot"

case "$ACTION" in
    start)
        echo "Starting trading bot..."
        sudo systemctl start trading-bot
        sleep 2
        sudo systemctl status trading-bot --no-pager
        ;;
    stop)
        echo "Stopping trading bot..."
        sudo systemctl stop trading-bot
        ;;
    restart)
        echo "Restarting trading bot..."
        sudo systemctl restart trading-bot
        sleep 2
        sudo systemctl status trading-bot --no-pager
        ;;
    status)
        sudo systemctl status trading-bot --no-pager
        ;;
    logs)
        journalctl -u trading-bot -f
        ;;
    paper)
        echo "Starting in paper trading mode..."
        sudo systemctl stop trading-bot 2>/dev/null || true
        cd "$BOT_DIR"
        RUST_LOG=info,algo_trade_polymarket=debug ./algo-trade-cli gabagool-auto \
            --mode paper \
            --duration "${2:-24h}"
        ;;
    live)
        echo "Starting in LIVE trading mode..."
        echo "WARNING: This will execute real trades!"
        read -p "Are you sure? (yes/no): " confirm
        if [ "$confirm" != "yes" ]; then
            echo "Aborted."
            exit 1
        fi
        sudo systemctl stop trading-bot 2>/dev/null || true
        cd "$BOT_DIR"
        RUST_LOG=info,algo_trade_polymarket=debug ./algo-trade-cli gabagool-auto \
            --mode live \
            --duration "${2:-24h}" \
            --max-bet 5
        ;;
    *)
        echo "Usage: $0 {start|stop|restart|status|logs|paper|live}"
        exit 1
        ;;
esac
