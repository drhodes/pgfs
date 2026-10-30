#!/usr/bin/env bash
# test_snapshots.sh — branchable trees from the command line while the
# tree is mounted (spec/branch.py BranchableTrees): an owner-drained
# snapshot captures buffered writes, a snapshot mounts read-only, a branch
# is promoted back, diff spans it, and export/import round-trips.
source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"
trap 'stop_pgfs; fusermount3 -u -z "$TESTDATA/mnt-snap" 2>/dev/null || true' EXIT
start_pgfs || fail "pgfs failed to mount"

mkdir -p "$MOUNT/src"
echo committed > "$MOUNT/src/a"
exec 3>"$MOUNT/src/b"
printf 'still buffered' >&3
pgfs snapshot main@before || fail "snapshot"
exec 3>&-
pgfs snapshot list | grep -q "main@before" || fail "snapshot list"
pass "snapshot while mounted"

mkdir -p "$TESTDATA/mnt-snap"
target/debug/pgfs mount main@before "$TESTDATA/mnt-snap" >>"$TESTDATA/pgfs.log" 2>&1 &
SNAP_PID=$!
for _ in $(seq 1 30); do grep -q " $TESTDATA/mnt-snap fuse" /proc/mounts && break; sleep 0.1; done
[ "$(cat "$TESTDATA/mnt-snap/src/b")" = "still buffered" ] || fail "the snapshot missed a buffered write"
! touch "$TESTDATA/mnt-snap/x" 2>/dev/null || fail "a snapshot must be read-only"
kill -TERM $SNAP_PID; wait $SNAP_PID 2>/dev/null || true
pass "snapshot mount is read-only and drained"

# A branch of the live tree, changed and promoted while main is unmounted.
pgfs branch main dev || fail "branch"
stop_pgfs
mkdir -p "$TESTDATA/mnt-snap"
target/debug/pgfs mount dev "$TESTDATA/mnt-snap" >>"$TESTDATA/pgfs.log" 2>&1 &
DEV_PID=$!
for _ in $(seq 1 30); do grep -q " $TESTDATA/mnt-snap fuse" /proc/mounts && break; sleep 0.1; done
echo changed > "$TESTDATA/mnt-snap/src/a"
rm "$TESTDATA/mnt-snap/src/b"
kill -TERM $DEV_PID; wait $DEV_PID 2>/dev/null || true
pgfs promote dev main || fail "promote"
pgfs diff main@before main | grep -q "src/b" || fail "diff across the promote"
pass "branch, promote and diff"

# Export the snapshot; import it elsewhere; compare.
pgfs export main@before > "$TESTDATA/before.tar" || fail "export"
pgfs import main:/restored < "$TESTDATA/before.tar" || fail "import"
start_mount() { target/debug/pgfs mount main "$MOUNT" >>"$TESTDATA/pgfs.log" 2>&1 & PGFS_PID=$!; for _ in $(seq 1 30); do mount_active && break; sleep 0.1; done; }
start_mount
[ "$(cat "$MOUNT/restored/src/b")" = "still buffered" ] || fail "import content"
[ "$(cat "$MOUNT/src/a")" = "changed" ] || fail "promoted content"
pass "export and import"
