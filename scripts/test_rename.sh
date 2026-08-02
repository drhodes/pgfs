#!/usr/bin/env bash
# test_rename.sh — integration test for rename-overwrite (POSIX semantics).
# Mounts pgfs, exercises rename via raw rename(2) syscall, verifies, cleans up.
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $*"; }
fail() { echo -e "${RED}FAIL${NC} $*" >&2; exit 1; }

cd "$(dirname "$0")/.."

# Helper: call the raw rename(2) syscall, bypassing mv's directory semantics.
# Usage: sys_rename <oldpath> <newpath>
sys_rename() {
    python3 -c "import os; os.rename('$1', '$2')"
}

# 1. Ensure Postgres is running; if not, init and start.
if ! pg_ctl -D testdata/pgdata status &>/dev/null; then
    echo "==> Initializing Postgres"
    ./scripts/init_db.sh >/dev/null 2>&1
fi

# 2. Ensure no stale mount and clean DB state.
fusermount3 -u -z testdata/mnt 2>/dev/null || true
sleep 0.3
psql -h "$(pwd)/testdata" -d pgfs -c "TRUNCATE entries" >/dev/null 2>&1 || true

# 3. Build and start pgfs in background.
cargo build --quiet 2>/dev/null
mkdir -p testdata/mnt
target/debug/pgfs testdata/mnt &
PGFS_PID=$!
for i in $(seq 1 20); do
    if mount | grep -q "testdata/mnt"; then break; fi
    sleep 0.1
done
if ! mount | grep -q "testdata/mnt"; then
    fail "pgfs failed to mount"
fi
echo "pgfs mounted (pid $PGFS_PID)"

cleanup() {
    echo "==> Cleaning up"
    fusermount3 -u -z testdata/mnt 2>/dev/null || true
    wait $PGFS_PID 2>/dev/null || true
}
trap cleanup EXIT

# ── Test 1: file-over-file rename overwrite ──────────────────────
echo "==> Test 1: file-over-file rename overwrite"
echo "hello" > testdata/mnt/a.txt
echo "world" > testdata/mnt/b.txt
mv testdata/mnt/b.txt testdata/mnt/a.txt
CONTENT=$(cat testdata/mnt/a.txt)
if [ "$CONTENT" != "world" ]; then
    fail "expected 'world' in a.txt after overwrite, got '$CONTENT'"
fi
if [ -f testdata/mnt/b.txt ]; then
    fail "b.txt should not exist after rename"
fi
pass "file-over-file rename overwrite"

# ── Test 2: rename to new name (no overwrite) ────────────────────
echo "==> Test 2: rename to new name"
echo "foo" > testdata/mnt/x.txt
mv testdata/mnt/x.txt testdata/mnt/y.txt
if [ -f testdata/mnt/x.txt ]; then
    fail "x.txt should not exist after rename"
fi
CONTENT=$(cat testdata/mnt/y.txt)
if [ "$CONTENT" != "foo" ]; then
    fail "expected 'foo' in y.txt, got '$CONTENT'"
fi
pass "rename to new name works"

# ── Test 3: empty-dir over empty-dir rename overwrite ────────────
# Uses raw rename(2) because mv moves src INTO dst when dst is a dir.
echo "==> Test 3: empty-dir over empty-dir rename overwrite"
mkdir testdata/mnt/src testdata/mnt/dst
echo "payload" > testdata/mnt/src/payload.txt
sys_rename testdata/mnt/src testdata/mnt/dst
# After rename: src is gone, dst has the payload.
if [ -d testdata/mnt/src ]; then
    fail "src should not exist after rename over dst"
fi
CONTENT=$(cat testdata/mnt/dst/payload.txt 2>/dev/null || echo "MISSING")
if [ "$CONTENT" != "payload" ]; then
    fail "expected 'payload' in dst/payload.txt, got '$CONTENT'"
fi
pass "empty-dir over empty-dir rename overwrite"

# Clean up from test 3.
rm -rf testdata/mnt/dst 2>/dev/null || true

# ── Test 4: dir-over-non-empty-dir must fail ─────────────────────
echo "==> Test 4: empty-dir over non-empty-dir must fail"
mkdir testdata/mnt/empty
mkdir testdata/mnt/full
echo "guard" > testdata/mnt/full/guard.txt
if sys_rename testdata/mnt/empty testdata/mnt/full; then
    fail "rename empty-dir over non-empty-dir should have failed"
fi
# Verify the non-empty dir is untouched.
CONTENT=$(cat testdata/mnt/full/guard.txt)
if [ "$CONTENT" != "guard" ]; then
    fail "non-empty target dir should be untouched, got '$CONTENT'"
fi
pass "non-empty dir target correctly rejected"

# ── Test 5: file→dir must fail (EISDIR) ─────────────────────────
echo "==> Test 5: file over dir must fail"
echo "f" > testdata/mnt/f.txt
mkdir testdata/mnt/d
if sys_rename testdata/mnt/f.txt testdata/mnt/d; then
    fail "rename file over existing dir should have failed"
fi
pass "file over existing dir correctly rejected"

# ── Test 6: dir→file must fail (ENOTDIR) ────────────────────────
echo "==> Test 6: dir over file must fail"
if sys_rename testdata/mnt/d testdata/mnt/f.txt; then
    fail "rename dir over existing file should have failed"
fi
pass "dir over existing file correctly rejected"

echo
echo "=============================="
echo " All rename tests passed."
echo "=============================="
