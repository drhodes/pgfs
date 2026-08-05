#!/usr/bin/env bash
# test_copy_performance.sh — End-to-end integration benchmark for file copy performance.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"

trap default_cleanup EXIT

echo "==> Building pgfs release binary"
cargo build --release --quiet

echo "==> Starting pgfs with full optimizations (--attr-ttl 1.0 --unlogged)"
ensure_db
fusermount3 -u -z "$MOUNT" 2>/dev/null || true
for _ in $(seq 1 40); do
    mount_active || break
    sleep 0.1
done
psql -h "$TESTDATA" -d pgfs -c "TRUNCATE entries, blocks" >/dev/null 2>&1 || true

mkdir -p "$MOUNT"
target/release/pgfs "$MOUNT" --attr-ttl 1.0 --unlogged &
PGFS_PID=$!
for _ in $(seq 1 30); do
    mount_active && break
    if ! kill -0 "$PGFS_PID" 2>/dev/null; then
        fail "pgfs daemon failed to start"
    fi
    sleep 0.1
done
require_mounted

echo "==> Preparing test datasets"
SRC_1M="$MOUNT/bench_src_1m.dat"
DST_1M="$MOUNT/bench_dst_1m.dat"
SRC_10M="$MOUNT/bench_src_10m.dat"
DST_10M="$MOUNT/bench_dst_10m.dat"
SRC_362M="$MOUNT/bench_src_362m.dat"
DST_362M="$MOUNT/bench_dst_362m.dat"

# Generate 1 MB deterministic data file
dd if=/dev/urandom of="$SRC_1M" bs=64k count=16 status=none
SRC_1M_HASH=$(sha256sum "$SRC_1M" | awk '{print $1}')

# Generate 10 MB deterministic data file
dd if=/dev/urandom of="$SRC_10M" bs=64k count=160 status=none
SRC_10M_HASH=$(sha256sum "$SRC_10M" | awk '{print $1}')

# Generate 362 MB deterministic data file
dd if=/dev/urandom of="$SRC_362M" bs=1M count=362 status=none
SRC_362M_HASH=$(sha256sum "$SRC_362M" | awk '{print $1}')


echo "==> Benchmarking 1 MB File Copy (cp)"
T0=$(date +%s%N)
cp "$SRC_1M" "$DST_1M"
T1=$(date +%s%N)
ELAPSED_1M_MS=$(( (T1 - T0) / 1000000 ))

DST_1M_HASH=$(sha256sum "$DST_1M" | awk '{print $1}')
if [ "$SRC_1M_HASH" != "$DST_1M_HASH" ]; then
    fail "Data corruption detected on 1MB copy!"
fi

echo "==> Benchmarking 10 MB File Copy (cp)"
T0=$(date +%s%N)
cp "$SRC_10M" "$DST_10M"
T1=$(date +%s%N)
ELAPSED_10M_MS=$(( (T1 - T0) / 1000000 ))

DST_10M_HASH=$(sha256sum "$DST_10M" | awk '{print $1}')
if [ "$SRC_10M_HASH" != "$DST_10M_HASH" ]; then
    fail "Data corruption detected on 10MB copy!"
fi

echo "==> Benchmarking 362 MB File Copy (cp)"
T0=$(date +%s%N)
cp "$SRC_362M" "$DST_362M"
T1=$(date +%s%N)
ELAPSED_362M_MS=$(( (T1 - T0) / 1000000 ))

DST_362M_HASH=$(sha256sum "$DST_362M" | awk '{print $1}')
if [ "$SRC_362M_HASH" != "$DST_362M_HASH" ]; then
    fail "Data corruption detected on 362MB copy!"
fi

# Calculate throughputs
THROUGHPUT_1M=$(awk -v ms="$ELAPSED_1M_MS" 'BEGIN { if (ms>0) printf "%.2f", (1.0 / (ms / 1000.0)); else print "N/A" }')
THROUGHPUT_10M=$(awk -v ms="$ELAPSED_10M_MS" 'BEGIN { if (ms>0) printf "%.2f", (10.0 / (ms / 1000.0)); else print "N/A" }')
THROUGHPUT_362M=$(awk -v ms="$ELAPSED_362M_MS" 'BEGIN { if (ms>0) printf "%.2f", (362.0 / (ms / 1000.0)); else print "N/A" }')

echo ""
echo "============================================================"
echo "                PGFS COPY PERFORMANCE REPORT                "
echo "============================================================"
echo "  1 MB File Copy Duration : ${ELAPSED_1M_MS} ms (${THROUGHPUT_1M} MB/s)"
echo " 10 MB File Copy Duration : ${ELAPSED_10M_MS} ms (${THROUGHPUT_10M} MB/s)"
echo "362 MB File Copy Duration : ${ELAPSED_362M_MS} ms (${THROUGHPUT_362M} MB/s)"
echo "  1 MB Data SHA256 Match  : VERIFIED"
echo " 10 MB Data SHA256 Match  : VERIFIED"
echo "362 MB Data SHA256 Match  : VERIFIED"
echo "============================================================"
echo ""

rm -f "$SRC_1M" "$DST_1M" "$SRC_10M" "$DST_10M" "$SRC_362M" "$DST_362M"

pass "File copy benchmark and integrity verification completed successfully!"
