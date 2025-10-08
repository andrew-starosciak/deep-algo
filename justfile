# Hyperliquid Algorithmic Trading System - Task Runner
# Usage: just --list (or just -l) to see all available commands

# Default recipe - show available commands
default:
    @just --list

# Start the trading daemon (persistent bot orchestrator)
# Environment: BOT_DATABASE_URL (default: sqlite://bots.db)
daemon:
    @echo "Starting trading daemon with config/Config.toml..."
    @echo "Database: ${BOT_DATABASE_URL:-sqlite://bots.db}"
    cargo run -p algo-trade-cli -- run --config config/Config.toml

# Start the TUI for live bot management
tui:
    @echo "Starting TUI (Terminal User Interface)..."
    cargo run -p algo-trade-cli -- tui

# Start the web API server only (no TUI)
server host="0.0.0.0" port="8080":
    @echo "Starting web API server on {{host}}:{{port}}..."
    cargo run -p algo-trade-cli -- server --addr {{host}}:{{port}}

# Run a backtest for a specific symbol and strategy
# Example: just backtest BTC ma_crossover
backtest symbol strategy="ma_crossover" data="tests/data/sample.csv":
    @echo "Running backtest for {{symbol}} with {{strategy}} strategy..."
    cargo run -p algo-trade-cli -- backtest --data {{data}} --strategy {{strategy}} --symbol {{symbol}}

# Fetch historical data for a symbol
# Example: just fetch BTC 1h
fetch symbol interval:
    @echo "Fetching {{interval}} data for {{symbol}}..."
    cargo run -p algo-trade-data -- fetch {{symbol}} {{interval}}

# Run all tests
test:
    @echo "Running all tests..."
    cargo test

# Run tests for a specific crate
# Example: just test-crate backtest
test-crate crate:
    @echo "Running tests for {{crate}}..."
    cargo test -p algo-trade-{{crate}}

# Run integration tests only
test-integration:
    @echo "Running integration tests..."
    cargo test --test integration_test

# Build the project in release mode
build:
    @echo "Building release binary..."
    cargo build --release

# Build a specific crate
# Example: just build-crate core
build-crate crate:
    @echo "Building {{crate}}..."
    cargo build -p algo-trade-{{crate}}

# Check all crates for compilation errors (fast)
check:
    @echo "Checking all crates..."
    cargo check

# Run clippy linter (strict mode)
lint:
    @echo "Running clippy with strict lints..."
    cargo clippy -- -D warnings

# Run clippy on a specific crate
# Example: just lint-crate bot-orchestrator
lint-crate crate:
    @echo "Running clippy on {{crate}}..."
    cargo clippy -p algo-trade-{{crate}} -- -D warnings

# Format all code
fmt:
    @echo "Formatting code..."
    cargo fmt

# Format check (CI mode - doesn't modify files)
fmt-check:
    @echo "Checking code formatting..."
    cargo fmt -- --check

# Setup TimescaleDB (requires psql and running PostgreSQL)
db-setup:
    @echo "Setting up TimescaleDB..."
    psql -U postgres -f scripts/setup_timescale.sql

# Clean build artifacts
clean:
    @echo "Cleaning build artifacts..."
    cargo clean

# Show project information
info:
    @echo "Hyperliquid Algorithmic Trading System"
    @echo "======================================"
    @echo "Workspace crates:"
    @echo "  - core: Event types, traits, TradingSystem engine"
    @echo "  - exchange-hyperliquid: REST/WebSocket, rate limiting"
    @echo "  - data: TimescaleDB, Arrow, Parquet"
    @echo "  - strategy: Strategy implementations"
    @echo "  - backtest: Historical simulation"
    @echo "  - bot-orchestrator: Multi-bot coordination with persistence"
    @echo "  - web-api: REST + WebSocket API"
    @echo "  - cli: Command-line interface + TUI"
    @echo ""
    @echo "Quick start:"
    @echo "  just daemon   # Start persistent trading daemon"
    @echo "  just tui      # Start TUI for bot management"
    @echo "  just --list   # Show all commands"

# Development workflow - check + test + lint
dev:
    @echo "Running development workflow: check → test → lint..."
    just check
    just test
    just lint
    @echo "✅ All checks passed!"

# CI workflow - fmt-check + check + test + lint + build
ci:
    @echo "Running CI workflow..."
    just fmt-check
    just check
    just test
    just lint
    just build
    @echo "✅ CI workflow complete!"

# Watch mode for development (requires cargo-watch)
# Install: cargo install cargo-watch
watch:
    @echo "Starting watch mode (recompile on file changes)..."
    cargo watch -x check -x test

# Run with debug logging
# Example: just debug daemon
debug command:
    @echo "Running with debug logging: {{command}}..."
    RUST_LOG=debug just {{command}}
