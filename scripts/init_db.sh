#!/usr/bin/env bash
# init_db.sh — sets up a Postgres cluster that lives entirely inside this
# project's testdata/ directory (spec/app.py ProvidesProjectLocalCluster):
# a Unix socket in testdata/, TCP on 127.0.0.1 only, data checksums on,
# fsync and full_page_writes checked, and a `pgfs` database formatted with
# `pgfs mkfs`. Safe to `rm -rf testdata` when you want a clean slate.
#
#   ./scripts/init_db.sh            the cluster on the local disk
#   ./scripts/init_db.sh --lazyfs   the data directory on LazyFS, so crash
#                                   conformance can drop unsynced writes
#                                   (spec/verify.py CrashConformance)
set -euo pipefail

cd "$(dirname "$0")/.."   # project root
ROOT="$(pwd)"
PGDATA="$ROOT/testdata/pgdata"
SOCKDIR="$ROOT/testdata"
LOGFILE="$ROOT/testdata/postgres.log"
LAZYFS=0
[ "${1:-}" = "--lazyfs" ] && LAZYFS=1

mkdir -p "$SOCKDIR"

if [ "$LAZYFS" = 1 ]; then
    # LazyFS keeps unsynced writes in its own cache until fsync, so
    # "lazyfs::clear-cache" through its FIFO simulates power loss.
    command -v lazyfs >/dev/null || { echo "error: lazyfs is not installed (https://github.com/dsrhaslab/lazyfs)" >&2; exit 2; }
    LAZY_ROOT="$ROOT/testdata/lazyfs-root"
    LAZY_MNT="$ROOT/testdata/lazyfs"
    mkdir -p "$LAZY_ROOT" "$LAZY_MNT"
    if ! grep -q " $LAZY_MNT " /proc/mounts; then
        cat > "$ROOT/testdata/lazyfs.toml" <<TOML
[faults]
fifo_path="$ROOT/testdata/lazyfs.fifo"
[cache]
apply_eviction=false
[cache.simple]
custom_size="1gb"
blocks_per_page=1
[filesystem]
logfile=""
log_all_operations=false
TOML
        lazyfs "$LAZY_MNT" --config-path "$ROOT/testdata/lazyfs.toml" -o allow_other -o modules=subdir -o subdir="$LAZY_ROOT" &
        for _ in $(seq 1 50); do grep -q " $LAZY_MNT " /proc/mounts && break; sleep 0.1; done
    fi
    PGDATA="$LAZY_MNT/pgdata"
    ln -sfn "$PGDATA" "$ROOT/testdata/pgdata"
fi

if [ ! -d "$PGDATA" ]; then
    echo "initializing new cluster at $PGDATA"
    # --auth=trust is fine here: the socket lives in a directory only this
    # user can read, and TCP listens on loopback only.
    initdb -D "$PGDATA" --auth=trust --no-locale --encoding=UTF8 --data-checksums
fi

if ! pg_ctl -D "$PGDATA" status >/dev/null 2>&1; then
    echo "starting postgres (unix socket + TCP on 127.0.0.1 only)"
    pg_ctl -D "$PGDATA" -l "$LOGFILE" \
        -o "-k $SOCKDIR -h 127.0.0.1" \
        start
fi

USER_NAME="$(id -un)"
SUPERUSER="$(stat -c '%U' "$PGDATA")"

# Ensure current executing user role exists in postgres
psql -h "$SOCKDIR" -U "$SUPERUSER" -d postgres -c "DO \$$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '$USER_NAME') THEN CREATE USER \"$USER_NAME\" WITH SUPERUSER; END IF; END \$$;" 2>/dev/null || true

# createdb needs a moment after start; retry briefly.
for i in $(seq 1 10); do
    if psql -h "$SOCKDIR" -U "$USER_NAME" -d postgres -c '\q' 2>/dev/null; then
        break
    fi
    sleep 0.5
done

if ! psql -h "$SOCKDIR" -U "$USER_NAME" -d postgres -c '\q' 2>/dev/null; then
    echo "error: postgres did not answer on $SOCKDIR within 5s" >&2
    echo "last lines of $LOGFILE:" >&2
    tail -20 "$LOGFILE" >&2
    exit 1
fi

# Durability is possible only with fsync and full_page_writes on
# (spec/durability.py ChecksServerDurability).
for setting in fsync full_page_writes; do
    v=$(psql -h "$SOCKDIR" -U "$USER_NAME" -d postgres -Atc "show $setting")
    if [ "$v" != "on" ]; then
        echo "error: $setting is $v; pgfs needs it on" >&2
        exit 1
    fi
done
[ "$(psql -h "$SOCKDIR" -U "$USER_NAME" -d postgres -Atc 'show data_checksums')" = "on" ] \
    || echo "warning: data_checksums is off (this cluster predates --data-checksums)"

if ! psql -h "$SOCKDIR" -U "$USER_NAME" -lqt | cut -d'|' -f1 | grep -qw pgfs; then
    echo "creating database 'pgfs'"
    createdb -h "$SOCKDIR" -U "$USER_NAME" pgfs
fi

CONN="host=$SOCKDIR dbname=pgfs"
if [ "$(psql -h "$SOCKDIR" -U "$USER_NAME" -d pgfs -Atc "select to_regclass('pgfs.meta') is null")" = "t" ]; then
    echo "formatting the pgfs database (pgfs mkfs)"
    cargo build --quiet
    PGFS_CONN="$CONN" "$ROOT/target/debug/pgfs" mkfs </dev/null
fi

echo
echo "ready. connection string for pgfs:"
echo "  $CONN"
echo
echo "to stop:   pg_ctl -D $PGDATA stop"
echo "to nuke:   pg_ctl -D $PGDATA stop; rm -rf $ROOT/testdata/pgdata"
