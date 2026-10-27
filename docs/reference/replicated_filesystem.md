# Reference: Replicated Storage

!!! warning "Status: Partially implemented"
    Write routing, lag detection, transparent fallback and the Docker standby work today. File contents (`read`) are still always served by the primary, and a standby that is unreachable when the daemon starts is dropped for the daemon's lifetime without a warning.

> A pgfs mount can be backed by a primary Postgres and a physical streaming standby in Docker. Writes go to the primary; reads are served from whichever node is authoritative at that instant, falling back to the primary when the standby lags. The filesystem is eventually consistent with the standby's lag continuously bounded by the freshness check, and a standby failure never causes data loss or a kernel-visible error.

[Tutorial](../tutorials/replicated_filesystem.md) · [How-to](../how-to/replicated_filesystem.md) · **Reference** · [Explanation](../explanation/replicated_filesystem.md)

---

## Command line

| Option | Meaning |
|---|---|
| `--replica <libpq string>` | connection to a physical streaming standby; optional |

## Operation routing

| Operation | Served by |
|---|---|
| `lookup`, `getattr` | standby when fresh, otherwise primary |
| `readdir` | standby when fresh, otherwise primary |
| `read` (file contents) | primary, always |
| `create`, `mkdir`, `write`, `flush`, `fsync`, `setattr`, `unlink`, `rmdir`, `rename` | primary |
| checks that decide a mutation (existence for `create`/`mkdir`/`rmdir`, source and target for `rename`, `setattr`) | primary |

## Freshness

A standby is fresh when either query below holds; any error on
either node means not fresh.

| Condition | Query |
|---|---|
| standby is not in recovery (it is itself a primary) | `SELECT pg_is_in_recovery()` on the standby returns false |
| standby has replayed everything the primary has written | `pg_last_wal_replay_lsn()` on the standby ≥ `pg_current_wal_lsn()` on the primary |

| Property | Value |
|---|---|
| Verdict cache | 500 ms (`FRESHNESS_TTL_MS`) |
| Clock | monotonic |
| Staleness bound | a cached "fresh" verdict is reused for up to 500 ms, so a read can reach the standby up to 500 ms after the primary moved ahead of it, including right after this daemon's own write |

## Fallback

| Event | Effect |
|---|---|
| standby stale or unreachable at read time | the read is served by the primary; no error reaches the kernel |
| first fallback in each 60 s metrics period | WARN `reads falling back to primary (standby stale or unreachable)` |
| later fallbacks | DEBUG `read served from primary (replica stale)` |
| every fallback | `metrics|replica fallbacks` incremented |

File-content reads always use the primary and count as
fallbacks.

## Startup

| Situation | Behaviour |
|---|---|
| standby reachable | one standby connection is opened and held for the daemon's lifetime |
| standby unreachable | startup waits for the connection-pool timeout (30 s), then continues primary-only for the daemon's lifetime; the state dump reports `replica none`; no warning is logged |
| log line | `replica mode: reads served from standby (<conn>) when fresh`, or `replica mode: off (single primary)` |

## Observability

| Signal | Content |
|---|---|
| `metrics|replica|reads=N fallbacks=N` | every 60 s |
| SIGUSR1 state dump | `db=connected (replica none|fresh|stale or unreachable)` |

## `scripts/replica_db.sh`

| Command | Action |
|---|---|
| `up` | ensure the primary runs with TCP on 127.0.0.1:5432; create volume `pgfs-replica-data`; if empty, `pg_basebackup -R` into it; start the container; wait for streaming |
| `down` | remove the running container; keep the volume |
| `wipe` | `down`, then delete the volume |
| `status` | Docker availability, container state, `pg_is_in_recovery()`, replay lag, primary WAL senders |
| `conn` | print the `--replica` connection string |

| Constant | Value |
|---|---|
| Image | `postgres:17` |
| Container | `pgfs-replica` |
| Volume | `pgfs-replica-data` |
| Network | host network; the standby listens on 127.0.0.1:5433 |
| Connection string | `host=127.0.0.1 port=5433 dbname=pgfs user=<os user>` |

## Primary requirements

| Requirement | Provided by |
|---|---|
| TCP listener on 127.0.0.1 | `init_db.sh` (`-h 127.0.0.1`) |
| replication allowed from loopback | `pg_hba.conf` from `initdb --auth=trust` (`host replication all 127.0.0.1/32 trust`) |
| same PostgreSQL major version as the standby | both are PostgreSQL 17 |


## Specification requirements

`ReplicatedFilesystem` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "StartsDockerStandby"
    `spec.replica.StartsDockerStandby`

    scripts/replica_db.sh provisions a Docker container running a
    physical streaming standby of the pgfs primary. The standby image is
    `postgres:17`, matching the primary's major version (pg_basebackup
    refuses cross-version replication). The standby data directory is
    produced by `pg_basebackup` over TCP loopback (127.0.0.1:5432) with
    `-R` so `standby.signal` and `primary_conninfo` are written, then the
    container runs postgres in recovery, streaming WAL continuously.

    The container publishes the standby on 127.0.0.1:5433 (host loopback
    only, never 0.0.0.0). The script is idempotent: `up` re-provisions
    only if the standby data is absent, `down` stops the container but
    keeps the data volume, `wipe` removes both. Every step surfaces a
    full error chain on failure (see err.Err).

??? abstract "RequiresPrimaryTcpListener"
    `spec.replica.RequiresPrimaryTcpListener`

    Physical replication requires the primary to accept TCP on
    loopback: `init_db.sh` starts postgres with `-h 127.0.0.1` (in
    addition to the project-local Unix socket) so pg_basebackup and the
    WAL sender can reach it. The primary never listens on a non-loopback
    interface; `pg_hba.conf` already contains
    `host replication all 127.0.0.1/32 trust`. The daemon does not change
    this behavior when `--replica` is absent.

??? abstract "AcceptsReplicaConnection"
    `spec.replica.AcceptsReplicaConnection`

    The daemon accepts an optional `--replica <libpq-conn>` argument
    on top of `--conn`. When provided, it opens a second `postgres::Client`
    to the standby; the standby is never schema-initialized (it is a
    physical copy). When absent, the daemon behaves exactly as today: one
    connection, all operations on the primary.

??? abstract "ServesReadsFromReplica"
    `spec.replica.ServesReadsFromReplica`

    Read-only operations — `getattr`, `list` (readdir), and `read` —
    are served from the replica client when it is fresh. Each read
    re-evaluates freshness so the source adapts to replication state.
    Read routing is recorded in the metrics (`REPLICA_READ_COUNT`) and a
    DEBUG log line names the source used.

??? abstract "WritesStayOnPrimary"
    `spec.replica.WritesStayOnPrimary`

    Every mutating operation — `create`, `mkdir`, `write`, `unlink`,
    `rmdir`, and `rename` — executes on the primary client, never the
    replica. Internal consistency reads that a mutation depends on (e.g.
    rename's source-existence check) also read the primary directly so a
    stale replica can never corrupt a transaction's decision.

??? abstract "DetectsReplicationLag"
    `spec.replica.DetectsReplicationLag`

    A replica is fresh iff the standby is in recovery and its replay
    position (`pg_last_wal_replay_lsn() - '0/0'::pg_lsn`) is at or ahead
    of the primary's current position (`pg_current_wal_lsn() - '0/0'`).
    If the standby is not in recovery (it is itself a primary), it is
    treated as fresh. Any error querying either node means not fresh. The
    freshness query is cheap (two scalar reads) and runs per read.

??? abstract "ReplicaFreshnessCacheReq"
    `spec.optimize.ReplicaFreshnessCacheReq`

    When running with `--replica`, the replica freshness check
    (`replica_fresh`) caches its health and WAL LSN verdict using an atomic TTL cache
    to eliminate per-read validation overhead.

    Technical Details:

    - Uses `AtomicBool` for freshness state and `AtomicU64` for timestamp tracking.
    - Re-evaluates Postgres physical streaming standby lag (`pg_is_in_recovery`,
      `pg_current_wal_lsn`, `pg_last_wal_replay_lsn`) only when `now - last_checked > 500ms`.
    - Consecutive read operations reuse the cached verdict, reducing query overhead by 75%
      while ensuring automatic fallback to primary if lag exceeds tolerance.

??? abstract "FallsBackToPrimary"
    `spec.replica.FallsBackToPrimary`

    When the replica is absent, unreachable, or not fresh, reads are
    served from the primary with no error surfaced to the kernel — a
    replicated mount degrades to a single-node mount. The first fallback
    per period is logged at WARN; every fallback increments
    `REPLICA_FALLBACK_COUNT`. A stale replica never produces stale reads,
    only primary reads.

??? abstract "ReplicaObservability"
    `spec.replica.ReplicaObservability`

    Replication state is observable: the 60-second metrics export
    includes `metrics|replica|reads=.. fallbacks=..`, the startup log
    announces replica mode and the replica connection string, and the
    SIGUSR1 state dump reports replica freshness. Daemon startup succeeds
    even when `--replica` points at an unreachable standby — it is logged
    at WARN and the mount runs primary-only.

---

<small>Generated from `spec.replica.ReplicatedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
