#!/usr/bin/env bash
# pgfs.sh — reliable lifecycle control for the pgfs FUSE mount.
#
# Everything this touches lives under the project's testdata/ dir: the
# Postgres cluster, the mount point, the daemon pid file and log. Nothing
# is written outside it.
#
#   ./scripts/pgfs.sh up      bring Postgres + the mount up, wait until it
#                             is really mounted, and report
#   ./scripts/pgfs.sh down    unmount cleanly and stop the daemon
#   ./scripts/pgfs.sh status  report mount/daemon/Postgres state
#   ./scripts/pgfs.sh run     run pgfs in the foreground (Ctrl+C to quit)
#
# Mounting uses fuser's AutoUnmount, so a pgfs daemon that dies for ANY
# reason (kill, crash, normal exit) has its mount removed by the kernel —
# there is never a "ghost" mount left behind to manually sweep.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
TESTDATA="$ROOT/testdata"
MOUNT="${PGFS_MOUNT:-$TESTDATA/mnt}"
PIDFILE="$TESTDATA/pgfs.pid"
LOG="$TESTDATA/pgfs.log"
PGDATA="$TESTDATA/pgdata"

mount_active() { grep -q " $MOUNT fuse" /proc/mounts 2>/dev/null; }
daemon_alive() { [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; }
db_running() { pg_ctl -D "$PGDATA" status >/dev/null 2>&1; }

ensure_db() {
    if db_running; then
        echo "postgres:  already running"
    else
        ./scripts/init_db.sh
    fi
}

start_daemon() {
    if daemon_alive; then
        echo "pgfs:      already running (pid $(cat "$PIDFILE"))"
        return 0
    fi
    rm -f "$PIDFILE"
    mkdir -p "$MOUNT"
    echo "pgfs:      starting (log: $LOG)"
    setsid nohup "$ROOT/target/debug/pgfs" "$MOUNT" >>"$LOG" 2>&1 &
    echo $! > "$PIDFILE"
}

wait_mounted() {
    for _ in $(seq 1 40); do
        mount_active && return 0
        if ! daemon_alive; then
            echo "pgfs:      daemon exited during startup; log tail:" >&2
            tail -5 "$LOG" >&2
            return 1
        fi
        sleep 0.25
    done
    return 1
}

cmd_up() {
    ensure_db
    echo "build:     cargo build"
    cargo build --quiet

    # A mount with no live daemon behind it is a stale ghost; sweep it so
    # the new mount can take over. AutoUnmount should make this impossible,
    # but this is the belt-and-braces path for pre-existing ghosts.
    if mount_active && ! daemon_alive; then
        echo "pgfs:      sweeping stale mount"
        fusermount3 -u -z "$MOUNT" || true
        sleep 0.5
    fi

    start_daemon
    if wait_mounted; then
        echo "mounted:   $MOUNT"
        echo "postgres:  $(psql -h "$TESTDATA" -d pgfs -Atc 'select version()' 2>/dev/null | cut -d' ' -f1-3)"
    else
        echo "pgfs:      mount did not come up in time" >&2
        return 1
    fi
}

cmd_down() {
    local pid=""
    [ -f "$PIDFILE" ] && pid="$(cat "$PIDFILE")"

    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "pgfs:      stopping (pid $pid)"
        kill -TERM "$pid"
        # Clean exit + AutoUnmount should remove the mount within a moment.
        for _ in $(seq 1 40); do
            mount_active || { rm -f "$PIDFILE"; echo "unmounted:  $MOUNT"; return 0; }
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.25
        done
        echo "pgfs:      daemon gone but mount persists; unmounting manually" >&2
    fi

    if mount_active; then
        if fusermount3 -u "$MOUNT" 2>/dev/null; then
            rm -f "$PIDFILE"
            echo "unmounted:  $MOUNT"
            return 0
        fi
        echo "pgfs:      mount busy; holders:" >&2
        fuser -vm "$MOUNT" 2>&1 || true
        echo "pgfs:      forcing lazy unmount" >&2
        fusermount3 -u -z "$MOUNT"
        rm -f "$PIDFILE"
        echo "unmounted:  $MOUNT (lazy)"
    else
        rm -f "$PIDFILE"
        echo "pgfs:      not mounted"
    fi
}

cmd_status() {
    echo "mount:     $(mount_active && echo 'active' || echo 'none')"
    echo "daemon:    $(daemon_alive && echo "running (pid $(cat "$PIDFILE"))" || echo 'not running')"
    echo "postgres:  $(db_running && echo 'running' || echo 'stopped')"
    if mount_active && ! daemon_alive; then
        echo "WARNING: stale mount present with no live daemon; run 'down' to sweep"
    fi
}

cmd_run() {
    ensure_db
    cargo build --quiet
    echo "running pgfs in foreground at $MOUNT (Ctrl+C to quit)"
    exec "$ROOT/target/debug/pgfs" "$MOUNT"
}

case "${1:-}" in
    up)      cmd_up ;;
    down)    cmd_down ;;
    status)  cmd_status ;;
    run)     cmd_run ;;
    *)
        echo "usage: $0 {up|down|status|run}" >&2
        exit 1
        ;;
esac
