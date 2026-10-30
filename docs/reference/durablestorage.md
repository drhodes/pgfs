# Reference: Durability and Crash Safety

!!! success "Status: Stable"
    fsync, fdatasync and `O_SYNC` writes commit synchronously and confirm replication with `pgfs.sync_barrier`; other operations commit asynchronously, renames commit pending writes first, close reports errors through `errseq_t`, shutdown flushes and barriers, the server's durability settings are checked, and scratch filesystems reset after a crash. `test_crash.sh` kills the daemon and Postgres; the LazyFS crash matrix is part of `verify.CrashConformance`.

> pgfs keeps the POSIX promise: data is durable when fsync returns. Everything else is fast and becomes durable in order, so after any crash the filesystem is an earlier moment of its history, and write-then-rename is atomic even without fsync.

[Tutorial](../tutorials/durablestorage.md) · [How-to](../how-to/durablestorage.md) · **Reference** · [Explanation](../explanation/durablestorage.md)

---

## Durability points

| Operation | Durable when |
|---|---|
| `fsync`, `fdatasync`, `fsyncdir` | on return |
| `write` with `O_SYNC` / `O_DSYNC` | on return |
| snapshot, branch, promote, tree delete, lease acquisition and takeover | on return |
| clean shutdown (`SIGTERM`, `SIGINT`, `SIGHUP`) | on completion (all prior operations) |
| other metadata operations | within the commit window (default 600 ms) |
| `write` on an ordinary handle | after the inode's buffer is committed through `write_seq`, then within the commit window |
| anything in scratch mode | never |

## What forces a commit of buffered writes

`flush`, `close`, `fsync`, rename or link or unlink of the
file, snapshot, branch, buffer limits, shutdown.

## Server settings checked at mount

| Setting | Required |
|---|---|
| `fsync` | `on` |
| `full_page_writes` | `on` |
| `data_checksums` | `on` recommended (warning) |

## Options

| Option | Default | Effect |
|---|---|---|
| `--shutdown-timeout` | 30 s | how long shutdown waits to commit buffers |
| `--i-accept-data-loss` | off | mount despite unsafe server settings |
| `mkfs --scratch` | off | unlogged tables; no durability |


## Specification requirements

`DurableStorage` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "DurabilityPoints"
    `spec.durability.DurabilityPoints`

    Each operation has one stated durability point. The event in the
    right-hand column is the moment after which the effect survives a
    crash of the daemon, the host or Postgres:

    | Operation | Durable when |
    |---|---|
    | `fsync`, `fdatasync`, `fsyncdir` | the call returns successfully |
    | `write` on an `O_SYNC` or `O_DSYNC` handle | the call returns successfully |
    | snapshot, branch, promote, tree delete, lease acquisition and takeover | the call or command returns successfully |
    | any other metadata operation (create, mkdir, rename, unlink, setattr, link, …) | its asynchronous commit is flushed: within `AsyncCommitWindow` of the reply |
    | `write` on an ordinary handle | the inode's buffer is committed through the write's `write_seq` (`flush`, `close`, `fsync`, a metadata operation on the file, or buffer pressure), then flushed as above |
    | everything, in scratch mode | never (`ScratchMode`) |

    A successful `write` does not mean the data is in the database. A
    daemon crash loses writes that were acknowledged but not committed.
    The documentation says so plainly (Ananke r46).

    **Sources.** r123, r56, r129, r46.

??? abstract "FsyncIsDurable"
    `spec.durability.FsyncIsDurable`

    `fsync` and `fdatasync` commit the inode's pending write buffer
    (`optimize.BufferedWriteFlushReq`) in one transaction run with
    `SET LOCAL synchronous_commit = on`, and reply only after that commit
    returns. `fsyncdir` does the same for the directory's metadata. When
    the cluster has synchronous standbys, that commit also waits for
    them, so the cluster's policy applies without pgfs adding its own.

    In PostgreSQL, a query cancel (`PQcancel` or `pg_cancel_backend`)
    that lands during a synchronous-replication wait
    (`SyncRepWaitForLSN`) does not abort `COMMIT`: Postgres commits
    locally and returns success with a `WARNING` `NoticeResponse`
    (`routine = "SyncRepWaitForLSN"`, SQLSTATE `01000`). Each worker
    connection hooks its notice callback. If the daemon itself sent a
    cancel (`failure.InFlightAgeWatchdog`, `failure.HonoursInterrupt`) or
    the notice callback observed a `SyncRepWaitForLSN` warning on any
    synchronous operation (`fsync`, `fdatasync`, `fsyncdir`, `O_SYNC` or
    `O_DSYNC` write, `snapshot`, `branch`, `promote`), the commit is
    treated as not yet replicated: the daemon runs the synchronous
    barrier (`FsyncForcesEarlierCommits`) before replying, and if the
    barrier fails or is itself cancelled, replies EIO (or EINTR if
    interrupted) and records the failure (`FsyncFailureSemantics`). CLI
    commands that commit synchronously without a mount follow the same
    rule using their own barrier.

    **Postcondition.** Every write and metadata change to the inode that
    completed before the call survives any later crash.

    **Durability.** The commit record is flushed to the primary's disk,
    and to the synchronous standbys that `synchronous_standby_names`
    requires.

    **Errors.** EIO if the commit or synchronous replication wait fails
    (`FsyncFailureSemantics`), or EINTR if interrupted before durability
    is confirmed. EROFS never: a read-only view has nothing to sync, and
    fsync succeeds.

    **Sources.** CrashMonkey r123 (bug class B3: fsync that does not
    persist).

??? abstract "FsyncForcesEarlierCommits"
    `spec.durability.FsyncForcesEarlierCommits`

    An `fsync` or `fsyncdir` that finds nothing pending in the inode's
    write buffer still waits for durability: earlier asynchronous commits
    (from `close`, `mkdir`, `rename`, or a previous `fsync` whose
    synchronous-replication wait was cancelled) may not have been flushed
    or replicated yet. Because PostgreSQL's `RecordTransactionCommit`
    flushes WAL and waits in `SyncRepWaitForLSN` only when a committing
    transaction has an `xid` assigned and wrote WAL, an empty transaction
    with `synchronous_commit = on` is a no-op.

    Instead, `pgfs.sync_barrier(mount_id, worker_id, op_seq)` takes the
    worker's next `op_seq` and performs an in-place (HOT) `UPDATE` of the
    worker's pre-allocated `op_log` slot (`failure.IdempotentMutations`)
    in a transaction with `SET LOCAL synchronous_commit = on` (never
    `pg_logical_emit_message`, whose `EXECUTE` privilege is revoked from
    `PUBLIC` by default, `security.LeastPrivilegeRoles`). A WAL flush and
    standby replay are cumulative, so one completed synchronous barrier
    proves that every earlier commit was flushed to disk and replicated
    to synchronous standbys. The same `pgfs.sync_barrier` is used by
    `fsync` and `fsyncdir` with nothing buffered, clean shutdown
    (`FlushesOnShutdown`), and recovering an ambiguous or locally
    committed synchronous operation (`FsyncIsDurable`,
    `failure.IdempotentMutations`).

    **Postcondition.** As for `FsyncIsDurable`, even when the daemon had
    no buffered data for the inode.

??? abstract "SyncHandles"
    `spec.durability.SyncHandles`

    A handle opened with `O_SYNC`, `O_DSYNC` or `O_DIRECT` orders
    every write through the inode's single write buffer and assigns it a
    monotonic per-inode `write_seq` (`optimize.BufferedWriteFlushReq`).
    Before replying, the write commits the inode's dirty buffer up to and
    including its own `write_seq`, so an earlier buffered write from
    another handle can never be committed later on top of it. For
    `O_SYNC` and `O_DSYNC`, the commit runs with
    `SET LOCAL synchronous_commit = on` (and with `O_SYNC` includes the
    inode's attributes); for `O_DIRECT` without `O_SYNC` or `O_DSYNC`, it
    commits with `synchronous_commit = off`.

    **Durability.** For `O_SYNC` and `O_DSYNC`, on return from each
    write (`FsyncIsDurable`).

??? abstract "AsyncCommitWindow"
    `spec.durability.AsyncCommitWindow`

    Operations other than those in `FsyncIsDurable` and `SyncHandles`
    commit with `synchronous_commit = off`. Postgres's WAL writer then
    flushes them within three times `wal_writer_delay`. At mount the
    daemon reads `wal_writer_delay`, logs the resulting window (600 ms
    with the default 200 ms), and reports it in `pgfs stats`.

    **Durability.** Within the stated window of the commit, unless the
    host or Postgres crashes first. A crash within the window may lose
    those commits, always as a suffix of history
    (`PrefixCrashConsistency`). Because Postgres makes a transaction
    with `synchronous_commit = off` visible to other sessions as soon
    as it commits in memory, external readers (other mounts or SQL
    queries) can observe a commit during this window before its WAL
    record reaches disk (CAD r56); callers that require monotonic
    external reads across a primary crash must `fsync` (`FsyncIsDurable`)
    or read from a snapshot (`branch.MountsSnapshots`).

    **Sources.** Postgres asynchronous commit; Group Commit r25; CAD
    r56.

??? abstract "PrefixCrashConsistency"
    `spec.durability.PrefixCrashConsistency`

    After a crash of any component, the filesystem is in the state
    produced by applying some prefix of its committed operations, in
    commit order. Postgres replays WAL in order, and each operation is
    one transaction (`consistency.OneOperationOneTransaction`), so no
    crash can apply a later operation without the earlier ones, or part
    of one operation.

    **Crash.** The crash set after acknowledged operations `o1 … on` is
    `{ state(o1 … ok) : ok is at or after the last durability point }`,
    where buffered writes count as part of the operation that commits
    them. `verify.CrashConformance` checks every recovered state against
    this set.

    **Sources.** ALICE r118 (persistence properties), CrashMonkey r123,
    PoWER r60.

??? abstract "FlushesBeforeNaming"
    `spec.durability.FlushesBeforeNaming`

    Before an operation changes a name or a link of a file, it commits
    the file's pending writes: in the same transaction when the
    executing worker owns the file, or in an immediately preceding
    transaction on the file's owning worker before the naming
    transaction begins (`concurrency.NoLockAcrossRoundTrip`). The
    operations are `rename` (of the source and any replaced target),
    `link`, `unlink`, `setattr` when it changes `size`, and `release` of
    the last handle. Snapshot and branch drain every file. Because
    Postgres's WAL serialises the data commit before the naming commit,
    any order a program sees is preserved by a crash.

    **Postcondition.** After write, close or no close, then rename, a
    crash never leaves the new name pointing to a file that is missing
    the writes made before the rename. Writing to a temporary file and
    renaming it over the target is atomic even without fsync. ext4
    provides this only as the `auto_da_alloc` heuristic.

    **Sources.** ALICE r118 (rename-after-write is the most common
    application assumption), Skyros r129.

??? abstract "FlushesOnClose"
    `spec.durability.FlushesOnClose`

    Each open handle records `last_write_seq` (the highest per-inode
    `write_seq` written through that handle) and `seen_err_seq`
    (`optimize.BufferedWriteFlushReq`). `flush(fh)` and `release(fh)`
    commit the inode's write buffer up to `fh.last_write_seq` in one
    asynchronous transaction before replying (or do nothing if
    `inode.committed_seq ≥ fh.last_write_seq`), which also commits any
    earlier buffered writes from other handles on the same inode. This
    makes the data visible to every handle and every later mount, but
    does not make it durable (`DurabilityPoints`).

    **Errors.** Any unseen commit error on the inode (`err_seq >
    fh.seen_err_seq`) replies EIO to `flush`, so `close(2)` returns the
    error (`FsyncFailureSemantics`).

    **Sources.** AFS 05 (close-to-open), r90.

??? abstract "FsyncFailureSemantics"
    `spec.durability.FsyncFailureSemantics`

    When committing buffered data fails, at fsync, flush, release or
    under buffer pressure:

    - uncommitted data stays in the inode's write buffer and is retried
      by the next flush, fsync or close. Exception: when a synchronous
      commit succeeded locally on the primary but its
      synchronous-replication wait was cancelled or interrupted
      (`FsyncIsDurable`), its data is already committed in the database
      and is not re-inserted; instead the inode stays marked as needing
      `pgfs.sync_barrier` (`FsyncForcesEarlierCommits`) on the next
      fsync;
    - errors follow the Linux `errseq_t` model: the inode keeps a
      monotonic error sequence number `err_seq` incremented on any buffer
      commit or barrier failure, each open handle keeps the
      `seen_err_seq` it last observed, and every descriptor that was open
      when the failure occurred sees EIO once at its next `fsync` or
      `flush`/`close`;
    - after a reconnect, the daemon never reports success for an
      ambiguous synchronous operation until `op_log` shows it committed
      and a subsequent `pgfs.sync_barrier` succeeds
      (`failure.IdempotentMutations`, `FsyncForcesEarlierCommits`).

    The daemon never discards dirty data silently. If it must give up,
    for example at unmount with the database unreachable, it logs every
    affected inode at ERROR.

    **Sources.** Can Applications Recover from fsync Failures? r90.

??? abstract "FlushesOnShutdown"
    `spec.durability.FlushesOnShutdown`

    On SIGTERM, SIGINT or SIGHUP the daemon stops accepting new
    requests, commits every inode's pending write buffer, and always runs
    `pgfs.sync_barrier` (`FsyncForcesEarlierCommits`) with
    `SET LOCAL synchronous_commit = on` before calling
    `session.unmount()`, then unmounts and releases its session row in
    `mounts` with `synchronous_commit = on` (`app.MountsCleanly`).
    Running the synchronous barrier before `session.unmount()` ensures
    that metadata changes and data committed asynchronously by earlier
    `close()` calls inside `AsyncCommitWindow` are durable before any
    caller waiting on `umount` sees the unmount finish, and even if
    `session.unmount()` fails. It waits up to `--shutdown-timeout`
    (default 30 s) for the database. Data that still could not be
    committed is logged by inode at ERROR, and the process exits with
    status 1.

    **Durability.** Every operation acknowledged before the signal is
    durable once a clean shutdown completes.

??? abstract "ChecksServerDurability"
    `spec.durability.ChecksServerDurability`

    At mount and after every reconnect the daemon reads the server's
    settings and refuses to run when durability is impossible:

    | Setting | Required | Otherwise |
    |---|---|---|
    | `fsync` | `on` | refuse to mount |
    | `full_page_writes` | `on` | refuse to mount |
    | `data_checksums` | `on` | WARN at mount |
    | `wal_level` | `replica` or higher | refuse with `--replica` |

    `--i-accept-data-loss` overrides a refusal and is logged at WARN on
    every mount.

    **Sources.** LazyFS r135 (`full_page_writes = off` passes restart but
    fails amcheck), protocol-aware recovery r94, Torturing Databases
    r120.

??? abstract "ScratchMode"
    `spec.durability.ScratchMode`

    `pgfs mkfs --scratch` sets `meta.scratch = 'true'`, keeps the
    small control tables (`meta`, `trees`, `layers`, `snapshots`,
    `mounts`) `LOGGED`, and creates the high-churn data and queue tables
    (`inodes`, `dentries`, `file_blocks`, `blobs`, `xattrs`, `orphans`,
    `op_log`, `changes`, `usage_deltas`, `gc_queue`) `UNLOGGED`. Because
    PostgreSQL forbids a permanent (`LOGGED`) table from holding a
    foreign key into an `UNLOGGED` table (SQLSTATE `42P16`), only
    `UNLOGGED -> UNLOGGED` (`file_blocks.blob_id -> blobs.blob_id`) and
    `UNLOGGED -> LOGGED` (`op_log.mount_id -> mounts.mount_id`) foreign
    keys exist, and none of the logged tables references `inodes` or
    `blobs`.

    After a server crash (`ResetUnloggedRelations`) or a failover to a
    standby (which holds only the empty init forks of unlogged tables),
    Postgres truncates all `UNLOGGED` tables together while `meta` and
    `trees` survive with `schema_version`, `block_size` and `inline_max`
    intact. On a fresh `pgfs mount`, if `meta.scratch = 'true'` and no
    tree's root inode row exists (`NOT EXISTS (SELECT 1 FROM
    pgfs.inodes)`; never `layer = 1 AND ino = 1`, as layer 1 may have
    been removed after a promote), the mount calls `pgfs.scratch_reset()`
    — a stored SQL function, not DDL (`ops.RefusesUnformattedDatabase`).
    `pgfs.scratch_reset()` deletes `snapshots`, collapses `layers` and
    `trees` to freshly formatted state (`depth = 0`, `base_layer = NULL`,
    `epoch = 1`, `root_ino = 1`), recreates the root directory inode
    (`ino = 1`) for each tree, and logs at WARN that the scratch contents
    were lost. A live mount that is running when Postgres crashes never
    resets: because `op_log` is `UNLOGGED`, its slots are gone after
    recovery, and `failure.RevalidatesAfterReconnect` moves the live
    mount to `Fenced`.

    The mount banner says `scratch: contents are lost on a database
    crash or failover`, fsync promises nothing, and `--replica` is
    refused. Scratch suits build caches and test fixtures.

    **Durability.** None.

    **Crash.** After a server crash or failover the unlogged tables are
    empty, live mounts are fenced, and the filesystem is reset by
    `pgfs.scratch_reset()` on the next fresh mount.

---

<small>Generated from `spec.durability.DurableStorage` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
