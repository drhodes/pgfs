#!/usr/bin/env bash
# replica_db.sh — run a physical streaming standby of the pgfs primary in
# Docker (see spec/replica.py).
#
#   up       provision: primary on TCP loopback → pg_basebackup into a
#            named volume → start a postgres:17 container as a standby on
#            127.0.0.1:5433, streaming WAL from the primary
#   down     stop and remove the standby container (keeps the data volume)
#   wipe     down + delete the data volume (full clean slate)
#   status   report container / recovery / replay-lag state
#   conn     print the --replica connection string
#
# The standby uses `--network host` so it shares the host network
# namespace: it can reach the primary at 127.0.0.1:5432 (loopback-only,
# per init_db.sh) and binds its own postgres on 127.0.0.1:5433 without
# colliding with the primary's 5432. Container-internal ownership is
# fixed up to the postgres uid (999) after pg_basebackup.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
TESTDATA="$ROOT/testdata"
PGDATA="$TESTDATA/pgdata"
SOCKDIR="$TESTDATA"
LOGFILE="$TESTDATA/postgres.log"

IMAGE="postgres:17"
CONTAINER="pgfs-replica"
VOLUME="pgfs-replica-data"
PRIMARY_HOST="127.0.0.1"
PRIMARY_PORT="5432"
REPLICA_PORT="5433"
PGUSER="$(whoami)"          # initdb creates a superuser named after the OS user
REPLICA_CONN="host=127.0.0.1 port=$REPLICA_PORT dbname=pgfs user=$PGUSER"

docker_ok() { command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; }
container_running() { docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; }
primary_tcp_up() { psql -h "$PRIMARY_HOST" -p "$PRIMARY_PORT" -d postgres -c '\q' >/dev/null 2>&1; }

ensure_db() {
    if ! pg_ctl -D "$PGDATA" status >/dev/null 2>&1; then
        echo "==> primary not running; starting it"
        ./scripts/init_db.sh >/dev/null 2>&1
    fi
}

# The primary must accept TCP on loopback for pg_basebackup + WAL
# streaming. If it was started before loopback was enabled, restart it.
ensure_primary_tcp() {
    if primary_tcp_up; then
        return 0
    fi
    echo "==> primary not reachable on 127.0.0.1:$PRIMARY_PORT; restarting with TCP"
    pg_ctl -D "$PGDATA" restart -m fast -l "$LOGFILE" \
        -o "-k $SOCKDIR -h 127.0.0.1" >/dev/null
    for _ in $(seq 1 20); do
        primary_tcp_up && return 0
        sleep 0.25
    done
    echo "error: primary did not come up on TCP" >&2
    exit 1
}

volume_has_data() {
    docker run --rm \
        -v "$VOLUME:/probe" \
        --entrypoint sh \
        "$IMAGE" -c '[ -s /probe/PG_VERSION ]' >/dev/null 2>&1
}

provision_standby_data() {
    echo "==> pg_basebackup from $PRIMARY_HOST:$PRIMARY_PORT into volume '$VOLUME'"
    # -R writes standby.signal + primary_conninfo so the container starts
    # in recovery and streams WAL. --network host lets the container see
    # the primary on the host loopback.
    #
    # The volume is cleared on failure: pg_basebackup writes PG_VERSION
    # early, so a wedged partial backup would otherwise look provisioned
    # and every future `up` would skip provisioning.
    if ! docker run --rm --network host \
        -v "$VOLUME:/backup" \
        --entrypoint sh \
        "$IMAGE" -c "
            rm -rf /backup/* &&
            pg_basebackup -h $PRIMARY_HOST -p $PRIMARY_PORT -U '$PGUSER' \
                -D /backup -R -X stream -c fast -P &&
            chown -R 999:999 /backup
        "; then
        docker run --rm -v "$VOLUME:/backup" --entrypoint sh \
            "$IMAGE" -c 'rm -rf /backup/*' >/dev/null 2>&1 || true
        echo "error: pg_basebackup failed; standby data volume cleared" >&2
        echo "      run '$0 up' again to retry" >&2
        exit 1
    fi
    echo "==> standby data provisioned"
}

start_standby() {
    if container_running; then
        echo "==> standby container already running"
        return 0
    fi
    echo "==> starting standby container on 127.0.0.1:$REPLICA_PORT"
    docker run -d \
        --name "$CONTAINER" \
        --network host \
        -e PGDATA=/var/lib/postgresql/data \
        -v "$VOLUME:/var/lib/postgresql/data" \
        "$IMAGE" -c port="$REPLICA_PORT"
}

wait_streaming() {
    echo "==> waiting for standby to enter recovery"
    local ok="" rec=""
    for _ in $(seq 1 40); do
        rec=$(psql -h "$PRIMARY_HOST" -p "$REPLICA_PORT" -U "$PGUSER" -d pgfs \
            -Atc 'SELECT pg_is_in_recovery()' 2>/dev/null || true)
        if [ "$rec" = "t" ]; then
            ok=1
            break
        fi
        sleep 0.5
    done
    if [ -z "$ok" ]; then
        echo "error: standby did not enter recovery" >&2
        docker logs --tail 20 "$CONTAINER" >&2 || true
        exit 1
    fi
    echo "==> standby is in recovery (streaming)"
}

cmd_up() {
    ensure_db
    ensure_primary_tcp
    if ! docker_ok; then
        echo "error: docker is not available (or the daemon is not running)" >&2
        exit 1
    fi
    docker volume create "$VOLUME" >/dev/null
    if ! volume_has_data; then
        provision_standby_data
    else
        echo "==> standby data already present in volume '$VOLUME'"
    fi
    start_standby
    wait_streaming
    echo
    echo "ready. replica connection string (for --replica):"
    echo "  $REPLICA_CONN"
}

cmd_down() {
    if container_running; then
        echo "==> stopping standby container (data volume kept)"
        docker rm -f "$CONTAINER" >/dev/null
    else
        echo "==> standby container not running"
    fi
}

cmd_wipe() {
    cmd_down
    echo "==> removing data volume '$VOLUME'"
    docker volume rm -f "$VOLUME" >/dev/null 2>&1 || true
}

cmd_status() {
    echo "docker:       $(docker_ok && echo available || echo 'NOT available')"
    echo "container:    $(container_running && docker ps --filter name="$CONTAINER" --format '{{.Status}}' || echo 'not running')"
    if container_running; then
        local rec
        rec=$(psql -h "$PRIMARY_HOST" -p "$REPLICA_PORT" -U "$PGUSER" -d pgfs -Atc 'SELECT pg_is_in_recovery()' 2>/dev/null || echo '?')
        echo "in recovery:  $rec"
        psql -h "$PRIMARY_HOST" -p "$REPLICA_PORT" -U "$PGUSER" -d pgfs \
            -Atc "SELECT 'replay_lag=' || COALESCE(EXTRACT(EPOCH FROM (now() - pg_last_xact_replay_timestamp()))::text, 'n/a') || 's'" 2>/dev/null \
            | sed 's/^/              /'
    fi
    echo "primary tcp:  $(primary_tcp_up && echo "listening on $PRIMARY_HOST:$PRIMARY_PORT" || echo 'NOT listening')"
    psql -h "$SOCKDIR" -d postgres -Atc \
        "SELECT 'wal_senders: ' || COALESCE(string_agg(application_name || '=' || state, ', '), 'none') FROM pg_stat_replication" 2>/dev/null \
        | sed 's/^/              /'
}

cmd_conn() {
    echo "$REPLICA_CONN"
}

case "${1:-}" in
    up)     cmd_up ;;
    down)   cmd_down ;;
    wipe)   cmd_wipe ;;
    status) cmd_status ;;
    conn)   cmd_conn ;;
    *)
        echo "usage: $0 {up|down|status|wipe|conn}" >&2
        exit 1
        ;;
esac
