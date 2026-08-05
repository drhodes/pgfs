#!/usr/bin/env bash
# test_replica.sh — replicated mode end-to-end (see spec/replica.py).
#
# Requires docker + the postgres:17 image. When docker is unavailable the
# test skips (exit 0) so the suite still runs on machines without docker.
#
#  1. provision the Docker streaming standby (scripts/replica_db.sh up)
#  2. mount pgfs with --replica pointing at the standby
#  3. write through the mount (must land on the primary, replicate to standby)
#  4. verify the standby directly via psql
#  5. read through the mount (may be served from the replica)
set -euo pipefail
. ./scripts/test_lib.sh

if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    echo "SKIP: docker not available — replicated-mode test skipped"
    exit 0
fi

# ── 1. Provision the standby ─────────────────────────────────────────
if ! ./scripts/replica_db.sh up >/dev/null 2>&1; then
    fail "replica_db.sh up failed (see testdata/replica.log or docker logs)"
fi

REPLICA_CONN=$(./scripts/replica_db.sh conn)
pass "docker streaming standby provisioned (--replica $REPLICA_CONN)"

# ── 2. Mount with --replica ───────────────────────────────────────────
# default_cleanup (from test_lib.sh): stop_pgfs + unmount on exit.
trap default_cleanup EXIT
start_pgfs_with_replica() {
    ensure_db
    fusermount3 -u -z "$MOUNT" 2>/dev/null || true
    for _ in $(seq 1 40); do
        mount_active || break
        sleep 0.1
    done
    psql -h "$TESTDATA" -d pgfs -c "TRUNCATE entries" >/dev/null 2>&1 || true

    cargo build --quiet 2>/dev/null
    mkdir -p "$MOUNT"
    target/debug/pgfs "$MOUNT" --replica "$REPLICA_CONN" &
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
start_pgfs_with_replica || fail "pgfs with --replica failed to mount"
require_mounted

# ── 3. Write through the mount (goes to primary) ──────────────────────
echo "==> write via mount"
echo "replica-test-payload" > "$MOUNT/repl.txt"
sleep 0.5

# ── 4. Verify the write replicated to the standby ─────────────────────
echo "==> verify row on standby (replication)"
for _ in $(seq 1 20); do
    STANDBY_DATA=$(psql -h 127.0.0.1 -p 5433 -U "$(whoami)" -d pgfs -Atc \
        "SELECT convert_from(data, 'UTF8') FROM entries WHERE name='repl.txt'" 2>/dev/null || true)
    if [ "$STANDBY_DATA" = "replica-test-payload" ]; then
        break
    fi
    sleep 0.5
done
if [ "$STANDBY_DATA" != "replica-test-payload" ]; then
    fail "write did not replicate to standby (got: '$STANDBY_DATA')"
fi
pass "write replicated primary → standby"

# ── 5. Read through the mount ─────────────────────────────────────────
echo "==> read via mount"
CONTENT=$(cat "$MOUNT/repl.txt")
if [ "$CONTENT" != "replica-test-payload" ]; then
    fail "read-back mismatch: '$CONTENT'"
fi
pass "read-back through replica-mode mount"

# ── 6. Fallback: with the standby stopped, reads still work ───────────
echo "==> stop standby, verify graceful fallback"
./scripts/replica_db.sh down >/dev/null 2>&1
sleep 1
CONTENT=$(cat "$MOUNT/repl.txt")
if [ "$CONTENT" != "replica-test-payload" ]; then
    fail "read failed after standby stop (should fall back to primary): '$CONTENT'"
fi
pass "reads fall back to primary when standby is down"

echo
echo "=============================="
echo " All replica tests passed."
echo "=============================="
