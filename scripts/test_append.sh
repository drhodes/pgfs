#!/usr/bin/env bash
# test_append.sh — O_APPEND offset correctness, ported from the OpenZFS
# functional test tests/zfs-tests/tests/functional/append/file_append.ksh.
#
# That test uses a C helper (file_append) to open a file with O_APPEND,
# write 1-3 blocks, and verify via lseek that the write landed at the
# file offset the kernel reported — i.e. appends always go to EOF and
# never overwrite earlier data. This port reproduces the same strategy
# with dd oflag=append and size verification: after each append the file
# must be exactly (previous size + bytes written), and the appended
# block must be readable back at that offset.
set -euo pipefail
. ./scripts/test_lib.sh

trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"
require_mounted

BS=131072   # same block size as the ZFS test
ITERATIONS=5

# Helper: write $2 blocks of $BS bytes (pattern repeats block index)
# with O_APPEND, then assert the file size equals $3.
append_blocks() {
    local file="$1" nblocks="$2" expected="$3" i
    for i in $(seq 1 "$nblocks"); do
        # dd with oflag=append opens with O_APPEND: the kernel passes
        # offset=EOF to the FUSE write, exactly like the ZFS helper.
        dd if=/dev/zero bs="$BS" count=1 of="$file" oflag=append conv=notrunc \
            status=none
    done
    local got
    got=$(stat -c %s "$file")
    if [ "$got" != "$expected" ]; then
        fail "after appending $nblocks block(s): expected size $expected, got $got"
    fi
}

# ── Test 1: buffered O_APPEND chain (the ZFS strategy verbatim) ──────
echo "==> Test 1: O_APPEND writes always land at EOF"
FILE="$MOUNT/append_file.bin"
rm -f "$FILE"
expected=0
for i in $(seq 1 "$ITERATIONS"); do
    # random number of blocks 1..3, like random_int_between 1 3
    nblocks=$(( (RANDOM % 3) + 1 ))
    expected=$(( expected + BS * nblocks ))
    append_blocks "$FILE" "$nblocks" "$expected"
done
pass "O_APPEND chain of $ITERATIONS appends stays at EOF ($expected bytes total)"

# ── Test 2: appends do not overwrite earlier data ────────────────────
echo "==> Test 2: appends never clobber earlier bytes"
rm -f "$FILE"
printf 'FIRST' > "$FILE"
printf 'SECOND' >> "$FILE"
CONTENT=$(cat "$FILE")
if [ "$CONTENT" != "FIRSTSECOND" ]; then
    fail "expected 'FIRSTSECOND', got '$CONTENT'"
fi
# interleave a write with an append: the write must not move the append point
printf 'X' > "$FILE"
printf 'Y' >> "$FILE"
CONTENT=$(cat "$FILE")
if [ "$CONTENT" != "XY" ]; then
    fail "expected 'XY', got '$CONTENT'"
fi
pass "appends never clobber earlier bytes"

# ── Test 3: appended data is readable back at the right offset ───────
echo "==> Test 3: appended block readable at its offset"
rm -f "$FILE"
printf '0123456789' > "$FILE"        # 10 bytes
dd if=/dev/zero bs=1 count=10 of="$FILE" oflag=append conv=notrunc status=none
# Bytes 10..19 are the appended zero block; there must be no non-zero
# bytes in that range (tr -d '\000' removes NULs, so wc -c counts the
# non-zero leftovers).
TAIL=$(dd if="$FILE" bs=1 skip=10 count=10 2>/dev/null | tr -d '\000' | wc -c)
if [ "$TAIL" -ne 0 ]; then
    fail "expected zero-filled append block after offset 10, found $TAIL non-zero bytes"
fi
HEAD=$(dd if="$FILE" bs=1 count=10 2>/dev/null)
if [ "$HEAD" != "0123456789" ]; then
    fail "leading bytes corrupted after append, got '$HEAD'"
fi
pass "appended block readable at the correct offset"

echo
echo "=============================="
echo " All append tests passed."
echo "=============================="
