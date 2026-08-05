"""
Replicated-mode requirements for pgfs.

A pgfs mount can be backed by a primary Postgres plus a physical
streaming standby running in Docker. Writes always go to the primary;
reads are served from the standby when it has caught up with the
primary's WAL, so a replicated mount is eventually consistent with a
bounded, continuously-measured lag and never silently diverges.

These Requirement classes are granular and single-responsibility,
following the project's convention: each one is independently
footprintable, testable, and implementable.
"""

from .err import Feat, Req


# ── Standby provisioning ──────────────────────────────────────────────

class StartsDockerStandby(Req):
    """scripts/replica_db.sh provisions a Docker container running a
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
    """


class RequiresPrimaryTcpListener(Req):
    """Physical replication requires the primary to accept TCP on
    loopback: `init_db.sh` starts postgres with `-h 127.0.0.1` (in
    addition to the project-local Unix socket) so pg_basebackup and the
    WAL sender can reach it. The primary never listens on a non-loopback
    interface; `pg_hba.conf` already contains
    `host replication all 127.0.0.1/32 trust`. The daemon does not change
    this behavior when `--replica` is absent."""


# ── Daemon replica connection ─────────────────────────────────────────

class AcceptsReplicaConnection(Req):
    """The daemon accepts an optional `--replica <libpq-conn>` argument
    on top of `--conn`. When provided, it opens a second `postgres::Client`
    to the standby; the standby is never schema-initialized (it is a
    physical copy). When absent, the daemon behaves exactly as today: one
    connection, all operations on the primary."""


class ServesReadsFromReplica(Req):
    """Read-only operations — `getattr`, `list` (readdir), and `read` —
    are served from the replica client when it is fresh. Each read
    re-evaluates freshness so the source adapts to replication state.
    Read routing is recorded in the metrics (`REPLICA_READ_COUNT`) and a
    DEBUG log line names the source used."""


class WritesStayOnPrimary(Req):
    """Every mutating operation — `create`, `mkdir`, `write`, `unlink`,
    `rmdir`, and `rename` — executes on the primary client, never the
    replica. Internal consistency reads that a mutation depends on (e.g.
    rename's source-existence check) also read the primary directly so a
    stale replica can never corrupt a transaction's decision."""


class DetectsReplicationLag(Req):
    """A replica is fresh iff the standby is in recovery and its replay
    position (`pg_last_wal_replay_lsn() - '0/0'::pg_lsn`) is at or ahead
    of the primary's current position (`pg_current_wal_lsn() - '0/0'`).
    If the standby is not in recovery (it is itself a primary), it is
    treated as fresh. Any error querying either node means not fresh. The
    freshness query is cheap (two scalar reads) and runs per read."""


class FallsBackToPrimary(Req):
    """When the replica is absent, unreachable, or not fresh, reads are
    served from the primary with no error surfaced to the kernel — a
    replicated mount degrades to a single-node mount. The first fallback
    per period is logged at WARN; every fallback increments
    `REPLICA_FALLBACK_COUNT`. A stale replica never produces stale reads,
    only primary reads."""


class ReplicaObservability(Req):
    """Replication state is observable: the 60-second metrics export
    includes `metrics|replica|reads=.. fallbacks=..`, the startup log
    announces replica mode and the replica connection string, and the
    SIGUSR1 state dump reports replica freshness. Daemon startup succeeds
    even when `--replica` points at an unreachable standby — it is logged
    at WARN and the mount runs primary-only."""


# ── User-visible feature ──────────────────────────────────────────────

class ReplicatedFilesystem(Feat):
    """A pgfs mount can be backed by a primary Postgres and a physical
    streaming standby in Docker. Writes go to the primary; reads are
    served from whichever node is authoritative at that instant, falling
    back to the primary when the standby lags. The filesystem is
    eventually consistent with the standby's lag continuously bounded by
    the freshness check, and a standby failure never causes data loss or
    a kernel-visible error."""
