"""
Replicated-mode requirements for pgfs.

A pgfs mount can be backed by a primary Postgres plus physical
streaming standbys. Writes always go to the primary. Reads are served
from a standby only under an LSN ticket: the standby must have replayed
the mount's own latest commit (design-decisions.md, topic 9; CRAQ r06,
Aurora r19, FlightTracker r75).

These Requirement classes are granular and single-responsibility,
following the project's convention: each one is independently
footprintable, testable, and implementable.
"""

from .err import PARTIAL, Feat, Req, md
from .branch import (
    MountsSnapshots,
)
from .durability import (
    ScratchMode,
)
from .failure import (
    ReconnectsWithBackoff,
    RevalidatesAfterReconnect,
)
from .security import (
    RedactsSecrets,
)


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
    connection, all operations on the primary.

    It refuses `--replica` on a scratch filesystem, because unlogged
    tables are not replicated and the standby would serve empty tables
    (`durability.ScratchMode`, CAFault r80). It verifies the standby's
    identity before using it (`VerifiesStandbyIdentity`)."""

    deps = [ScratchMode]


class VerifiesStandbyIdentity(Req):
    """At connect, and after every reconnect, the daemon checks that the
    standby belongs to this cluster:

    - its system identifier (`pg_control_system()`) equals the
      primary's;
    - its timeline is the primary's or an ancestor of it;
    - it is in recovery;
    - its `meta.fs_uuid` equals the primary's.

    A standby that fails any check is not used, and the daemon logs a
    WARN naming the check. An LSN comparison alone cannot tell a standby
    of this cluster from a node of another one after a failover.

    **Sources.** Ceph r05, Raft r25."""

    deps = [AcceptsReplicaConnection]


class LsnTicketReads(Req):
    """After each commit, the daemon records a read ticket from a second
    autocommit statement pipelined right after the mutating function call
    (`SELECT pgfs.<op>(…); SELECT pg_current_wal_insert_lsn(),
    pg_current_wal_flush_lsn()`) in the same network round trip. It must
    run as a separate statement after the mutation's transaction commits,
    because an LSN read inside the transaction lies before the commit
    record, and a standby that has replayed only that far does not show
    the change yet. Tickets are tracked per inode (and on the parent
    `ino` for name mutations — both parents for `rename` — and on the
    mount for `statfs`), so a write to one file does not force reads of
    unrelated files to wait or fall back.

    A read may use the standby only when the standby's
    `pg_last_wal_replay_lsn()` is at or past the inode's ticket. Because
    Postgres's `walsender` streams WAL only up to the primary's flush
    LSN (`pg_current_wal_flush_lsn()`), an asynchronous commit
    (`durability.AsyncCommitWindow`) cannot reach the standby until
    `walwriter` (every `wal_writer_delay`, default 200 ms) or a
    synchronous commit flushes the primary's WAL. Therefore:

    - if the latest known `primary_flush_lsn < ticket`, the WAL has not
      even been flushed on the primary yet, so the daemon routes the
      read directly to the primary without waiting `--replica-wait`;
    - if `primary_flush_lsn ≥ ticket` and `standby_lsn < ticket`, the
      WAL is in flight to the standby and the daemon waits up to
      `--replica-wait` (default 20 ms) for the standby to catch up, then
      serves the read from the primary (`FallsBackToPrimary`).

    The standby's replay LSN (and the primary's flush LSN when polled) is
    polled at most once per millisecond and shared by all workers.

    **Postcondition.** Read-your-writes: no read through the mount
    misses an operation the mount completed, whichever node serves it.

    **Sources.** CRAQ r06, Aurora r19, TiDB r34, FlightTracker r75. This
    replaces a 500 ms cached freshness verdict, which could call a
    standby fresh while the daemon's own later write had not replayed."""

    deps = [VerifiesStandbyIdentity]


class SnapshotReadsFromAnyStandby(Req):
    """A snapshot mount (`branch.MountsSnapshots`) may read from any
    standby on which the snapshot's row in `snapshots(tree, name)` is
    visible, with no per-read ticket, because `pgfs.snapshot` commits
    while holding the exclusive layer gate and physical streaming
    replication replays WAL in commit order, so the snapshot row's
    presence on the standby guarantees that every transaction in the
    snapshot has replayed and nothing in the snapshot can change. With
    several `--replica` options, reads are spread across the standbys
    that qualify. The mount still maintains its read-only session in
    `mounts` on the primary (`consistency.ExclusiveTreeLease`), fails
    closed if renewal lapses (`consistency.FencesStaleWriters`), and
    re-checks on every reconnect that its `snapshots(tree, name)` row
    still exists (`failure.RevalidatesAfterReconnect`)."""

    deps = [LsnTicketReads, MountsSnapshots]


class FollowsFailover(Req):
    """With a multi-host `--conn` and `target_session_attrs=read-write`,
    a daemon whose primary fails reconnects to whichever host is the new
    primary (`failure.ReconnectsWithBackoff`), then revalidates
    (`failure.RevalidatesAfterReconnect`).

    **Durability.** An fsync survives the loss of the primary only if
    the cluster requires synchronous standbys
    (`synchronous_standby_names`). With asynchronous standbys, a
    failover can lose commits that were acknowledged, including fsyncs.
    The documentation states this, and the mount logs the cluster's
    setting at start."""

    deps = [ReconnectsWithBackoff, RevalidatesAfterReconnect]


class ServesReadsFromReplica(Req):
    """Read-only operations may be served from the standby under an LSN
    ticket (`LsnTicketReads`): `lookup`, `getattr`, `readdir` and
    `read`. Operations that drain buffers first
    (`consistency.DrainsBuffersBeforeExternalising`) use the standby
    only after the drain's commit has flushed on the primary and
    replayed on the standby (on an asynchronous mount, a read of an
    inode written within the last `wal_writer_delay` goes straight to
    the primary without waiting `--replica-wait`). Read routing is
    recorded in the metrics (`REPLICA_READ_COUNT`), and a DEBUG log line
    names the source used."""

    deps = [LsnTicketReads]


class WritesStayOnPrimary(Req):
    """Every mutating operation — `create`, `mkdir`, `write`, `unlink`,
    `rmdir`, and `rename` — executes on the primary client, never the
    replica. Internal consistency reads that a mutation depends on (e.g.
    rename's source-existence check) also read the primary directly so a
    stale replica can never corrupt a transaction's decision."""


class DetectsReplicationLag(Req):
    """Lag is measured against the daemon's own read ticket, not the
    primary's current position: the standby is usable for a read when
    its replay LSN is at or past the ticket (`LsnTicketReads`). Comparing
    against `pg_current_wal_lsn()` on a busy primary is almost never
    true, and reading the two positions with separate queries on two
    nodes is not atomic. A standby that is not in recovery is not a
    standby: it is not used, and the daemon logs a WARN. Any error
    querying the standby makes it unusable until the next successful
    poll. Lag is also reported in bytes and in seconds
    (`ReplicaObservability`)."""

    deps = [LsnTicketReads]


class FallsBackToPrimary(Req):
    """When the replica is absent, unreachable, fails its identity check,
    or has not reached the ticket within `--replica-wait`, reads are
    served from the primary with no error surfaced to the kernel. A
    replicated mount degrades to a single-node mount. The first fallback
    per period is logged at WARN, and every fallback increments
    `REPLICA_FALLBACK_COUNT`. Because of the ticket rule, a lagging
    replica never produces stale reads, only primary reads.

    If the standby is consistently slower than the primary for reads it
    could serve (p99 over 2 times the primary's for 5 minutes), the
    daemon stops routing to it and logs a WARN, until a probe shows it
    has recovered.

    **Sources.** IASO r92, CRUSH r03 (route on measured, not assumed,
    health)."""

    deps = [DetectsReplicationLag]


class ReplicaObservability(Req):
    """Replication state is observable:

    - the 60-second metrics export includes
      `metrics|replica|reads=… fallbacks=… lag_bytes=… lag_ms=… ticket_waits=…`;
    - the startup log announces replica mode, with the standby's host and
      the result of its identity check, and the connection string
      redacted (`security.RedactsSecrets`);
    - the SIGUSR1 state dump reports the replica's state.

    Daemon startup succeeds even when `--replica` points at an
    unreachable standby. That is logged at WARN, and the daemon retries
    the standby in the background with backoff, rather than dropping it
    for its lifetime."""

    deps = [RedactsSecrets]


# ── User-visible feature ──────────────────────────────────────────────

class ReplicatedFilesystem(Feat):
    """A pgfs mount can be backed by a primary Postgres and physical
    streaming standbys. Writes go to the primary. A read goes to a
    standby only when the standby has replayed the mount's own latest
    commit, so the mount always reads its own writes. Snapshots can be
    read from any standby that has them. A standby failure never causes
    a kernel-visible error. A standby is not protection against
    corruption: physical replication copies a damaged page faithfully,
    which is what checksums and scrub are for."""

    deps = [
        StartsDockerStandby,
        RequiresPrimaryTcpListener,
        AcceptsReplicaConnection,
        VerifiesStandbyIdentity,
        LsnTicketReads,
        ServesReadsFromReplica,
        SnapshotReadsFromAnyStandby,
        WritesStayOnPrimary,
        DetectsReplicationLag,
        FallsBackToPrimary,
        FollowsFailover,
        ReplicaObservability,
    ]

    def title(self):
        return "Replicated Storage"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "Reads of names, attributes, listings and file contents go to "
            "the standby under per-inode LSN tickets, the standby's "
            "identity is verified, a lagging or unreachable standby falls "
            "back to the primary with no visible error, multi-host "
            "connection strings follow failover, and replica metrics are "
            "exported. Not yet: demoting a standby that is consistently "
            "slower, reading the primary's flush LSN other than from the "
            "mount's own commits, and spreading snapshot reads over "
            "several `--replica` standbys (only the first is used)."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will give pgfs a read replica: a second PostgreSQL
            that follows the first by streaming its write-ahead log. We will mount
            pgfs with both, see reads served by the standby once it has replayed
            our writes, then take the standby away and see pgfs carry on.

            We need Docker, and the project-local cluster from [Your first pgfs
            mount](projectlocalenvironment.md).

            ## Step 1: start the standby

            ```bash
            ./scripts/pgfs.sh down
            ./scripts/replica_db.sh up
            ```

            The script copies the primary with `pg_basebackup`, starts a
            `postgres:17` container on port 5433, and waits until it streams.

            ## Step 2: mount with the replica

            ```bash
            mkdir -p testdata/mnt
            RUST_LOG=pgfs=debug ./target/debug/pgfs mount main testdata/mnt \
                --replica "$(./scripts/replica_db.sh conn)" 2> testdata/replica-demo.log &
            sleep 1
            grep 'replica mode' testdata/replica-demo.log
            ```

            ```text
            INFO pgfs::replica: replica mode: standby host=127.0.0.1 port=5433 dbname=pgfs user=alice verified
            ```

            The daemon checked that the standby has the primary's system
            identifier and `fs_uuid`, and that it is in recovery.

            ## Step 3: write, then read

            ```bash
            echo "on the primary" > testdata/mnt/a.txt
            sleep 1
            cat testdata/mnt/a.txt
            grep 'served by standby' testdata/replica-demo.log | tail -2
            ```

            The write went to the primary. The read went to the standby only after
            the standby had replayed past the write's LSN ticket.

            ## Step 4: take the standby away

            ```bash
            ./scripts/replica_db.sh down
            cat testdata/mnt/a.txt
            grep 'fall back' testdata/replica-demo.log | head -1
            ```

            The read still works; the first fallback is logged once.

            ## Step 5: clean up

            ```bash
            kill -TERM "$(pgrep -f 'pgfs mount main testdata/mnt')"
            ./scripts/replica_db.sh wipe
            ```

            ## What we did

            We mounted pgfs across a primary and a standby, saw reads move to the
            standby only when it had our writes, and back to the primary when it
            went away, with no error reaching programs. The
            [explanation](../explanation/replicated_filesystem.md) covers why.
        """)

    def how_to(self):
        return md(r"""
            ## Provision the Docker standby

            ```bash
            ./scripts/replica_db.sh up
            ./scripts/replica_db.sh conn      # prints the --replica string
            ```

            ## Mount with a replica

            ```bash
            pgfs mount main /srv/pgfs --replica "host=standby.internal dbname=pgfs sslmode=verify-full"
            ```

            The standby must be a physical copy of the primary; pgfs never writes
            to it. `--replica` is refused on a scratch filesystem, whose unlogged
            tables do not replicate.

            ## Check what the mount thinks of the standby

            ```bash
            kill -USR1 <pid>; grep 'state|replica' <daemon log> | tail -1
            grep 'metrics|replica' <daemon log> | tail -1
            ```

            `reads` counts standby reads, `fallbacks` reads that went to the
            primary, and `lag_bytes` / `lag_ms` the standby's distance.

            ## Follow a failover

            Give `--conn` every host and `target_session_attrs=read-write`:

            ```bash
            pgfs mount main /srv/pgfs --conn "host=pg1,pg2 dbname=pgfs target_session_attrs=read-write"
            ```

            After a failover the daemon reconnects to the new primary and
            revalidates (system identifier, timeline, the op_log continuity check)
            before serving.

            ## Fix a standby that never catches up

            If the container log says `requested WAL segment ... has already been
            removed`, the primary recycled WAL the standby still needed. Rebuild it:

            ```bash
            ./scripts/replica_db.sh wipe && ./scripts/replica_db.sh up
            ```
        """)

    def reference(self):
        return md(r"""
            ## Options

            | Option | Meaning |
            |---|---|
            | `--replica <libpq string>` | a physical streaming standby for reads; repeatable, the first is used |
            | `--replica-wait <ms>` | how long a read waits for the standby to reach its ticket (default 20) |

            ## Routing

            | Operation | Served by |
            |---|---|
            | `lookup`, `getattr`, `readdir`, `readdirplus`, `read`, `readlink`, xattr reads | the standby when its replay LSN is at or past the inode's ticket, else the primary |
            | every mutation, and every read inside a mutation | the primary |

            A ticket is the primary's WAL insert LSN, read in the same round trip
            right after the mutation commits, recorded on the inode (and the
            parents of name changes). When the primary's known flush LSN is below
            the ticket, the read goes straight to the primary.

            ## Identity checks, at connect and reconnect

            | Check | Must hold |
            |---|---|
            | system identifier | equals the primary's |
            | timeline | the primary's or an ancestor |
            | `pg_is_in_recovery()` | true |
            | `pgfs.meta.fs_uuid` | equals the primary's |

            ## Observability

            | Where | What |
            |---|---|
            | startup log | `replica mode: standby <redacted conn> verified`, or why it is not used |
            | `metrics\|replica\|…` | `reads`, `fallbacks`, `lag_bytes`, `lag_ms`, `ticket_waits` |
            | SIGUSR1 dump | `state\|replica=…` with usability, replay LSN and known primary flush LSN |

            ## `scripts/replica_db.sh`

            | Command | Effect |
            |---|---|
            | `up` | provision (if absent) and start the standby on 127.0.0.1:5433 |
            | `down` | stop the container, keep its data volume |
            | `wipe` | remove the container and its data volume |
            | `status` | recovery state and replay lag |
            | `conn` | print the `--replica` connection string |
        """)

    def explanation(self):
        return md(r"""
            ## Why a read ticket

            A standby replays the primary's WAL a little behind it. A mount that
            reads from a standby must never miss its own writes, so each mutation
            records the LSN of its commit, and a read of that inode may use the
            standby only once the standby has replayed that far. Comparing the
            standby with the primary's *current* position instead is almost never
            true on a busy primary, and two positions read by separate queries on
            two nodes are not comparable at one instant.

            Asynchronous commits complicate this: the WAL sender streams only up to
            the primary's flush position, so a commit not yet flushed cannot be on
            the standby however long one waits. The daemon therefore sends such
            reads straight to the primary instead of waiting.

            ## Why identity checks

            After a failover, an LSN alone cannot tell a standby of this cluster
            from a node of another one. Checking the system identifier, the
            timeline and `fs_uuid` makes sure the standby holds this filesystem's
            history.

            ## What a standby is not

            A standby is a copy, not protection against corruption: physical
            replication copies a damaged page faithfully. Checksums and
            `pgfs scrub` are for that. A standby failure never surfaces as an error
            in the mount; it only turns standby reads into primary reads.
        """)
