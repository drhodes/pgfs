#!/usr/bin/env bash
# test_lib.sh — shared harness for the pgfs integration test suite.
#
# Modeled on the harnesses the ext4 and ZFS suites build every test on:
#   - xfstests sources common/rc (via common/preamble) for _require_*,
#     $TEST_DIR, _register_cleanup, and the pass/fail accounting.
#   - OpenZFS zfs-tests sources include/libtest.shlib for log_assert /
#     log_must / log_pass.
# Here the same job is one file: project layout, Postgres bring-up, a
# clean mount at testdata/mnt, and pass/fail helpers. Every scripts/
# test_*.sh sources this and nothing else.
set -euo pipefail

# ── Pass/fail accounting ─────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $*"; }
fail() { echo -e "${RED}FAIL${NC} $*" >&2; exit 1; }

# ── Project layout (anchors everything to the repo root) ─────────────
cd "$(dirname "${BASH_SOURCE[0]}")/.."
ROOT="$PWD"
TESTDATA="$ROOT/testdata"
MOUNT="$TESTDATA/mnt"
PGDATA="$TESTDATA/pgdata"
PGFS_PID=""

mount_active() { grep -q " $MOUNT fuse" /proc/mounts 2>/dev/null; }

# ── Database ─────────────────────────────────────────────────────────
# ensure_db: init + start the project-local Postgres cluster if it is
# not already running. Never touches a system Postgres.
ensure_db() {
    if pg_ctl -D "$PGDATA" status >/dev/null 2>&1; then
        return 0
    fi
    echo "==> Initializing Postgres"
    ./scripts/init_db.sh >/dev/null 2>&1
}

# ── Mount lifecycle ──────────────────────────────────────────────────
# start_pgfs: sweep any stale mount, TRUNCATE the entries table, build,
# and mount at $MOUNT. Waits until the mount is live in /proc/mounts.
start_pgfs() {
    ensure_db
    # Wait for a stale mount from a previous run to fully vanish before
    # mounting over it (lazy unmount is async).
    fusermount3 -u -z "$MOUNT" 2>/dev/null || true
    for _ in $(seq 1 40); do
        mount_active || break
        sleep 0.1
    done
    psql -h "$TESTDATA" -d pgfs -c "TRUNCATE entries" >/dev/null 2>&1 || true

    cargo build --quiet 2>/dev/null
    mkdir -p "$MOUNT"
    target/debug/pgfs "$MOUNT" &
    PGFS_PID=$!
    for _ in $(seq 1 30); do
        mount_active && return 0
        if ! kill -0 "$PGFS_PID" 2>/dev/null; then
            echo "pgfs daemon exited during startup:" >&2
            tail -5 "$TESTDATA/pgfs.log" 2>/dev/null || true
            return 1
        fi
        sleep 0.1
    done
    return 1
}

# stop_pgfs: unmount and reap the daemon. Safe to call twice.
stop_pgfs() {
    fusermount3 -u -z "$MOUNT" 2>/dev/null || true
    if [ -n "$PGFS_PID" ]; then
        wait "$PGFS_PID" 2>/dev/null || true
        PGFS_PID=""
    fi
}

# default_cleanup: trap target for scripts that want the standard
# teardown. Any test that mounts must call start_pgfs and arrange for
# this (or its own) cleanup to run on EXIT.
default_cleanup() {
    echo "==> Cleaning up"
    stop_pgfs
}

# require_mounted: hard failure if the filesystem is not actually up.
require_mounted() {
    if ! mount_active; then
        fail "pgfs is not mounted at $MOUNT"
    fi
}
