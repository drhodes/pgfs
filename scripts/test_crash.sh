#!/usr/bin/env bash
# test_crash.sh — crashes of the daemon and of Postgres (spec/verify.py
# CrashConformance, the local subset without LazyFS): fsync'd data
# survives, write-then-rename is atomic, `pgfs check` passes after
# recovery, and a scratch filesystem resets after a Postgres crash.
source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"
trap default_cleanup EXIT
export PGFS_MOUNT_ARGS="--lease 6"

remount() {
    target/debug/pgfs mount ${1:-} --lease 6 main "$MOUNT" >>"$TESTDATA/pgfs.log" 2>&1 &
    PGFS_PID=$!
    for _ in $(seq 1 300); do mount_active && return 0; sleep 0.1; done
    return 1
}
pg_restart_after_crash() {
    pg_ctl -D "$PGDATA" -w -m immediate stop >/dev/null
    pg_ctl -D "$PGDATA" -w start -l "$TESTDATA/postgres.log" -o "-k $TESTDATA -h 127.0.0.1" >/dev/null
}

# 1. kill -9 the daemon after fsync: the data is there.
start_pgfs || fail "mount"
python3 - "$MOUNT" <<'PY'
import os, sys
m = sys.argv[1]
fd = os.open(f"{m}/synced", os.O_CREAT | os.O_WRONLY, 0o644)
os.write(fd, b"durable"); os.fsync(fd); os.close(fd)
# write-then-rename without fsync
with open(f"{m}/config.tmp", "w") as f:
    f.write("new config")
os.rename(f"{m}/config.tmp", f"{m}/config")
PY
kill -9 "$PGFS_PID"; wait "$PGFS_PID" 2>/dev/null || true
fusermount3 -u -z "$MOUNT" 2>/dev/null || true
for _ in $(seq 1 40); do mount_active || break; sleep 0.1; done
remount --takeover || fail "remount after a daemon crash"
[ "$(cat "$MOUNT/synced")" = durable ] || fail "fsync'd data lost after a daemon crash"
[ "$(cat "$MOUNT/config")" = "new config" ] || fail "rename lost its writes"
pass "daemon kill -9: fsync'd data and write-then-rename survive"

# 2. Crash Postgres (immediate stop) after fsync.
python3 -c "
import os
fd = os.open('$MOUNT/pgcrash', os.O_CREAT | os.O_WRONLY, 0o644)
os.write(fd, b'survives'); os.fsync(fd); os.close(fd)"
stop_pgfs
pg_restart_after_crash
target/debug/pgfs check >/dev/null || fail "pgfs check after a Postgres crash"
remount || fail "remount after a Postgres crash"
[ "$(cat "$MOUNT/pgcrash")" = survives ] || fail "fsync'd data lost after a Postgres crash"
pass "Postgres crash: fsync'd data survives and pgfs check passes"
stop_pgfs

# 3. A scratch filesystem is reset by the next mount after a Postgres crash.
target/debug/pgfs mkfs --force --scratch </dev/null >/dev/null || fail "mkfs --scratch"
remount || fail "mount scratch"
echo gone > "$MOUNT/x"
stop_pgfs
pg_restart_after_crash
remount || fail "mount scratch after a crash"
[ ! -e "$MOUNT/x" ] || fail "scratch contents survived a crash?"
grep -q "has been reset\|scratch filesystem was emptied" "$TESTDATA/pgfs.log" || fail "no scratch reset logged"
pass "scratch filesystem resets after a Postgres crash"
