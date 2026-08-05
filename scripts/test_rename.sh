#!/usr/bin/env bash
# test_rename.sh — integration test for rename-overwrite (POSIX semantics).
# Mounts pgfs, exercises rename via raw rename(2) syscall, verifies, cleans up.
set -euo pipefail
. ./scripts/test_lib.sh

trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"
require_mounted

# Helper: call the raw rename(2) syscall, bypassing mv's directory semantics.
# Usage: sys_rename <oldpath> <newpath>
sys_rename() {
    python3 -c "import os; os.rename('$1', '$2')"
}

# ── Test 1: file-over-file rename overwrite ──────────────────────
echo "==> Test 1: file-over-file rename overwrite"
echo "hello" > "$MOUNT/a.txt"
echo "world" > "$MOUNT/b.txt"
mv "$MOUNT/b.txt" "$MOUNT/a.txt"
CONTENT=$(cat "$MOUNT/a.txt")
if [ "$CONTENT" != "world" ]; then
    fail "expected 'world' in a.txt after overwrite, got '$CONTENT'"
fi
if [ -f "$MOUNT/b.txt" ]; then
    fail "b.txt should not exist after rename"
fi
pass "file-over-file rename overwrite"

# ── Test 2: rename to new name (no overwrite) ────────────────────
echo "==> Test 2: rename to new name"
echo "foo" > "$MOUNT/x.txt"
mv "$MOUNT/x.txt" "$MOUNT/y.txt"
if [ -f "$MOUNT/x.txt" ]; then
    fail "x.txt should not exist after rename"
fi
CONTENT=$(cat "$MOUNT/y.txt")
if [ "$CONTENT" != "foo" ]; then
    fail "expected 'foo' in y.txt, got '$CONTENT'"
fi
pass "rename to new name works"

# ── Test 3: empty-dir over empty-dir rename overwrite ────────────
# Uses raw rename(2) because mv moves src INTO dst when dst is a dir.
echo "==> Test 3: empty-dir over empty-dir rename overwrite"
mkdir "$MOUNT/src" "$MOUNT/dst"
echo "payload" > "$MOUNT/src/payload.txt"
sys_rename "$MOUNT/src" "$MOUNT/dst"
# After rename: src is gone, dst has the payload.
if [ -d "$MOUNT/src" ]; then
    fail "src should not exist after rename over dst"
fi
CONTENT=$(cat "$MOUNT/dst/payload.txt" 2>/dev/null || echo "MISSING")
if [ "$CONTENT" != "payload" ]; then
    fail "expected 'payload' in dst/payload.txt, got '$CONTENT'"
fi
pass "empty-dir over empty-dir rename overwrite"

# Clean up from test 3.
rm -rf "$MOUNT/dst" 2>/dev/null || true

# ── Test 4: dir-over-non-empty-dir must fail ─────────────────────
echo "==> Test 4: empty-dir over non-empty-dir must fail"
mkdir "$MOUNT/empty"
mkdir "$MOUNT/full"
echo "guard" > "$MOUNT/full/guard.txt"
if sys_rename "$MOUNT/empty" "$MOUNT/full"; then
    fail "rename empty-dir over non-empty-dir should have failed"
fi
# Verify the non-empty dir is untouched.
CONTENT=$(cat "$MOUNT/full/guard.txt")
if [ "$CONTENT" != "guard" ]; then
    fail "non-empty target dir should be untouched, got '$CONTENT'"
fi
pass "non-empty dir target correctly rejected"

# ── Test 5: file→dir must fail (EISDIR) ─────────────────────────
echo "==> Test 5: file over dir must fail"
echo "f" > "$MOUNT/f.txt"
mkdir "$MOUNT/d"
if sys_rename "$MOUNT/f.txt" "$MOUNT/d"; then
    fail "rename file over existing dir should have failed"
fi
pass "file over existing dir correctly rejected"

# ── Test 6: dir→file must fail (ENOTDIR) ────────────────────────
echo "==> Test 6: dir over file must fail"
if sys_rename "$MOUNT/d" "$MOUNT/f.txt"; then
    fail "rename dir over existing file should have failed"
fi
pass "dir over existing file correctly rejected"

echo
echo "=============================="
echo " All rename tests passed."
echo "=============================="
