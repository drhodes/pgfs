#!/usr/bin/env bash
# ensure_db.sh — ensures Postgres cluster is initialized, running, and pgfs DB exists.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PGDATA="$ROOT/testdata/pgdata"
SOCKDIR="$ROOT/testdata"

USER_NAME="$(id -un)"

if pg_ctl -D "$PGDATA" status >/dev/null 2>&1; then
    if psql -h "$SOCKDIR" -U "$USER_NAME" -d postgres -c '\q' 2>/dev/null; then
        exit 0
    fi
fi

exec "$ROOT/scripts/init_db.sh"
