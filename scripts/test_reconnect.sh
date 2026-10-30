#!/usr/bin/env bash
# test_reconnect.sh — Bug 5: the mount survives a Postgres restart
# (spec/failure.py DatabaseStateMachine, ReconnectsWithBackoff,
# RevalidatesAfterReconnect). Requests fail fast while the database is
# away, then the mount revalidates and serves again with nothing lost.
source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"
trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"

echo before > "$MOUNT/f"
sync "$MOUNT/f"
sleep 1   # idle past the asynchronous commit window
pg_ctl -D "$PGDATA" -w -m fast restart -l "$TESTDATA/postgres.log" -o "-k $TESTDATA -h 127.0.0.1" >/dev/null \
    || fail "restart postgres"
ok=0
for _ in $(seq 1 100); do
    if echo after >> "$MOUNT/f" 2>/dev/null; then ok=1; break; fi
    sleep 0.2
done
[ "$ok" = 1 ] || fail "the mount did not come back after a Postgres restart"
grep -q before "$MOUNT/f" && grep -q after "$MOUNT/f" || fail "content after reconnect: $(cat "$MOUNT/f")"
grep -q "Revalidating -> Connected\|revalidation passed" "$TESTDATA/pgfs.log" || fail "no revalidation logged"
pass "reconnects and revalidates after a Postgres restart (Bug 5)"
