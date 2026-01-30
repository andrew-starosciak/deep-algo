#!/bin/bash
# Database migration runner for algo-trade
# Usage: ./scripts/migrate.sh [--db-url URL]

set -e

# Get database URL
if [ -n "$1" ] && [ "$1" = "--db-url" ]; then
    DATABASE_URL="$2"
elif [ -n "$DATABASE_URL" ]; then
    : # Use existing DATABASE_URL
elif [ -f "secrets/db_password.txt" ]; then
    DB_PASSWORD=$(cat secrets/db_password.txt)
    DATABASE_URL="postgresql://postgres:${DB_PASSWORD}@localhost:5432/algo_trade"
else
    echo "Error: No database URL provided"
    echo "Usage: ./scripts/migrate.sh --db-url <url>"
    echo "   or: export DATABASE_URL=..."
    exit 1
fi

echo "Running migrations..."

# Create migrations tracking table if not exists
psql "$DATABASE_URL" -c "
CREATE TABLE IF NOT EXISTS _migrations (
    version VARCHAR(100) PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
" 2>/dev/null || true

# Get list of applied migrations
APPLIED=$(psql "$DATABASE_URL" -t -c "SELECT version FROM _migrations ORDER BY version;" 2>/dev/null | tr -d ' ')

# Apply pending migrations
MIGRATIONS_DIR="$(dirname "$0")/migrations"
for migration in "$MIGRATIONS_DIR"/V*.sql; do
    if [ ! -f "$migration" ]; then
        continue
    fi

    VERSION=$(basename "$migration" .sql)

    if echo "$APPLIED" | grep -q "^${VERSION}$"; then
        echo "  [SKIP] $VERSION (already applied)"
    else
        echo "  [APPLY] $VERSION..."
        psql "$DATABASE_URL" -f "$migration" >/dev/null 2>&1 || {
            # Check if it's just a "already exists" error
            psql "$DATABASE_URL" -f "$migration" 2>&1 | grep -q "already exists" && {
                echo "    (objects already exist, marking as applied)"
            } || {
                echo "    ERROR applying migration!"
                exit 1
            }
        }
        psql "$DATABASE_URL" -c "INSERT INTO _migrations (version) VALUES ('$VERSION') ON CONFLICT DO NOTHING;" >/dev/null
        echo "    Done."
    fi
done

echo ""
echo "Migration status:"
psql "$DATABASE_URL" -c "SELECT version, applied_at FROM _migrations ORDER BY version;"
