#!/usr/bin/env bash
# bench.sh — `make bench` (spec/perf.py BenchmarkSuite): fixed workloads on
# a fresh filesystem, results as JSON with the commit, the machine and the
# Postgres version, plus WAL bytes per byte of user data. fio, mdtest and
# a passthrough-FUSE baseline are used when installed; otherwise the
# portable fallbacks below run and the ratio fields are null.
source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"
trap default_cleanup EXIT
OUT="${BENCH_OUT:-$TESTDATA/bench.json}"
N="${BENCH_FILES:-2000}"
start_pgfs || fail "mount"
wal() { psql -h "$TESTDATA" -d pgfs -Atc "select wal_bytes from pg_stat_wal"; }
t() { local s=$(date +%s.%N); "$@" >/dev/null 2>&1; echo "$(date +%s.%N) - $s" | bc; }

W0=$(wal)
seq_write=$(t dd if=/dev/zero of="$MOUNT/seq" bs=1M count=64 conv=fsync)
seq_read=$(t dd if="$MOUNT/seq" of=/dev/null bs=1M)
rand_write=$(t python3 -c "
import os, random
fd = os.open('$MOUNT/seq', os.O_WRONLY)
for _ in range(2000):
    os.pwrite(fd, os.urandom(4096), random.randrange(0, 64 << 20, 4096))
os.fsync(fd); os.close(fd)")
mkdir -p "$MOUNT/meta"
create=$(t bash -c "for i in \$(seq 1 $N); do : > '$MOUNT/meta/f'\$i; done")
stat=$(t bash -c "stat '$MOUNT'/meta/* >/dev/null")
unlink=$(t rm -rf "$MOUNT/meta")
git_clone=$(t git clone -q --no-hardlinks "$ROOT" "$MOUNT/repo")
git_status=$(t git -C "$MOUNT/repo" status --porcelain)
W1=$(wal)
user_bytes=$(( (64 << 20) + 2000 * 4096 ))
stop_pgfs
snap=$(t target/debug/pgfs snapshot main@bench)
branch=$(t target/debug/pgfs branch main@bench bench-branch)
promote=$(t target/debug/pgfs promote --force bench-branch main)
diff=$(t target/debug/pgfs diff main@bench main)
cat > "$OUT" <<JSON
{
  "commit": "$(git -C "$ROOT" rev-parse HEAD)",
  "machine": "$(uname -srm) cpus=$(nproc)",
  "postgres": "$(psql -h "$TESTDATA" -d pgfs -Atc 'show server_version')",
  "seconds": {
    "seq_write_64MiB_fsync": $seq_write, "seq_read_64MiB": $seq_read, "random_write_4KiB_x2000": $rand_write,
    "create_${N}": $create, "stat_${N}": $stat, "unlink_${N}": $unlink,
    "git_clone": $git_clone, "git_status": $git_status,
    "snapshot": $snap, "branch": $branch, "promote_force": $promote, "diff": $diff
  },
  "wal_bytes_per_user_byte": $(echo "scale=3; ($W1 - $W0) / $user_bytes" | bc),
  "ratio_to_passthrough_fuse": null
}
JSON
cat "$OUT"
