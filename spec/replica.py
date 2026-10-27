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

from .err import PARTIAL, Feat, Req, md
from .optimize import ReplicaFreshnessCacheReq


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

    deps = [
        StartsDockerStandby,
        RequiresPrimaryTcpListener,
        AcceptsReplicaConnection,
        ServesReadsFromReplica,
        WritesStayOnPrimary,
        DetectsReplicationLag,
        ReplicaFreshnessCacheReq,
        FallsBackToPrimary,
        ReplicaObservability,
    ]

    def title(self):
        return "Replicated Storage"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "Write routing, lag detection, transparent fallback and the "
            "Docker standby work today. File contents (`read`) are still "
            "always served by the primary, and a standby that is unreachable "
            "when the daemon starts is dropped for the daemon's lifetime "
            "without a warning."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will give pgfs a read replica: a second
            PostgreSQL that follows the first by streaming its write-ahead
            log. We will mount pgfs with both, see metadata reads go to the
            replica, then take the replica away and see pgfs carry on
            without it.

            We need Docker, and the project-local cluster from [Your first
            pgfs mount](projectlocalenvironment.md).

            ## Step 1: start the standby

            ```bash
            ./scripts/pgfs.sh down
            ./scripts/replica_db.sh up
            ```

            The script copies the primary with `pg_basebackup`, starts a
            `postgres:17` container on port 5433, and waits until it is
            streaming:

            ```text
            ==> standby is in recovery (streaming)

            ready. replica connection string (for --replica):
              host=127.0.0.1 port=5433 dbname=pgfs user=alice
            ```

            If it reports that the container name `/pgfs-replica` is
            already in use, run `docker rm pgfs-replica` and repeat.

            ## Step 2: mount with the replica

            ```bash
            mkdir -p testdata/mnt
            ./target/debug/pgfs --replica "$(./scripts/replica_db.sh conn)" testdata/mnt \
                2> testdata/replica-demo.log &
            sleep 1
            grep 'replica mode' testdata/replica-demo.log
            ```

            ```text
            INFO pgfs: replica mode: reads served from standby (host=127.0.0.1 port=5433 dbname=pgfs user=alice) when fresh
            ```

            ## Step 3: write, wait, read

            ```bash
            mkdir testdata/mnt/replicated
            echo "on the primary" > testdata/mnt/replicated/a.txt
            sleep 1
            ls -l testdata/mnt/replicated
            ```

            Writes went to the primary. After a moment the standby has
            replayed them, and the `ls` (a directory listing plus
            attribute lookups) is answered by the standby. Ask the daemon:

            ```bash
            kill -USR1 "$(pgrep -f 'pgfs --replica')"; ls testdata/mnt > /dev/null
            grep 'state|ino_cache' testdata/replica-demo.log | tail -1
            ```

            ```text
            ... db=connected (replica fresh) ...
            ```

            ## Step 4: take the standby away

            ```bash
            ./scripts/replica_db.sh down
            ls -l testdata/mnt/replicated
            cat testdata/mnt/replicated/a.txt
            ```

            Both commands still work. The log records the switch once:

            ```text
            WARN pgfs::db: reads falling back to primary (standby stale or unreachable)
            ```

            ## Step 5: clean up

            ```bash
            kill -TERM "$(pgrep -f 'pgfs --replica')"
            rm -f testdata/replica-demo.log
            ```

            The standby's data stays in the Docker volume
            `pgfs-replica-data`; `./scripts/replica_db.sh wipe` deletes it.

            ## What we did

            We built a primary/standby pair, mounted pgfs across both, and
            saw reads move to the standby when it was current and back to
            the primary when it disappeared, with no error reaching the
            programs using the mount. The
            [explanation](../explanation/replicated_filesystem.md) covers
            the consistency rules behind this.
        """)

    def how_to(self):
        return md(r"""
            ## Provision the Docker standby

            ```bash
            ./scripts/replica_db.sh up
            ./scripts/replica_db.sh conn      # prints the --replica string
            ```

            `up` restarts the primary with a loopback TCP listener if it
            does not already have one.

            ## Mount with a replica

            ```bash
            ./target/debug/pgfs --replica "$(./scripts/replica_db.sh conn)" testdata/mnt
            ```

            `scripts/pgfs.sh` does not pass `--replica`; start the binary
            directly.

            ## Use a standby you manage yourself

            Any physical streaming standby of the primary's database
            works:

            ```bash
            pgfs --conn "host=primary.internal dbname=pgfs" \
                 --replica "host=standby.internal dbname=pgfs" /srv/pgfs
            ```

            pgfs never creates or alters the schema on the standby. It must
            be a physical copy of the primary.

            ## Check that the standby is healthy

            ```bash
            ./scripts/replica_db.sh status
            ```

            Look for `in recovery: t` and a small `replay_lag`. Then ask
            the daemon what it thinks:

            ```bash
            kill -USR1 <pid>; ls <mountpoint> > /dev/null
            grep 'state|ino_cache' <daemon log> | tail -1     # replica fresh | stale or unreachable | none
            ```

            ## Tell whether reads are actually reaching the standby

            Compare the counters in the minute-by-minute metrics line:

            ```bash
            grep 'metrics|replica' <daemon log> | tail -3
            ```

            `reads` growing means the standby is serving. `fallbacks`
            growing much faster means it is lagging or unreachable.

            ## Fix a standby that never catches up

            Symptom: `status` shows `in recovery: t`, the daemon reports
            `stale or unreachable`, and the container log contains
            `requested WAL segment ... has already been removed`:

            ```bash
            docker logs --tail 5 pgfs-replica
            ```

            The primary has recycled WAL the standby still needs, usually
            because the standby was down for a long time. Rebuild it from a
            fresh base backup:

            ```bash
            ./scripts/replica_db.sh wipe
            docker rm pgfs-replica 2>/dev/null    # if a stopped container remains
            ./scripts/replica_db.sh up
            ```

            ## Fix "container name /pgfs-replica is already in use"

            A stopped container from an earlier run remains, and `down` only
            removes running containers:

            ```bash
            docker rm pgfs-replica
            ./scripts/replica_db.sh up
            ```

            ## Attach a standby to a daemon that is already running

            Restart the daemon with `--replica`. The standby connection is
            made only at startup, and a standby that was unreachable then
            stays detached until the next restart.

            ## Remove the standby

            ```bash
            ./scripts/replica_db.sh down     # stop, keep data
            ./scripts/replica_db.sh wipe     # stop and delete the data volume
            ```
        """)

    def reference(self):
        return md(r"""
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
        """)

    def explanation(self):
        return md(r"""
            ## Why a physical standby

            PostgreSQL's physical streaming replication ships the primary's
            write-ahead log byte for byte, and the standby replays it. The
            standby is therefore an exact copy of the whole cluster: same
            tables, same rows, same schema, with no extra setup and no
            schema to keep in sync. That is why pgfs never initialises the
            schema on the standby. Nothing it could create would be
            different from what replication already delivers.

            ## Reads can go anywhere, decisions cannot

            A standby is always slightly behind. Serving a stale
            directory listing is a small, recoverable inaccuracy. Making a
            write decision from stale data is a correctness bug. If
            `create` checked a stale standby for an existing file, it could
            decide the name was free when it was not.

            So pgfs draws the line between answering and deciding. Pure
            metadata reads may use the standby. Every mutation, and every
            read whose result decides what a mutation does, goes to the
            primary. A lagging standby can therefore make the mount look
            briefly out of date, but it can never make it write the wrong
            thing.

            ## Measuring lag instead of assuming it

            Before using the standby, pgfs compares two positions in the
            write-ahead log: how far the primary has written, and how far
            the standby has replayed. If the standby has replayed at least
            as far, it holds everything the primary holds, and reading from
            it is indistinguishable from reading the primary.

            That check costs three small queries. Running it before every
            read would cost more than the reads it offloads, so the verdict
            is cached for half a second. This is the one place where pgfs
            trades strictness for speed. Inside that window, a read can see
            the standby's slightly older state.

            ## Failing towards availability

            When the standby is stale, unreachable or broken, pgfs reads
            from the primary and says so once per minute in the log. The
            programs using the mount never see an error. A replicated mount
            degrades into a single-node mount, which is still correct and
            only less scalable.

            ## What replication does not give you

            The standby offloads reads; it does not protect writes. If the
            primary fails, the mount fails. pgfs does not promote the
            standby or redirect writes, and a promoted standby is a new
            primary that pgfs would have to be restarted against. High
            availability for writes is a property of the PostgreSQL
            deployment underneath, not of pgfs.

            ## Why Docker, and why host networking

            The standby needs its own PostgreSQL instance with its own data
            directory and port. A container provides that without a second
            installation, and the named volume survives container restarts.
            Host networking lets the container reach the primary on
            loopback, so the primary never has to listen on a routable
            interface.
        """)
