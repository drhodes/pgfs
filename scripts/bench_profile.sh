#!/usr/bin/env bash
# bench_profile.sh — drive the mounted filesystem under the CPU profiler.
#
# Builds pgfs with --features profiling, mounts it, and runs workload
# phases — each wrapped in a SIGUSR2 start/stop pair. On stop the daemon
# writes:
#   /tmp/pgfs-profile-{ts}.svg      flamegraph
#   /tmp/pgfs-profile-{ts}.stacks   readable stack dump
# On clean shutdown it also writes /tmp/pgfs-trace-{ts}.json (span trace).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
TESTDATA="$ROOT/testdata"
MOUNT="$TESTDATA/mnt"
LOG="$TESTDATA/bench-pgfs.log"
PIDFILE="$TESTDATA/bench.pid"
BIN="$ROOT/target/release/pgfs"

cleanup() {
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
        kill -TERM "$(cat "$PIDFILE")" 2>/dev/null || true
    fi
    sleep 1
    fusermount3 -u "$MOUNT" 2>/dev/null || fusermount3 -u -z "$MOUNT" 2>/dev/null || true
    rm -f "$PIDFILE"
}
trap cleanup EXIT

echo "==> building release + profiling"
cargo build --release --features profiling

echo "==> cleaning previous mount/daemon"
./scripts/pgfs.sh down >/dev/null 2>&1 || true
rm -f "$LOG"

echo "==> mounting profiling build"
mkdir -p "$MOUNT"
setsid nohup "$BIN" "$MOUNT" >>"$LOG" 2>&1 &
echo $! > "$PIDFILE"
for _ in $(seq 1 40); do
    grep -q " $MOUNT fuse" /proc/mounts 2>/dev/null && break
    if ! kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
        echo "daemon died during startup:" >&2
        tail -5 "$LOG" >&2
        exit 1
    fi
    sleep 0.25
done
grep -q " $MOUNT fuse" /proc/mounts || { echo "mount did not come up in time" >&2; exit 1; }

pid=$(cat "$PIDFILE")
sigusr2() { kill -SIGUSR2 "$pid"; }
sigusr2_start() { sigusr2; sleep 0.3; }
sigusr2_stop() { sigusr2; sleep 1.0; }

run_phase() {
    local name="$1"; shift
    echo "==> phase: $name"
    sigusr2_start
    "$@"
    sigusr2_stop
}

echo "==> baseline noise (2s idle sample)"
sigusr2_start; sleep 2; sigusr2_stop

# 16 MB written in 4096-byte writes: every write re-reads and re-writes the
# whole blob so far (read-modify-write amplification).
run_phase write-ampl \
    dd if=/dev/zero of="$MOUNT/write-ampl.bin" bs=4096 count=4096 oflag=append conv=notrunc

# Sequential read of that 16 MB file.
run_phase seq-read \
    dd if="$MOUNT/write-ampl.bin" of=/dev/null bs=1M

# 2000 tiny file creations: round-trip bound (getattr + create per file).
run_phase many-files \
    bash -c 'for i in $(seq 1 2000); do : > "$1/f$i.txt"; done' _ "$MOUNT"

echo "==> profiles:"
ls -lat /tmp/pgfs-profile-*.svg /tmp/pgfs-profile-*.stacks 2>/dev/null | head -8
echo "==> daemon log (profiler + metrics lines):"
grep -E "profiling|metrics\|" "$LOG" | tail -15 || true
echo "==> waiting for clean shutdown trace"
sleep 1
ls -lat /tmp/pgfs-trace-*.json 2>/dev/null | head -3 || true
