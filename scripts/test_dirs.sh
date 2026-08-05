#!/usr/bin/env bash
# test_dirs.sh — deep directory nesting, listing, and rmdir semantics.
#
# Patterned on the OpenZFS zfs-tests functional mkdir/rmdir tests (the
# "mkdir_001_pos"/"rmdir_001_pos" style: create, verify, remove in
# stages) and on xfstests' directory-traversal coverage. pgfs stores
# directories as rows whose `parent` column is the full path, so deep
# trees exercise the path-keyed lookup and the recursive parent rewrite
# that matters most.
set -euo pipefail
. ./scripts/test_lib.sh

trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"
require_mounted

# ── Test 1: mkdir -p builds an arbitrary-depth tree ──────────────────
echo "==> Test 1: mkdir -p deep tree"
mkdir -p "$MOUNT/a/b/c/d/e/f/g"
for p in a a/b a/b/c a/b/c/d a/b/c/d/e a/b/c/d/e/f a/b/c/d/e/f/g; do
    if [ ! -d "$MOUNT/$p" ]; then
        fail "expected $p to exist after mkdir -p"
    fi
done
pass "mkdir -p created 7 levels of nesting"

# ── Test 2: files deep in the tree are read/write/listable ───────────
echo "==> Test 2: deep file I/O"
echo "deep" > "$MOUNT/a/b/c/d/e/f/g/leaf.txt"
CONTENT=$(cat "$MOUNT/a/b/c/d/e/f/g/leaf.txt")
if [ "$CONTENT" != "deep" ]; then
    fail "deep file read-back mismatch: '$CONTENT'"
fi
LISTING=$(ls "$MOUNT/a/b/c/d/e/f/g")
if [ "$LISTING" != "leaf.txt" ]; then
    fail "expected listing 'leaf.txt', got '$LISTING'"
fi
pass "deep file round-trip works"

# ── Test 3: readdir lists a mixed directory correctly ────────────────
echo "==> Test 3: mixed listing at root"
mkdir -p "$MOUNT/alpha" "$MOUNT/beta" "$MOUNT/charlie"
echo "1" > "$MOUNT/rootfile.txt"
LISTING=$(ls "$MOUNT" | sort | tr '\n' ' ')
EXPECTED="a alpha beta charlie rootfile.txt "
if [ "$LISTING" != "$EXPECTED" ]; then
    fail "root listing mismatch: got '$LISTING' expected '$EXPECTED'"
fi
pass "mixed root listing sorted correctly"

# ── Test 4: rmdir rejects non-empty dirs (ENOTEMPTY) ─────────────────
echo "==> Test 4: rmdir on non-empty directory"
if rmdir "$MOUNT/a" 2>/dev/null; then
    fail "rmdir of non-empty 'a' should have failed"
fi
if [ ! -d "$MOUNT/a/b" ]; then
    fail "removal attempt must not touch descendants"
fi
pass "rmdir correctly rejects non-empty directory"

# ── Test 5: rmdir bottom-up succeeds once empty ──────────────────────
echo "==> Test 5: rmdir bottom-up"
rm "$MOUNT/a/b/c/d/e/f/g/leaf.txt"
for p in a/b/c/d/e/f/g a/b/c/d/e/f a/b/c/d/e a/b/c/d a/b/c a/b a; do
    rmdir "$MOUNT/$p" || fail "rmdir $p failed"
done
if [ -e "$MOUNT/a" ]; then
    fail "a should be gone after full bottom-up removal"
fi
pass "bottom-up rmdir of 7 levels succeeded"

# ── Test 6: rmdir on a missing dir fails (ENOENT) ────────────────────
echo "==> Test 6: rmdir on missing directory"
if rmdir "$MOUNT/does-not-exist" 2>/dev/null; then
    fail "rmdir of a missing directory should have failed"
fi
pass "rmdir correctly fails on missing directory"

echo
echo "=============================="
echo " All directory tests passed."
echo "=============================="
