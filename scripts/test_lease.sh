#!/usr/bin/env bash
# test_lease.sh — one read-write mount per tree, any number of read-only
# ones (spec/consistency.py ExclusiveTreeLease, ReadOnlyLiveMounts).
source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"
trap 'stop_pgfs; fusermount3 -u -z "$TESTDATA/mnt-ro" 2>/dev/null || true' EXIT
start_pgfs || fail "pgfs failed to mount"
mkdir -p "$TESTDATA/mnt-ro" "$TESTDATA/mnt-rw2"
out=$(timeout 10 target/debug/pgfs mount main "$TESTDATA/mnt-rw2" 2>&1) && fail "a second read-write mount succeeded"
echo "$out" | grep -q "EBUSY" || fail "second mount error does not say EBUSY: $out"
pass "second read-write mount refused, naming the holder"

echo v1 > "$MOUNT/f"
sync "$MOUNT/f"
target/debug/pgfs mount --read-only main "$TESTDATA/mnt-ro" >>"$TESTDATA/pgfs.log" 2>&1 &
RO=$!
for _ in $(seq 1 30); do grep -q " $TESTDATA/mnt-ro fuse" /proc/mounts && break; sleep 0.1; done
[ "$(cat "$TESTDATA/mnt-ro/f")" = v1 ] || fail "read-only mount content"
echo v2 > "$MOUNT/f"
sleep 1.5   # the read-only mount's attribute TTL
[ "$(cat "$TESTDATA/mnt-ro/f")" = v2 ] || fail "read-only mount did not see the change after its TTL"
! touch "$TESTDATA/mnt-ro/x" 2>/dev/null || fail "read-only mount accepted a write"
kill -TERM $RO; wait $RO 2>/dev/null || true
pass "read-only live mount alongside the writer"
