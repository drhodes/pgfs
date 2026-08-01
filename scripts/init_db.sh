#!/usr/bin/env bash
# init_db.sh — sets up a Postgres cluster that lives entirely inside this
# project's testdata/ directory. No system-wide install location, no
# /var/lib/postgresql, no TCP listener — Unix socket only, scoped to this
# folder. Safe to `rm -rf testdata` when you want a clean slate.
set -euo pipefail

cd "$(dirname "$0")/.."   # project root
PGDATA="$(pwd)/testdata/pgdata"
SOCKDIR="$(pwd)/testdata"
LOGFILE="$(pwd)/testdata/postgres.log"

mkdir -p "$SOCKDIR"

if [ ! -d "$PGDATA" ]; then
    echo "initializing new cluster at $PGDATA"
    # --auth=trust is fine here because we're only ever listening on a
    # Unix socket inside a directory only this user can read (see -k below)
    # -- there is no network exposure to secure against.
    initdb -D "$PGDATA" --auth=trust --no-locale --encoding=UTF8
fi

echo "starting postgres (unix socket only, no TCP)"
pg_ctl -D "$PGDATA" -l "$LOGFILE" \
    -o "-k $SOCKDIR -h ''" \
    start

# createdb needs a moment after start; retry briefly.
for i in $(seq 1 10); do
    if psql -h "$SOCKDIR" -d postgres -c '\q' 2>/dev/null; then
        break
    fi
    sleep 0.5
done

if ! psql -h "$SOCKDIR" -d postgres -c '\q' 2>/dev/null; then
    echo "error: postgres did not answer on $SOCKDIR within 5s" >&2
    echo "last lines of $LOGFILE:" >&2
    tail -20 "$LOGFILE" >&2
    exit 1
fi

if ! psql -h "$SOCKDIR" -lqt | cut -d'|' -f1 | grep -qw pgfs; then
    echo "creating database 'pgfs'"
    createdb -h "$SOCKDIR" pgfs
fi

echo
echo "ready. connection string for pgfs:"
echo "  host=$SOCKDIR dbname=pgfs"
echo
echo "to stop:   pg_ctl -D $PGDATA stop"
echo "to nuke:   pg_ctl -D $PGDATA stop; rm -rf $(pwd)/testdata/pgdata"
