#!/usr/bin/env bash
# test_truncate.sh — truncate/extend semantics, ported from xfstests
# generic/014 ("truncfile", FS QA Test No. 014).
#
# The original uses the C helper $here/src/truncfile to punch a file to
# a fixed size and check the result. This port keeps the same spirit —
# truncate a file, verify size AND content afterward — using standard
# tools (truncate, dd, stat, cmp) so it runs anywhere pgfs mounts.
set -euo pipefail
. ./scripts/test_lib.sh

trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"
require_mounted

# Helper: current size of a file in bytes.
size_of() { stat -c %s "$1"; }

# ── Test 1: truncate down keeps the leading bytes ────────────────────
echo "==> Test 1: truncate down preserves leading content"
echo "abcdefghijklmnopqrstuvwxyz0123456789" > "$MOUNT/t1.txt"
truncate -s 10 "$MOUNT/t1.txt"
SIZE=$(size_of "$MOUNT/t1.txt")
if [ "$SIZE" != "10" ]; then
    fail "expected size 10 after truncate down, got $SIZE"
fi
CONTENT=$(cat "$MOUNT/t1.txt")
if [ "$CONTENT" != "abcdefghij" ]; then
    fail "expected 'abcdefghij' after truncate down, got '$CONTENT'"
fi
pass "truncate down preserves leading content"

# ── Test 2: truncate up zero-fills the extension ─────────────────────
echo "==> Test 2: truncate up zero-fills"
truncate -s 20 "$MOUNT/t1.txt"
SIZE=$(size_of "$MOUNT/t1.txt")
if [ "$SIZE" != "20" ]; then
    fail "expected size 20 after truncate up, got $SIZE"
fi
# bytes 10..19 must be NUL
if dd if="$MOUNT/t1.txt" bs=1 skip=10 count=10 2>/dev/null | tr -d '\000' | grep -q .; then
    fail "extension beyond original content should be zero-filled"
fi
pass "truncate up zero-fills the extension"

# ── Test 3: truncate to zero empties the file ────────────────────────
echo "==> Test 3: truncate to zero"
truncate -s 0 "$MOUNT/t1.txt"
SIZE=$(size_of "$MOUNT/t1.txt")
if [ "$SIZE" != "0" ]; then
    fail "expected size 0 after truncate to zero, got $SIZE"
fi
if [ -s "$MOUNT/t1.txt" ]; then
    fail "file should be empty after truncate to zero"
fi
pass "truncate to zero empties the file"

# ── Test 4: truncate after append keeps appended bytes ───────────────
echo "==> Test 4: truncate after append"
printf 'AAAA' > "$MOUNT/t4.txt"
printf 'BBBB' >> "$MOUNT/t4.txt"
truncate -s 6 "$MOUNT/t4.txt"
CONTENT=$(cat "$MOUNT/t4.txt")
if [ "$CONTENT" != "AAAABB" ]; then
    fail "expected 'AAAABB' after truncate to 6, got '$CONTENT'"
fi
pass "truncate after append keeps leading bytes"

# ── Test 5: the generic/014 size sweep (10 KB file) ──────────────────
# generic/014 calls truncfile -c 10000: create a file, truncate it to
# exactly 10000 bytes. Reproduce that, then walk a range of sizes.
echo "==> Test 5: truncfile-style size sweep"
# Page-sized fill: pgfs does whole-blob read-modify-write per write(2),
# so byte-at-a-time fills would be O(n^2).
head -c 10000 /dev/urandom > "$MOUNT/t5.bin"
truncate -s 10000 "$MOUNT/t5.bin"
if [ "$(size_of "$MOUNT/t5.bin")" != "10000" ]; then
    fail "truncfile-style 10000-byte truncate failed"
fi
for n in 0 1 2 4 8 16 32 64 128 256 512 1024 2048 4096 8192 9999 10000 10001 16384; do
    truncate -s "$n" "$MOUNT/t5.bin"
    GOT=$(size_of "$MOUNT/t5.bin")
    if [ "$GOT" != "$n" ]; then
        fail "truncate to $n produced size $GOT"
    fi
done
pass "truncfile-style size sweep ($n sizes) all correct"

# ── Test 6: truncate preserves unrelated bytes (data integrity) ──────
echo "==> Test 6: truncate preserves unrelated bytes"
head -c 4096 /dev/urandom > "$MOUNT/t6.bin"
cp "$MOUNT/t6.bin" "$TESTDATA/t6.ref"
truncate -s 100 "$MOUNT/t6.bin"
truncate -s 4096 "$MOUNT/t6.bin"
# Bytes 0..99 are original; bytes 100..4095 are zero-filled now.
dd if="$MOUNT/t6.bin" bs=1 count=100 2>/dev/null > "$TESTDATA/t6.head"
dd if="$TESTDATA/t6.ref" bs=1 count=100 2>/dev/null > "$TESTDATA/t6.refhead"
if ! cmp -s "$TESTDATA/t6.head" "$TESTDATA/t6.refhead"; then
    fail "leading bytes corrupted after truncate down + up"
fi
if dd if="$MOUNT/t6.bin" bs=1 skip=100 count=100 2>/dev/null | tr -d '\000' | grep -q .; then
    fail "re-extended region should be zero-filled"
fi
pass "truncate preserves unrelated bytes"

rm -f "$TESTDATA/t6.ref" "$TESTDATA/t6.head" "$TESTDATA/t6.refhead"

echo
echo "=============================="
echo " All truncate tests passed."
echo "=============================="
