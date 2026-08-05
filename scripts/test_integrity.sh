#!/usr/bin/env bash
# test_integrity.sh — create/write/unlink chains, ported from xfstests
# generic/001 ("Random file copier", FS QA Test No. 001).
#
# The original builds chains of copies (foo, foo.0, foo.1, ... foo.N),
# renames the tail of each chain to foo.last, then diffs head vs tail at
# the end of every iteration to catch data corruption across creat,
# write, and unlink for a variety of file sizes and directory depths.
# This port keeps the same operations — fill, chain, check, unlink —
# with plain tools (dd, cp, mv, rm, cmp) instead of the C fill helper.
set -euo pipefail
. ./scripts/test_lib.sh

trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"
require_mounted

WORK="$MOUNT/qa001.$$"      # per-run namespace dir (like $TEST_DIR/$$)
mkdir -p "$WORK/sub/deep"

# ── 1. Fill: create files of many sizes, some nested ─────────────────
echo "==> Fill: creating files of varied sizes"
# (name, bytes) pairs mirroring the generic/001 default config scale.
# Written in page-sized chunks: pgfs does whole-blob read-modify-write
# per write(2), so byte-at-a-time fills would be O(n^2) here.
fill_file() {
    head -c "$2" /dev/urandom > "$1"
}
fill_file "$WORK/small" 10
fill_file "$WORK/big" 102400
fill_file "$WORK/sub/small" 10
fill_file "$WORK/sub/big" 102400
fill_file "$WORK/sub/a" 1
fill_file "$WORK/sub/b" 2
fill_file "$WORK/sub/c" 4
fill_file "$WORK/sub/d" 8
fill_file "$WORK/sub/e" 16
fill_file "$WORK/sub/f" 32
fill_file "$WORK/sub/g" 64
fill_file "$WORK/sub/h" 128
fill_file "$WORK/sub/i" 256
fill_file "$WORK/sub/j" 512
fill_file "$WORK/sub/k" 1024
fill_file "$WORK/sub/l" 2048
fill_file "$WORK/sub/m" 4096
fill_file "$WORK/sub/n" 8192
fill_file "$WORK/sub/deep/x" 1000
fill_file "$WORK/sub/deep/y" 16000
pass "filled $(find "$WORK" -type f | wc -l) files"

# ── 2. Chain: copy each file into a chain and move the tail ──────────
echo "==> Chain: building copy chains"
chain_file() {
    cp "$1" "$1.0"
    cp "$1.0" "$1.1"
    mv "$1.1" "$1.last"   # tail of the chain (generic/001 renames foo.N)
    rm -f "$1.0"          # unlink the intermediates
}
for f in "$WORK"/small "$WORK"/big "$WORK"/sub/small "$WORK"/sub/big \
         "$WORK"/sub/a "$WORK"/sub/m "$WORK"/sub/n "$WORK"/sub/deep/x \
         "$WORK"/sub/deep/y; do
    chain_file "$f"
done
pass "chained $(find "$WORK" -name '*.last' | wc -l) files"

# ── 3. Check: head vs tail must be byte-identical ────────────────────
echo "==> Check: diffing head vs tail"
for f in "$WORK"/small "$WORK"/big "$WORK"/sub/small "$WORK"/sub/big \
         "$WORK"/sub/a "$WORK"/sub/m "$WORK"/sub/n "$WORK"/sub/deep/x \
         "$WORK"/sub/deep/y; do
    if [ ! -f "$f" ]; then
        fail "$f vanished!"
    fi
    if [ ! -f "$f.last" ]; then
        fail "$f.last missing!"
    fi
    if ! cmp -s "$f" "$f.last"; then
        fail "data corruption: $f differs from $f.last"
    fi
done
pass "all head/tail pairs byte-identical"

# ── 4. Verify nested dirs survived the writes ────────────────────────
echo "==> Verify: nested structure intact"
for p in "$WORK" "$WORK/sub" "$WORK/sub/deep"; do
    if [ ! -d "$p" ]; then
        fail "directory $p vanished"
    fi
done
pass "nested directory structure intact"

# ── 5. Unlink: remove every file, verify empty ───────────────────────
echo "==> Unlink: removing everything"
# Delete files (not the dirs) at every depth, then rmdir bottom-up.
find "$WORK" -type f -delete
if [ -n "$(find "$WORK" -type f)" ]; then
    fail "files remain after unlink"
fi
rmdir "$WORK/sub/deep" "$WORK/sub" "$WORK"
pass "all files unlinked, directories removed"

echo
echo "=============================="
echo " All integrity tests passed."
echo "=============================="
