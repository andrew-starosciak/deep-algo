#!/bin/bash
# =============================================================================
# Polymarket BTC 15-Minute Binary Options Paper Trading Bot
# =============================================================================
# This script starts all components needed for paper trading:
# 1. Liquidation/funding data collection (background)
# 2. Polymarket odds collection (background)
# 3. Paper trading bot with real signals (foreground)
#
# Usage:
#   ./scripts/start-polymarket-bot.sh [--duration 24h] [--signal-mode cascade]
#
# Requirements:
#   - DATABASE_URL environment variable set
#   - PostgreSQL/TimescaleDB running
#   - cargo build completed
# =============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default configuration
DURATION="${DURATION:-24h}"
SIGNAL_MODE="${SIGNAL_MODE:-cascade}"
MIN_SIGNAL_STRENGTH="${MIN_SIGNAL_STRENGTH:-0.6}"
MIN_EDGE="${MIN_EDGE:-0.03}"
KELLY_FRACTION="${KELLY_FRACTION:-0.25}"
ENTRY_STRATEGY="${ENTRY_STRATEGY:-edge_threshold}"
ENTRY_THRESHOLD="${ENTRY_THRESHOLD:-0.05}"
MIN_VOLUME_USD="${MIN_VOLUME_USD:-100000}"
IMBALANCE_THRESHOLD="${IMBALANCE_THRESHOLD:-0.6}"
BANKROLL="${BANKROLL:-10000}"

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --duration)
            DURATION="$2"
            shift 2
            ;;
        --signal-mode)
            SIGNAL_MODE="$2"
            shift 2
            ;;
        --min-signal-strength)
            MIN_SIGNAL_STRENGTH="$2"
            shift 2
            ;;
        --min-edge)
            MIN_EDGE="$2"
            shift 2
            ;;
        --kelly-fraction)
            KELLY_FRACTION="$2"
            shift 2
            ;;
        --entry-strategy)
            ENTRY_STRATEGY="$2"
            shift 2
            ;;
        --entry-threshold)
            ENTRY_THRESHOLD="$2"
            shift 2
            ;;
        --min-volume-usd)
            MIN_VOLUME_USD="$2"
            shift 2
            ;;
        --bankroll)
            BANKROLL="$2"
            shift 2
            ;;
        --simulated)
            USE_SIMULATED="--use-simulated-signals"
            shift
            ;;
        --help)
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  --duration <time>         Trading duration (default: 24h)"
            echo "  --signal-mode <mode>      cascade|exhaustion|combined (default: cascade)"
            echo "  --min-signal-strength <n> Minimum signal strength 0.0-1.0 (default: 0.6)"
            echo "  --min-edge <n>            Minimum edge threshold (default: 0.03)"
            echo "  --kelly-fraction <n>      Kelly fraction (default: 0.25)"
            echo "  --entry-strategy <s>      immediate|fixed_time|edge_threshold (default: edge_threshold)"
            echo "  --entry-threshold <n>     Edge threshold for entry (default: 0.05)"
            echo "  --min-volume-usd <n>      Min liquidation volume (default: 100000)"
            echo "  --bankroll <n>            Starting bankroll (default: 10000)"
            echo "  --simulated               Use simulated signals (for testing)"
            echo "  --help                    Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check prerequisites
if [ -z "$DATABASE_URL" ]; then
    echo -e "${RED}ERROR: DATABASE_URL environment variable not set${NC}"
    echo "Please set DATABASE_URL in your .env file or environment"
    exit 1
fi

# Build if needed
BINARY="./target/debug/algo-trade"
if [ ! -f "$BINARY" ]; then
    echo -e "${YELLOW}Building algo-trade CLI...${NC}"
    cargo build -p algo-trade-cli
fi

# Create logs directory
mkdir -p logs

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Shutting down...${NC}"

    # Kill background processes
    if [ ! -z "$COLLECTOR_PID" ]; then
        echo "Stopping data collector (PID: $COLLECTOR_PID)"
        kill $COLLECTOR_PID 2>/dev/null || true
    fi

    if [ ! -z "$POLYMARKET_PID" ]; then
        echo "Stopping Polymarket collector (PID: $POLYMARKET_PID)"
        kill $POLYMARKET_PID 2>/dev/null || true
    fi

    echo -e "${GREEN}Shutdown complete${NC}"
    exit 0
}

trap cleanup SIGINT SIGTERM

# =============================================================================
# Start Components
# =============================================================================

echo -e "${BLUE}=============================================${NC}"
echo -e "${BLUE}  Polymarket BTC 15-min Paper Trading Bot${NC}"
echo -e "${BLUE}=============================================${NC}"
echo ""
echo -e "Configuration:"
echo -e "  Duration:          ${GREEN}$DURATION${NC}"
echo -e "  Signal Mode:       ${GREEN}$SIGNAL_MODE${NC}"
echo -e "  Min Signal:        ${GREEN}$MIN_SIGNAL_STRENGTH${NC}"
echo -e "  Min Edge:          ${GREEN}$MIN_EDGE${NC}"
echo -e "  Kelly Fraction:    ${GREEN}$KELLY_FRACTION${NC}"
echo -e "  Entry Strategy:    ${GREEN}$ENTRY_STRATEGY${NC}"
echo -e "  Entry Threshold:   ${GREEN}$ENTRY_THRESHOLD${NC}"
echo -e "  Min Volume USD:    ${GREEN}$MIN_VOLUME_USD${NC}"
echo -e "  Bankroll:          ${GREEN}\$$BANKROLL${NC}"
if [ ! -z "$USE_SIMULATED" ]; then
    echo -e "  Signals:           ${YELLOW}SIMULATED (testing mode)${NC}"
else
    echo -e "  Signals:           ${GREEN}REAL (liquidation cascade)${NC}"
fi
echo ""

# -----------------------------------------------------------------------------
# 1. Start data collection (liquidations, funding, orderbook)
# -----------------------------------------------------------------------------
echo -e "${YELLOW}[1/3] Starting data collection...${NC}"

$BINARY collect-signals \
    --duration "$DURATION" \
    --sources liquidations,funding \
    > logs/data-collector.log 2>&1 &
COLLECTOR_PID=$!

echo -e "  Data collector started (PID: $COLLECTOR_PID)"
echo -e "  Log: logs/data-collector.log"
sleep 2

# Check if collector started successfully
if ! kill -0 $COLLECTOR_PID 2>/dev/null; then
    echo -e "${RED}ERROR: Data collector failed to start${NC}"
    echo "Check logs/data-collector.log for details"
    exit 1
fi

# -----------------------------------------------------------------------------
# 2. Start Polymarket odds collection
# -----------------------------------------------------------------------------
echo -e "${YELLOW}[2/3] Starting Polymarket odds collection...${NC}"

$BINARY collect-polymarket \
    --duration "$DURATION" \
    --market-pattern "Bitcoin|BTC" \
    --min-liquidity 5000 \
    --poll-interval-secs 15 \
    > logs/polymarket-collector.log 2>&1 &
POLYMARKET_PID=$!

echo -e "  Polymarket collector started (PID: $POLYMARKET_PID)"
echo -e "  Log: logs/polymarket-collector.log"
sleep 2

# Check if polymarket collector started successfully
if ! kill -0 $POLYMARKET_PID 2>/dev/null; then
    echo -e "${RED}ERROR: Polymarket collector failed to start${NC}"
    echo "Check logs/polymarket-collector.log for details"
    cleanup
fi

# -----------------------------------------------------------------------------
# 3. Start paper trading bot (foreground)
# -----------------------------------------------------------------------------
echo -e "${YELLOW}[3/3] Starting paper trading bot...${NC}"
echo ""
echo -e "${BLUE}=============================================${NC}"
echo -e "${BLUE}  Paper Trading Active - Press Ctrl+C to stop${NC}"
echo -e "${BLUE}=============================================${NC}"
echo ""

# Build the command
PAPER_TRADE_CMD="$BINARY polymarket-paper-trade \
    --duration $DURATION \
    --signal-mode $SIGNAL_MODE \
    --min-signal-strength $MIN_SIGNAL_STRENGTH \
    --min-edge $MIN_EDGE \
    --kelly-fraction $KELLY_FRACTION \
    --entry-strategy $ENTRY_STRATEGY \
    --entry-threshold $ENTRY_THRESHOLD \
    --min-volume-usd $MIN_VOLUME_USD \
    --imbalance-threshold $IMBALANCE_THRESHOLD \
    --bankroll $BANKROLL \
    --liquidation-window-mins 5 \
    --liquidation-symbol BTCUSDT \
    --liquidation-exchange binance"

# Add simulated flag if requested
if [ ! -z "$USE_SIMULATED" ]; then
    PAPER_TRADE_CMD="$PAPER_TRADE_CMD --use-simulated-signals"
fi

# Run paper trading in foreground with enhanced logging
RUST_LOG=info,algo_trade_cli::commands::polymarket_paper_trade=debug \
    $PAPER_TRADE_CMD 2>&1 | tee logs/paper-trade.log

# Cleanup after paper trading exits
cleanup
