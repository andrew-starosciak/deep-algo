#!/bin/bash
#
# Cross-Market Settlement Runner
#
# Runs the settlement handler to resolve pending trades.
# Run this alongside run_cross_market_auto.sh
#
# Usage:
#   ./scripts/run_cross_market_settle.sh [options]
#
# Options:
#   --once              Process one batch and exit
#   --delay <secs>      Settlement delay after window close (default: 120)
#   --verbose           Show verbose output
#

set -e

# Auto-source .env file if it exists
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    source "$PROJECT_ROOT/.env"
    set +a
fi

# Check DATABASE_URL
if [[ -z "$DATABASE_URL" ]]; then
    echo "ERROR: DATABASE_URL environment variable required"
    exit 1
fi

# Colors
CYAN='\033[0;36m'
WHITE='\033[1;37m'
DIM='\033[2m'
NC='\033[0m'

echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║${NC}           ${WHITE}Cross-Market Settlement Handler${NC}                       ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${DIM}Settles pending trades after 15-minute windows expire${NC}"
echo -e "${DIM}Press Ctrl+C to stop${NC}"
echo ""

# Pass through all arguments
exec cargo run -p algo-trade-cli --release -- cross-market-settle "$@"
