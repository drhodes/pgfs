"""
pgfs requirements — granular, single-responsibility, testable.

Each class specifies one capability of the PostgreSQL-backed FUSE
filesystem. Every Requirement and Feature inherits the Rust-idiomatic
error-handling and specification contract contexts (err.py); the
observability and profiling contexts live in observe.py.
"""

from .err import STABLE, PARTIAL, Feat, Req, md
from .concurrency import (
    LockOrder,
    RenameSerialisationPoint,
)
from .consistency import (
    BoundedTransactions,
    DrainsBuffersBeforeExternalising,
    ExclusiveTreeLease,
    KernelCacheContract,
    OneOperationOneTransaction,
)
from .durability import (
    ChecksServerDurability,
    FlushesBeforeNaming,
    FlushesOnShutdown,
    FsyncIsDurable,
    SyncHandles,
)
from .integrity import (
    BlobChecksums,
)
from .ops import (
    RefusesUnformattedDatabase,
    SupportedPostgresVersions,
)
from .optimize import (
    BufferedWriteFlushReq,
)
from .posix import (
    ErrnoCatalogue,
    ExclusiveCreate,
    LinkCounts,
    OpenAfterUnlink,
    RenameFlags,
    SetsAttributes,
)
from .security import (
    EnforcesPermissions,
    OwnershipFromCaller,
    RedactsSecrets,
)
from .space import (
    FreesUnsharedBlobsInline,
)
from .storage import (
    BlockMap,
    DentryTable,
    ForgetsInodes,
    ImmutableBlobs,
    InlinesSmallFiles,
    PersistentInodes,
)
from .verify import (
    CheckAfterEveryTest,
)
from .failure import (
    BoundsEveryWait,
    DatabaseStateMachine,
    HonoursInterrupt,
    IdempotentMutations,
    InFlightAgeWatchdog,
    MapsDatabaseErrors,
    PreservesBuffersAcrossReconnect,
    ReconnectsWithBackoff,
    RetriesSerializationFailures,
    RevalidatesAfterReconnect,
    SlowDatabaseSignal,
)


class ConnectsToPostgres(Req):
    """The daemon connects to Postgres with a libpq connection string
    from `--conn` or `PGFS_CONN`, with the timeouts and keepalives of
    `failure.BoundsEveryWait`, and requires session-mode connections
    (`failure.RevalidatesAfterReconnect`). Before mounting, in one round
    trip, it checks:

    - the server version (`ops.SupportedPostgresVersions`);
    - the server's durability settings
      (`durability.ChecksServerDurability`);
    - the schema version (`storage.SchemaVersion`).

    It never creates or alters tables (`ops.RefusesUnformattedDatabase`;
    on a scratch filesystem whose unlogged tables were wiped by a crash,
    it calls `pgfs.scratch_reset()`, `durability.ScratchMode`). Then it
    registers its mount session or acquires the tree's lease
    (`consistency.ExclusiveTreeLease`).

    **Errors.** A connection failure exits with status 2 and a full
    error chain: the operation, the host and database, and the root
    cause. The connection string is redacted (`security.RedactsSecrets`).

    The schema is `storage.StorageLayout`. The first schema, the
    path-keyed `entries` and `blocks` tables created implicitly at
    mount, is retired.
    """

    deps = [
        RefusesUnformattedDatabase,
        SupportedPostgresVersions,
        ChecksServerDurability,
        ExclusiveTreeLease,
        BoundsEveryWait,
        RedactsSecrets,
    ]


class CreatesFiles(Req):
    """A FUSE `create` (or `mknod` of a regular file) calls
    `pgfs.create`, which in one transaction following
    `concurrency.LockOrder`:

    - allocates the new `ino` and a 63-bit random `gen`
      (`storage.PersistentInodes`), locks the parent directory's inode
      row, and updates the parent's mtime and ctime;
    - inserts the new inode (`shadows_base = false`), with the caller's
      owner (`security.OwnershipFromCaller`) and the requested mode
      after umask;
    - inserts the dentry with `INSERT … ON CONFLICT DO NOTHING
      RETURNING` (or replaces an existing whiteout with
      `shadows_base = true`, `branch.EpochVersionedRows`);
    - records the change (`changefeed.RecordsChanges`).

    The reply carries the new inode's attributes (`ino` and `gen`) and a
    handle.

    **Postcondition.** The name resolves to a new empty regular file
    whose `(ino, gen)` has never been used.

    **Crash.** The name, the inode and the parent's times appear
    together or not at all.

    **Concurrency.** The unique index decides EEXIST inside the
    statement, with no prior existence read (Lockify r41,
    `posix.ExclusiveCreate`).

    **Errors.** EEXIST if the name exists. ENOENT if the parent was
    removed. ENOTDIR if the parent is not a directory. ENAMETOOLONG or
    EINVAL for a bad name (`storage.ByteExactNames`). ENOSPC at capacity.
    EROFS on a snapshot.

    **Bounds.** One round trip. Two rows inserted and one updated.
    """

    deps = [PersistentInodes, OneOperationOneTransaction, ExclusiveCreate, OwnershipFromCaller]


class ReadsFiles(Req):
    """A FUSE `read` returns bytes `[offset, min(offset + size,
    file size))` of a regular file. It first merges the inode's pending
    write buffer over the committed content
    (`consistency.DrainsBuffersBeforeExternalising`). It reads inline
    data, or the blobs mapped to the touched blocks, resolving layers on
    a branch (`branch.LayeredLookup`), and verifies every blob's
    checksum (`integrity.BlobChecksums`).

    **Postcondition.** Every byte returned is the last byte written at
    that offset, through any handle on the mount, or zero for a hole.
    A read at or past the end returns no bytes.

    **Errors.** EISDIR for a directory. EIO for a checksum mismatch.
    ESTALE if the inode was deleted or its `gen` does not match the
    handle (`storage.PersistentInodes`). ENOENT never: the kernel holds
    a reference to the inode.

    **Bounds.** One round trip, fetching only the touched blocks.

    Replica reads of file data follow `replica.LsnTicketReads`.
    """

    deps = [BlockMap, InlinesSmallFiles, DrainsBuffersBeforeExternalising, BlobChecksums]


class WritesFiles(Req):
    """A FUSE `write` places `data` at `offset` in a regular file. Small
    or unaligned writes go to the inode's write buffer
    (`BufferedWriteFlushReq`). Writes on `O_SYNC`, `O_DSYNC` or
    `O_DIRECT` handles commit at once (`durability.SyncHandles`).
    Committing a block that is only partly covered by new data reads the
    current block, overlays the new bytes, and stores the result as a
    new blob (`storage.ImmutableBlobs`). A block that did not exist
    starts as zeros.

    **Postcondition.** For every byte `b` outside
    `[offset, offset + len)`, `content′(b) = content(b)`. The written
    range holds `data`. `size′ = max(size, offset + len)`, and bytes in
    any gap between the old size and `offset` read as zero
    (`posix.SparseFiles`). mtime and ctime are set (`posix.Timestamps`),
    and set-id bits are cleared as `security.ClearsSetIdBits` requires.

    **Durability.** None at reply. See `durability.DurabilityPoints`. A
    successful `write()` does not mean the data is in the database, and
    a daemon crash loses acknowledged writes that were not yet
    committed.

    **Errors.** EBADF for a read-only handle (from the kernel). EFBIG
    past the maximum size. ENOSPC at capacity. EROFS on a snapshot. EIO
    when a needed buffer commit fails.

    The first postcondition is the clause whose absence let Bug 1 ship:
    a buffered partial-block write zeroed the bytes it did not touch.
    """

    deps = [BufferedWriteFlushReq, ImmutableBlobs, SyncHandles]


class UnlinksFiles(Req):
    """A FUSE `unlink` calls `pgfs.unlink`, which in one transaction
    following the discover-sort-lock-revalidate protocol of
    `concurrency.LockOrder`:

    - discovers `child_ino`, locks `parent_ino` and `child_ino` in
      ascending `ino` order, and updates the parent's mtime and ctime;
    - decrements the child inode's `nlink` (in a subtree branch, drops
      `nlink` to 0 if no remaining live dentry is reachable from
      `trees.root_ino`, `branch.ViewsRootedAtDirectory`);
    - removes the dentry, or writes a whiteout when `shadows_base = true`
      (`branch.CopiesUpOnWrite`);
    - records the change.

    If `nlink` reaches 0, the inode is deleted (or kept as an inode
    whiteout with `nlink = 0, size = 0, inherit_limit = 0,
    inline_data = NULL, shadows_base = true` when `shadows_base = true`,
    `branch.CopiesUpOnWrite`) together with its block map and xattrs in
    the layer and unshared blobs (`space.FreesUnsharedBlobsInline`),
    unless a handle is open, in which case it becomes an orphan
    (`posix.OpenAfterUnlink`). Pending writes of the file are committed
    first (`durability.FlushesBeforeNaming`, verified against the
    mount's dirty-inode set via `concurrency.InodeShardedWorkers`).

    **Postcondition.** The name no longer resolves. Other names of the
    inode, and open handles, are unaffected.

    **Crash.** All or nothing.

    **Errors.** ENOENT if the name does not exist. EISDIR for a
    directory. EROFS on a snapshot.

    **Bounds.** Constant work in the transaction. The blocks of a file
    larger than one batch go to the reclaimer
    (`consistency.BoundedTransactions`).
    """

    deps = [OpenAfterUnlink, FreesUnsharedBlobsInline, FlushesBeforeNaming]


class CreatesDirectories(Req):
    """A FUSE `mkdir` calls `pgfs.mkdir`, which in one transaction
    following `concurrency.LockOrder`:

    - allocates the new `ino` and a 63-bit random `gen`
      (`storage.PersistentInodes`), locks the parent directory's inode
      row, increments the parent's `nlink`, and updates the parent's
      mtime and ctime;
    - inserts a directory inode (`shadows_base = false`), with the
      requested mode after umask and the caller's owner (set-group-id
      inherited, `security.OwnershipFromCaller`);
    - inserts its dentry, with `ON CONFLICT DO NOTHING` (or replaces an
      existing whiteout with `shadows_base = true`,
      `branch.EpochVersionedRows`);
    - records the change.

    **Postcondition.** The name resolves to an empty directory with
    `nlink = 2`, and the parent's `nlink` grew by one (`posix.LinkCounts`).

    **Crash.** All or nothing.

    **Errors.** EEXIST, ENOENT, ENOTDIR, ENAMETOOLONG, EINVAL, ENOSPC,
    EROFS, as for `CreatesFiles`.
    """

    deps = [PersistentInodes, LinkCounts]


class RemovesDirectories(Req):
    """A FUSE `rmdir` calls `pgfs.rmdir`. In one transaction following
    the discover-sort-lock-revalidate protocol of
    `concurrency.LockOrder`, it discovers the directory's `ino`, locks
    `parent_ino` and the directory's `ino` in ascending `ino` order,
    checks that no live dentry names the directory as its parent, and
    then:

    - decrements the parent's `nlink` and updates the parent's mtime
      and ctime;
    - deletes or retires any child dentry whiteouts under the directory
      in this layer so no live child row remains under a whited-out
      directory (`storage.DentryTable`);
    - deletes the directory's inode (or turns it into an inode whiteout
      when `shadows_base = true`, `branch.CopiesUpOnWrite`);
    - removes the directory's dentry, or writes a whiteout when
      `shadows_base = true`;
    - records the change.

    On a branch, the emptiness check sees the merged view, so a
    directory whose inherited entries are all whited out is empty.

    **Concurrency.** A create in the directory locks the same inode row
    (`concurrency.LockOrder`), so no create can land between the check
    and the delete.

    **Crash.** All or nothing.

    **Errors.** ENOTEMPTY if any entry remains. ENOENT if absent.
    ENOTDIR if not a directory. EBUSY for the root of a view. EROFS on a
    snapshot.
    """

    deps = [LockOrder, LinkCounts]


class ListsDirectories(Req):
    """A FUSE `readdir` or `readdirplus` returns `.` and `..` and then
    the directory's entries in byte order of name. Each entry has its
    inode number and type, and readdirplus also returns attributes. The
    offset of each entry is a cookie that the directory handle maps to
    the entry's name (a 64-bit offset cannot hold a 255-byte name). The
    next call resumes with `name > cookie`, using keyset pagination over
    the `(layer, parent_ino, name)` index. On a branch, the merged view of
    `branch.LayeredLookup` is paginated the same way.

    **Postcondition.** Every entry that exists for the whole listing is
    returned exactly once. An entry created or removed during the
    listing may or may not appear, never twice.

    **Bounds.** Each call costs one indexed range scan of at most the
    number of entries that fit the kernel's buffer. Listing a directory
    of `n` entries is O(n) in total.

    **Sources.** ScaleCheck r57, HMFS r131, TableFS r07. The first
    implementation re-ran the full listing on every call and indexed
    into it, which is O(n²) and skips or repeats entries under
    concurrent change.
    """

    deps = [DentryTable]


class LooksUpEntries(Req):
    """A FUSE `lookup` of `(parent ino, name)` returns the child's
    persistent inode number (`ino`), generation (`inodes.gen`,
    `storage.PersistentInodes`) and attributes, and increments the
    kernel lookup count that `forget` later decrements
    (`storage.ForgetsInodes`). A miss replies ENOENT with an entry TTL,
    so the kernel caches the negative result
    (`consistency.KernelCacheContract`).

    **Postcondition.** The same name resolves to the same `(ino, gen)`
    across lookups, restarts and remounts, until it is unlinked or
    renamed.

    **Bounds.** One index probe per layer of the view, in one round
    trip, or none on a daemon cache hit.
    """

    deps = [PersistentInodes, ForgetsInodes, KernelCacheContract]


class ReportsAttributes(Req):
    """A FUSE `getattr` returns kind, mode, nlink, uid, gid, rdev, size,
    `blocks` (512-byte units of stored data, holes excluded), `blksize`
    (the block size), and atime, mtime, ctime and birth time in
    nanoseconds. Size and mtime include writes still buffered on the
    mount (`consistency.DrainsBuffersBeforeExternalising`). The reply's
    TTL follows `consistency.KernelCacheContract`. Inode 1 is the root
    directory of the mounted view.

    **Postcondition.** The values reflect every operation that replied
    before the request.
    """

    deps = [DrainsBuffersBeforeExternalising, KernelCacheContract]


class TruncatesFiles(Req):
    """A FUSE `setattr` with a size truncates or extends a regular file.
    Pending buffered writes in the inode's write buffer past the new
    size are discarded first, so that no later flush brings them back,
    and any remaining buffered writes below the new size are committed
    (`durability.FlushesBeforeNaming`). Then, in one transaction, it:

    - sets `size`, and on a downward truncate in a branch lowers
      `inherit_limit` to `min(inherit_limit, floor(new / block_size))`
      (`branch.CopiesUpOnWrite`);
    - for an inline file (`storage.InlinesSmallFiles`), slices or
      zero-pads `inline_data` in place when `new ≤ inline_max`, or moves
      `inline_data` into block 0 and clears `inline_data` when `new >
      inline_max`;
    - deletes or retires map rows in the layer at or past the new end;
    - stores a new blob with its tail zeroed when the new end falls
      inside a block (copying up from the base layer if inherited);
    - sets mtime and ctime, clears set-id bits
      (`security.ClearsSetIdBits`), and records the change
      (`changefeed.RecordsChanges`).

    Extending past `inline_max` creates a hole.

    **Postcondition.** Bytes in `[0, min(old, new))` are unchanged. Bytes
    in `[min(old, new), new)` read as zero, including after any later
    extension.

    **Crash.** All or nothing. A truncate larger than one batch commits
    the new size and hands the remaining map rows to the reclaimer,
    which cannot expose them, because they lie past the size
    (`consistency.BoundedTransactions`).

    **Errors.** EISDIR for a directory. EFBIG past the maximum size.
    EROFS on a snapshot.
    """

    deps = [BlockMap, BoundedTransactions]


class RenamesEntries(Req):
    """A FUSE `rename` calls `pgfs.rename`, which moves one dentry from
    `(old parent, old name)` to `(new parent, new name)`. It follows
    POSIX semantics:

    - If the target exists and is a non-directory, it is replaced
      atomically. Its `nlink` drops, and it is deleted or orphaned as in
      `UnlinksFiles`.
    - If the target is an empty directory, it is replaced atomically.
    - If the target is a non-empty directory, it replies ENOTEMPTY.
    - If the source is a non-directory and the target a directory, it
      replies EISDIR.
    - If the source is a directory and the target a non-directory, it
      replies ENOTDIR.
    - A directory keeps its inode and its descendants. Only its dentry
      row moves (preserving `shadows_base` on the old and new keys,
      `branch.EpochVersionedRows`), and the parents' `nlink`, mtime and
      ctime change. The renamed inode's ctime is set, and the change is
      recorded (`changefeed.RecordsChanges`).

    **Invariant.** Exactly one dentry changes, plus at most one replaced
    target. No other row is touched, and no name matching of any kind is
    involved. This is what makes Bug 2 impossible: the old rename
    matched descendants with `LIKE`, so `_` and `%` acted as wildcards.

    **Crash.** All or nothing: after a crash, the source name or the
    target name resolves to the inode, never both and never neither.

    **Concurrency.** Pending writes of the source and of a replaced
    target are committed first (`durability.FlushesBeforeNaming`,
    verified against the mount's dirty-inode set via
    `concurrency.InodeShardedWorkers`). That is Bug 3: rename lost
    buffered writes. A directory moving to a different parent takes the
    rename lock (`concurrency.RenameSerialisationPoint`). Rows lock in
    the order of `concurrency.LockOrder`.

    **Errors.** ENOENT if the source is missing. EXDEV across trees.
    EROFS on a snapshot. EBUSY for the root of a view.

    **Bounds.** Constant work, whatever the size of the subtree (Oracle
    FSS r91, InfiniFS r16).
    """

    deps = [RenameSerialisationPoint, FlushesBeforeNaming]


class RejectsInvalidRenames(Req):
    """`rename` validates before it changes anything:

    - moving a directory into itself or one of its descendants replies
      EINVAL, checked under the rename lock;
    - renaming a name onto a name of the same inode succeeds and does
      nothing, as POSIX requires;
    - `RENAME_NOREPLACE` and `RENAME_EXCHANGE` are supported
      (`posix.RenameFlags`);
    - `RENAME_WHITEOUT` and unknown flags reply EINVAL;
    - `.` and `..` as either name reply EINVAL (the kernel also checks).
    """

    deps = [RenamesEntries, RenameFlags]


class MountsCleanly(Req):
    """`pgfs mount VIEW MOUNTPOINT` mounts with `AutoUnmount`, so the
    kernel removes the mount if the process exits for any reason. It
    also sets `default_permissions` (`security.EnforcesPermissions`) and
    names the mount `pgfs-<first 8 hex digits of fs_uuid>` in
    `/proc/mounts`; FUSE's statfs reply has no field for a filesystem
    id. If `AutoUnmount` is refused,
    for example when `/etc/fuse.conf` lacks `user_allow_other`, the
    daemon mounts without it and logs a WARN saying a crashed daemon
    will leave a stale mount.

    The mountpoint must be an existing, empty directory that is not
    already mounted. The mount session or tree lease is registered in
    `mounts` (`consistency.ExclusiveTreeLease`) before the mount
    appears, and released only after the unmount, so no other mount can
    write the tree while this mount's kernel caches still exist.

    **Errors.** EBUSY if the tree is mounted read-write elsewhere. The
    error names the holder.
    """

    deps = [ExclusiveTreeLease, EnforcesPermissions]


class HandlesShutdownSignals(Req):
    """SIGINT, SIGTERM and SIGHUP start a clean shutdown on a dedicated
    `sigwait` thread, which never blocks dispatch. The shutdown commits
    every buffer and runs `pgfs.sync_barrier` so every earlier
    asynchronous commit is flushed to WAL (`durability.FlushesOnShutdown`),
    calls `session.unmount()`, and then releases the mount session or
    lease synchronously (`synchronous_commit = on`, `MountsCleanly`). If
    the unmount fails, the lease is not released: `AutoUnmount` removes
    the mount when the process exits, and the lease expires on its own.
    """

    deps = [FlushesOnShutdown]


class DistinguishesExpectedErrors(Req):
    """Expected conditions reply their errno to the kernel and are logged
    at DEBUG at most. These are the errnos in `posix.ErrnoCatalogue`
    other than EIO. Unexpected failures are logged in full through
    `log_and_reply!`: database errors that map to EIO, checksum
    failures, invariant violations and fencing. Database errors are
    classified by SQLSTATE in one table (`failure.MapsDatabaseErrors`),
    never by matching message text.
    """

    deps = [ErrnoCatalogue, MapsDatabaseErrors]


class ReportsCrashes(Req):
    """If the daemon panics, the panic hook emits a crash report to both
    stderr and the log channel. The report includes the panic message,
    file:line location, a symbolized backtrace, and OS/Rust metadata.
    In release builds, `panic = \"abort\"` prevents a broken daemon from
    lingering."""


class UsesStructuredTracing(Req):
    """The daemon uses the `tracing` crate (not bare `log`/`env_logger`)
    for all diagnostic output. The `tracing-subscriber` subscriber is
    initialized before any other work. `RUST_LOG` controls filter levels
    with the same syntax as `env_logger`; `RUST_LOG_FORMAT=json` switches
    to machine-parseable JSON output."""


class InstrumentsFuseCallbacks(Req):
    """Every `Filesystem` callback the daemon implements opens a
    `tracing::info_span!` recording `ino`, `name` where there is one,
    the worker, and the view. The span is entered for the duration of
    the callback, so nested database operations are attributed to it.
    The callbacks are those listed in `posix.PosixCompatibility`, plus
    `init`, `destroy`, `forget`, `batch_forget`, `statfs`, `interrupt`
    and the open and release calls. The list is checked by a test
    against the `Filesystem` impl, so it cannot drift from the code.
    """


class InstrumentsDbOperations(Req):
    """Every database call opens a `tracing::debug_span!` recording the
    `pgfs` function or prepared statement name, the `ino` it concerns,
    the backend pid, and the round-trip time. A slow span can therefore
    be matched to its row in `pg_stat_activity` (Fail-Slow at Scale
    r122).
    """


class SupportsGitInit(Req):
    """Git works on a mount, as a regression workload for the POSIX
    details it depends on. On a mounted tree, `git init`, `git clone`
    of a pinned repository, `git status`, `git commit`, `git gc` and
    `git checkout` of another branch all succeed, and `git fsck` passes.
    This exercises:

    - rename-over for lockfiles;
    - `O_EXCL` creates of `*.lock` (`posix.ExclusiveCreate`);
    - `chmod +x` on hooks (`posix.SetsAttributes`);
    - `fsync` of objects and refs under `core.fsync`
      (`durability.FsyncIsDurable`);
    - hard links and open-after-unlink during `gc`.

    The same workload is a benchmark (`perf.BenchmarkSuite`).

    **Sources.** Bento r53 (git through FUSE as the yardstick), ALICE
    r118.
    """

    deps = [RenamesEntries, SetsAttributes, ExclusiveCreate, FsyncIsDurable]


# ── Development environment ──────────────────────────────────────────

class ProvidesProjectLocalCluster(Req):
    """`scripts/init_db.sh` creates a PostgreSQL cluster at
    `testdata/pgdata`, with
    `initdb --auth=trust --no-locale --encoding=UTF8 --data-checksums`.
    It starts the cluster with its Unix socket in `testdata/` and a TCP
    listener on 127.0.0.1 only, and checks that `fsync` and
    `full_page_writes` are on. It ensures that a superuser role named
    after the invoking OS user and a database named `pgfs` exist, runs
    `pgfs mkfs` if the database is unformatted, and prints the
    connection string.

    `scripts/ensure_db.sh` is the idempotent wrapper used by
    `make test`: it exits at once when the cluster already answers.
    `init_db.sh --lazyfs` puts the data directory on LazyFS, so that
    crash conformance can run locally (`verify.CrashConformance`).
    Nothing is written outside the project's `testdata/` directory.

    **Sources.** LazyFS r135, protocol-aware recovery r94.
    """


class ProvidesLifecycleScript(Req):
    """`scripts/pgfs.sh up|down|status|run` owns the mount lifecycle.
    `up` starts Postgres if needed, runs `cargo build`, sweeps a stale mount
    whose daemon is gone, starts `target/debug/pgfs` detached with output
    appended to `testdata/pgfs.log`, and waits up to 10 s for the mount to
    appear in `/proc/mounts`. `down` sends SIGTERM, waits up to 10 s, then
    escalates to `fusermount3 -u`, reports the holders of a busy mount,
    and finally lazy-unmounts. `status` reports mount, daemon and Postgres
    state and flags ghost mounts. `run` builds and execs the daemon in the
    foreground. `PGFS_MOUNT` overrides the default `testdata/mnt`."""


class RunsIntegrationSuite(Req):
    """`scripts/run_tests.sh` (`make test-integration`) runs every
    `scripts/test_*.sh` except the shared harness `test_lib.sh`, each in
    isolation under a 300 s timeout, writing each test's output to
    `testdata/<test>.log`. After every test it kills any leftover daemon
    and lazily unmounts `testdata/mnt` so the next test starts clean. It
    then runs `pgfs check`, and a violation fails the test
    (`verify.CheckAfterEveryTest`). It prints PASS or FAIL per test and
    exits non-zero if any test failed. This shell layer is the base. The
    model, crash and fault suites in `verify.py` find the bug classes it
    cannot."""

    deps = [CheckAfterEveryTest]


# ── Observability requirements ───────────────────────────────────────

class ExportsPeriodicMetrics(Req):
    """A background thread logs nine INFO lines every 60 s:
    `metrics|ops|…` (per-callback counters), `metrics|errors|…` (EIO and
    per-errno counters),
    `metrics|replica|reads=… fallbacks=… lag_bytes=… lag_ms=… ticket_waits=…`
    (`replica.ReplicaObservability`), `metrics|latency|fuse=…` and
    `metrics|latency|db=…`. Counters are
    lifetime totals and never reset. Latency histograms bucket wall-clock
    durations at ≤10µs, ≤100µs, ≤1ms, ≤10ms, ≤100ms, ≤1s and >1s and
    report p50/p95/p99 as the upper bound of the bucket that contains the
    percentile.

    Further lines report:

    - `metrics|db|state=… reconnects=… fenced=… slow_db=…`;
    - `metrics|inflight|oldest_ms=…`;
    - `metrics|space|…` (`space.ReportsSpace`);
    - `metrics|integrity|checksum_failures=…`.

    Every per-errno counter is incremented where its errno is replied,
    or removed. None stays at zero by omission."""


class DumpsStateOnSigusr1(Req):
    """SIGUSR1 makes the signal thread itself write a state dump at INFO.
    It does not wait for the next callback, so the dump appears even
    when every worker is stuck. The dump reads only atomics and
    `try_lock` snapshots, and prints `locked` for any state it cannot
    take at once. It contains:

    - the mount, view, fence and connection states;
    - the in-flight request of each worker, with its age and backend
      pid;
    - cache, handle and buffer sizes;
    - the replica state;
    - the latency summaries.

    **Sources.** Gray Failure r124. The first design ran the dump on the
    next callback, which on a stuck mount never comes."""

    deps = [InFlightAgeWatchdog]


class WritesHealthFiles(Req):
    """After connecting to Postgres the daemon writes its PID to
    `/tmp/pgfs-<hash>.pid`, prints `health pid file: <path>` on stdout and
    logs it. After the FUSE session is created it writes
    `/tmp/pgfs-<hash>.ready` containing `ready`. `<hash>` is 16 hex digits
    derived from the mountpoint argument exactly as given. Both files are
    removed when `run()` returns."""


# ── Features (user-visible capabilities) ─────────────────────────────

class ProjectLocalEnvironment(Feat):
    """A contributor goes from a fresh clone to a live pgfs mount with one
    command. The Postgres cluster, the mount point, the logs and the pid
    files all live under the project's `testdata/` directory, so nothing
    touches a system-wide Postgres and `rm -rf testdata` is a full reset."""

    deps = [
        ProvidesProjectLocalCluster,
        ProvidesLifecycleScript,
        RunsIntegrationSuite,
        ConnectsToPostgres,
    ]

    def title(self):
        return "Project-Local Development Environment"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "`scripts/init_db.sh` creates the cluster with "
            "`--data-checksums`, checks `fsync` and `full_page_writes`, "
            "and formats the `pgfs` database with `pgfs mkfs`; "
            "`scripts/pgfs.sh`, the Makefile targets and the integration "
            "suite drive the mount, and `run_tests.sh` runs `pgfs check` "
            "after every test. The `--lazyfs` mode needs LazyFS installed "
            "and is not yet exercised by the suite."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will go from a fresh clone to a mounted pgfs
            filesystem, write a file, find it in PostgreSQL, and take everything
            down again. Everything lives under `testdata/`.

            ## Step 1: enter the development shell

            ```bash
            nix develop
            ```

            ## Step 2: bring it up

            ```bash
            ./scripts/pgfs.sh up
            ```

            This creates the cluster in `testdata/pgdata` (with data checksums),
            starts it on a Unix socket in `testdata/` and on 127.0.0.1, formats the
            `pgfs` database with `pgfs mkfs`, builds the binary and mounts the tree
            `main` at `testdata/mnt`.

            ## Step 3: write a file

            ```bash
            mkdir testdata/mnt/tutorial
            echo "hello from a database" > testdata/mnt/tutorial/hello.txt
            cat testdata/mnt/tutorial/hello.txt
            ```

            ```text
            hello from a database
            ```

            ## Step 4: find the file in PostgreSQL

            ```bash
            psql -h "$PWD/testdata" -d pgfs -c \
              "SELECT path, ino, kind, size FROM pgfs.files WHERE tree = 'main' AND path LIKE '/tutorial%'"
            ```

            ```text
                    path         | ino | kind | size
            ---------------------+-----+------+------
             /tutorial           |   2 | d    |    0
             /tutorial/hello.txt |   3 | f    |   22
            ```

            A file this small lives inside its inode row
            ([Storage Layout](storagelayout.md)).

            ## Step 5: take it down

            ```bash
            rm -r testdata/mnt/tutorial
            ./scripts/pgfs.sh down
            ./scripts/pgfs.sh status
            ```

            ```text
            mount:     none
            daemon:    not running
            postgres:  running
            ```

            ## What we did

            We built a private PostgreSQL, formatted it, mounted it, and saw a file
            become rows. `rm -rf testdata` undoes all of it.
        """)

    def how_to(self):
        return md(r"""
            ## Start from scratch

            ```bash
            pg_ctl -D testdata/pgdata stop; rm -rf testdata
            ./scripts/pgfs.sh up
            ```

            ## Run the tests

            ```bash
            make test               # cargo test: unit, storage and model-conformance tests
            make test-integration   # scripts/test_*.sh, each followed by pgfs check
            ```

            ## Run the daemon in the foreground

            ```bash
            ./scripts/pgfs.sh run
            ```

            ## Mount somewhere else

            ```bash
            PGFS_MOUNT=/tmp/pgfs ./scripts/pgfs.sh up
            ```

            ## Put the cluster on LazyFS for crash tests

            ```bash
            ./scripts/init_db.sh --lazyfs
            ```

            This needs [LazyFS](https://github.com/dsrhaslab/lazyfs) installed.
        """)

    def reference(self):
        return md(r"""
            ## Scripts

            | Script | Does |
            |---|---|
            | `scripts/init_db.sh [--lazyfs]` | create and start the cluster in `testdata/pgdata` (`--data-checksums`, socket in `testdata/`, TCP on 127.0.0.1), check `fsync` and `full_page_writes`, create and `pgfs mkfs` the `pgfs` database, print the connection string |
            | `scripts/ensure_db.sh` | `init_db.sh` unless the cluster already answers |
            | `scripts/pgfs.sh up\|down\|status\|run` | mount lifecycle, with stale-mount sweeping and escalating unmount |
            | `scripts/run_tests.sh` | every `scripts/test_*.sh` under a 300 s timeout, then `pgfs check` |

            ## Files

            | Path | Holds |
            |---|---|
            | `testdata/pgdata` | the cluster |
            | `testdata/postgres.log` | the server log |
            | `testdata/mnt` | the default mount point |
            | `testdata/pgfs.log` | the daemon log from `pgfs.sh up` |
            | `testdata/<test>.log` | each integration test's output |

            ## Environment

            | Variable | Default |
            |---|---|
            | `PGFS_CONN` | `host=<repo>/testdata dbname=pgfs` |
            | `PGFS_MOUNT` | `testdata/mnt` |
        """)

    def explanation(self):
        return md(r"""
            ## Why everything lives in the repository

            A development filesystem that depends on a system-wide PostgreSQL
            leaks state between projects and between people. Keeping the cluster,
            the socket, the mount point and the logs under `testdata/` makes a
            reset one command, and makes the tests reproducible.

            ## Why checksums and durability checks at setup

            pgfs promises that fsync means durable. That is only true on a server
            with `fsync` and `full_page_writes` on, and silent corruption is only
            detectable with page checksums, so the local cluster is set up the way
            pgfs asks production servers to be.
        """)

class FlatFilesystem(Feat):
    """Users can create, read, write, and delete files through standard
    POSIX file operations on the mounted directory."""

    deps = [
        ConnectsToPostgres,
        CreatesFiles,
        ReadsFiles,
        WritesFiles,
        TruncatesFiles,
        UnlinksFiles,
        LooksUpEntries,
        ReportsAttributes,
        DistinguishesExpectedErrors,
    ]

    def title(self):
        return "Regular Files"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "Create, read, write, append, truncate and delete run on the "
            "inode-keyed schema: persistent inode numbers with "
            "generations, buffered writes visible to every handle on the "
            "mount, read-modify-write of partial blocks (Bug 1), "
            "single-transaction unlink and checksummed immutable blocks. "
            "Covered by `tests/storage.rs`, the model-conformance suite "
            "and `scripts/test_*.sh`."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will create, write, append to, truncate and delete
            a file, and look at its inode in PostgreSQL. We need a mount (see
            [Your first pgfs mount](projectlocalenvironment.md)).

            ## Step 1: create and write

            ```bash
            cd testdata/mnt
            echo "first line" > notes.txt
            echo "second line" >> notes.txt
            cat notes.txt
            ls -i notes.txt
            ```

            The inode number `ls -i` prints is the file's `ino` in the database,
            and stays the same across remounts.

            ## Step 2: look at the inode

            ```bash
            psql -h "$OLDPWD/testdata" -d pgfs -c \
              "SELECT ino, size, convert_from(inline_data, 'UTF8') FROM pgfs.inodes
               WHERE ino = $(stat -c %i notes.txt) AND died = 9223372036854775807"
            ```

            The 23 bytes are inside the inode row.

            ## Step 3: truncate and delete

            ```bash
            truncate -s 5 notes.txt && cat notes.txt; echo
            rm notes.txt
            ```

            ## What we did

            We used ordinary tools on a pgfs file, and saw that a file is an inode
            row with a permanent number.
        """)

    def how_to(self):
        return md(r"""
            ## Make sure data is durable

            `write` returns once the data is buffered in the daemon. `close`
            commits it (visible to everyone, durable within the asynchronous commit
            window); `fsync` makes it durable before returning
            ([Durability and Crash Safety](durablestorage.md)).

            ## Find a file's rows

            ```sql
            SELECT path, ino, size FROM pgfs.files WHERE tree = 'main' AND path = '/docs/notes.txt';
            SELECT block_no, blob_id FROM pgfs.file_blocks
            WHERE ino = 1001 AND died = 9223372036854775807 ORDER BY block_no;
            ```

            ## Diagnose an `Input/output error`

            EIO means something unexpected failed. The daemon log has the full
            error chain; `grep ERROR` it. A `checksum mismatch` names the inode,
            block and blob ([Integrity and Backup](dataintegrity.md)).
        """)

    def reference(self):
        return md(r"""
            ## FUSE operations

            | Operation | SQL | Errors |
            |---|---|---|
            | `create`, `mknod` (regular) | `pgfs.create` | EEXIST, ENOENT, ENOTDIR, ENAMETOOLONG, EINVAL, ENOSPC, EROFS |
            | `lookup` | `pgfs.lookup` (negative results are cached by the kernel) | ENOENT |
            | `getattr` | `pgfs.getattr`, or the daemon's cache | ESTALE |
            | `read` | `pgfs.read_blocks`, with buffered writes overlaid | EISDIR, EIO (checksum), ESTALE |
            | `write` | buffered; committed by `pgfs.write_blocks` | EBADF, EFBIG, ENOSPC, EROFS, EIO |
            | `setattr` with a size | `pgfs.setattr` | EISDIR, EFBIG, EROFS |
            | `unlink` | `pgfs.unlink` | ENOENT, EISDIR, EROFS |

            ## Limits

            | Limit | Value |
            |---|---|
            | file size | 2^63 − 1 bytes |
            | name | 1 – 255 bytes |
        """)

    def explanation(self):
        return md(r"""
            ## A file is an inode row

            Every file has a permanent inode number and a random generation. The
            kernel holds `(ino, gen)`, and every operation checks it, so a number
            reused after a database restore is caught as ESTALE rather than
            silently reading another file.

            ## Why writes are buffered

            Each database round trip costs far more than copying a few kilobytes.
            Writes collect in a per-inode buffer and commit as whole blocks at
            close, fsync or under memory pressure. A partly written block is read
            and overlaid before it is stored, so untouched bytes are never zeroed
            (Bug 1), and every read on the mount sees the buffered bytes.
        """)

class HierarchicalDirectories(Feat):
    """Users can create nested directories, list their contents, and
    remove empty directories through standard POSIX directory
    operations."""

    deps = [
        CreatesDirectories,
        RemovesDirectories,
        ListsDirectories,
        LooksUpEntries,
        ReportsAttributes,
    ]

    def title(self):
        return "Directory Trees"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "`mkdir`, `rmdir`, `readdir` and `readdirplus` keep link "
            "counts and directory times, page by name with keyset "
            "cookies, and check emptiness in the same transaction as the "
            "delete. Covered by `tests/storage.rs`, the model-conformance "
            "suite and `test_dirs.sh`."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will build a small project tree, list it, and see
            how directories are stored. We need a mount.

            ## Step 1: build a tree

            ```bash
            cd testdata/mnt
            mkdir -p project/src/util project/docs
            touch project/src/main.rs project/src/util/strings.rs project/docs/README.md
            ls -la project/src
            ```

            ## Step 2: see the link counts

            ```bash
            stat -c '%n %h' project project/src
            ```

            A directory's link count is 2 plus its subdirectories.

            ## Step 3: see how the tree is stored

            ```bash
            psql -h "$OLDPWD/testdata" -d pgfs -c \
              "SELECT path, kind FROM pgfs.files WHERE tree = 'main' AND path LIKE '/project%' ORDER BY path"
            ```

            Each name is a `dentries` row `(parent_ino, name) -> ino`; no row
            stores a path.

            ## Step 4: remove it

            ```bash
            rm -r project
            ```
        """)

    def how_to(self):
        return md(r"""
            ## List a large directory

            `ls` and `find` page through it 256 names at a time, resuming after
            the last name returned, so a listing costs O(n) whatever the size and
            never repeats an entry.

            ## Count everything below a directory

            ```sql
            SELECT kind, count(*) FROM pgfs.files
            WHERE tree = 'main' AND starts_with(path, '/a/')
            GROUP BY kind;
            ```

            ## Find the largest directories

            ```sql
            SELECT regexp_replace(path, '/[^/]*$', '') AS dir, sum(size) AS bytes
            FROM pgfs.files WHERE tree = 'main' AND kind = 'f'
            GROUP BY dir ORDER BY bytes DESC LIMIT 10;
            ```
        """)

    def reference(self):
        return md(r"""
            ## FUSE operations

            | Operation | SQL | Errors |
            |---|---|---|
            | `mkdir` | `pgfs.mkdir` | EEXIST, ENOENT, ENOTDIR, ENAMETOOLONG, EINVAL, ENOSPC, EROFS |
            | `rmdir` | `pgfs.rmdir` | ENOTEMPTY, ENOENT, ENOTDIR, EBUSY (root of the view), EROFS |
            | `readdir`, `readdirplus` | `pgfs.readdir`, `pgfs.readdirplus`, 256 names per page | ENOTDIR |

            ## Attributes

            | Field | Value |
            |---|---|
            | `nlink` | 2 + subdirectories |
            | `mtime`, `ctime` | updated by every create, remove and rename in the directory |
        """)

    def explanation(self):
        return md(r"""
            ## Names point at inodes

            A directory entry is a row `(parent_ino, name) -> ino`. A lookup is one
            index probe, a listing is one range scan in byte order of name, and a
            rename changes one row whatever the size of the subtree. The first pgfs
            schema keyed rows by path, which made a directory rename rewrite every
            descendant.

            ## Why emptiness is checked inside the transaction

            `rmdir` locks the directory's inode row before checking that it is
            empty, and a create in that directory locks the same row, so no create
            can slip in between the check and the delete.
        """)

class AtomicRename(Feat):
    """Users can rename (move) files and directories with standard
    `mv` semantics, including overwriting existing files and empty
    directories."""

    deps = [RenamesEntries, RejectsInvalidRenames, SupportsGitInit]

    def title(self):
        return "Atomic Rename"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "A rename moves one dentry in constant time with no name "
            "matching (Bug 2), commits the source's buffered writes first "
            "(Bug 3), takes the rename lock for directory moves, and "
            "honours `RENAME_NOREPLACE` and `RENAME_EXCHANGE`. Covered by "
            "`tests/storage.rs`, the model-conformance suite, "
            "`test_rename.sh`, `test_posix.sh` and the git workload."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will replace a file atomically the way editors and
            package managers do, swap two names, and move a directory. We need a
            mount.

            ## Step 1: write-then-rename

            ```bash
            cd testdata/mnt
            echo "version 1" > config
            echo "version 2" > config.tmp
            mv config.tmp config
            cat config
            ```

            Readers see either version 1 or version 2, never a mix; after a crash
            the new name never points at a file missing its writes.

            ## Step 2: swap two names

            ```bash
            echo a > left; echo b > right
            mv --exchange left right
            cat left right
            ```

            ## Step 3: move a large directory

            ```bash
            mkdir -p big/sub && for i in $(seq 1 1000); do : > big/sub/f$i; done
            time mv big moved
            ```

            The move changes one row, so it takes as long as moving an empty
            directory.
        """)

    def how_to(self):
        return md(r"""
            ## Replace a file atomically

            Write a temporary file in the same directory and `rename` it over the
            target. No fsync is needed for atomicity: pgfs commits the temporary
            file's writes before the rename.

            ## Refuse to replace

            ```bash
            mv --no-clobber src dst       # RENAME_NOREPLACE: fails if dst exists
            ```

            ## Swap two entries

            ```bash
            mv --exchange a b             # RENAME_EXCHANGE
            ```
        """)

    def reference(self):
        return md(r"""
            ## Outcome matrix

            | Source | Target | Result |
            |---|---|---|
            | file | absent | moved |
            | file | file | target atomically replaced (deleted, or orphaned if open) |
            | file | directory | EISDIR |
            | directory | absent | moved, descendants untouched |
            | directory | empty directory | target atomically replaced |
            | directory | non-empty directory | ENOTEMPTY |
            | directory | inside itself | EINVAL |
            | either | the same inode | success, nothing changes |

            ## Flags

            | Flag | Behaviour |
            |---|---|
            | `RENAME_NOREPLACE` | EEXIST if the target exists |
            | `RENAME_EXCHANGE` | swap; ENOENT if either is missing |
            | `RENAME_WHITEOUT`, unknown | EINVAL |
        """)

    def explanation(self):
        return md(r"""
            ## One row, no patterns

            A rename moves one dentry row and, at most, removes one replaced
            target. No name matching is involved, which is what makes Bug 2
            impossible: the first schema found descendants with `LIKE`, so `_` and
            `%` in names acted as wildcards.

            ## Why buffered writes go first

            A program that writes a file and renames it expects the new name to
            show the writes. pgfs commits the file's pending writes before the
            rename's own transaction, and PostgreSQL's WAL keeps that order through
            a crash (Bug 3 was a rename that lost them).

            ## Why directory moves take a lock

            Two concurrent moves of directories into each other could form a
            cycle. A move to a different parent takes a per-layer lock and checks
            that the destination is not inside the source, in the same
            transaction.
        """)

class ReliableMount(Feat):
    """The filesystem mounts and unmounts cleanly. Crashing or killing
    the daemon never leaves a ghost mount behind."""

    deps = [
        MountsCleanly,
        HandlesShutdownSignals,
        WritesHealthFiles,
        ProvidesLifecycleScript,
        BoundsEveryWait,
        DatabaseStateMachine,
        ReconnectsWithBackoff,
        RevalidatesAfterReconnect,
        IdempotentMutations,
        PreservesBuffersAcrossReconnect,
        RetriesSerializationFailures,
        HonoursInterrupt,
        InFlightAgeWatchdog,
        SlowDatabaseSignal,
        MapsDatabaseErrors,
    ]

    def title(self):
        return "Reliable Mount & Shutdown"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "AutoUnmount, clean shutdown that commits every buffer and "
            "runs the synchronous barrier, health files, bounded waits, "
            "the connection state machine with reconnect and revalidation "
            "(Bug 5), op_log idempotency, serialization retries, the "
            "in-flight watchdog, the slow-database signal and the "
            "SQLSTATE table are built and tested. The fuser 0.14 crate "
            "never delivers FUSE_INTERRUPT to the filesystem, so "
            "`failure.HonoursInterrupt` is limited to the watchdog's "
            "cancels."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will stop the database under a mounted filesystem
            and watch the mount recover, then shut it down cleanly. We need a mount
            started with `./scripts/pgfs.sh up`.

            ## Step 1: write something

            ```bash
            echo before > testdata/mnt/f
            ```

            ## Step 2: restart PostgreSQL

            ```bash
            pg_ctl -D testdata/pgdata -m fast restart -l testdata/postgres.log -o "-k $PWD/testdata -h 127.0.0.1"
            echo after >> testdata/mnt/f
            cat testdata/mnt/f
            grep 'database state' testdata/pgfs.log | tail -4
            ```

            ```text
            INFO pgfs::health: database state Connected -> Broken: ...
            INFO pgfs::health: database state Broken -> Reconnecting
            INFO pgfs::health: database state Reconnecting -> Revalidating
            INFO pgfs::health: database state Revalidating -> Connected: revalidation passed
            ```

            The append that found the connection broken waited for the mount to
            reconnect and revalidate, then committed.

            ## Step 3: shut down cleanly

            ```bash
            ./scripts/pgfs.sh down
            grep -E 'committing buffers|flushed to WAL|session released' testdata/pgfs.log | tail -3
            ```

            SIGTERM commits every buffer, runs the synchronous barrier, unmounts,
            and releases the lease.
        """)

    def how_to(self):
        return md(r"""
            ## Take over a tree from a crashed daemon

            ```bash
            pgfs mount --takeover main /srv/pgfs
            ```

            The new mount waits for the old lease to expire (`--lease`, default
            30 s), then acquires it and reclaims the old mount's orphans.

            ## Find a stuck request

            The watchdog logs a WARN past `--stall-warn` (5 s) naming the worker,
            operation, inode and backend pid, and past `--stall-error` (60 s)
            writes a state dump and cancels the query. Match the pid in
            `pg_stat_activity`.

            ## Run under systemd

            Install `contrib/pgfs@.service`; the daemon sends `READY=1`,
            `STOPPING=1` and `WATCHDOG=1` while no request is stalled.
        """)

    def reference(self):
        return md(r"""
            ## Startup

            1. Connect with the bounded waits below; check the server version,
               durability settings and schema version.
            2. On a scratch filesystem emptied by a crash, run
               `pgfs.scratch_reset()`.
            3. Acquire the lease (read-write) or register the session (read-only).
            4. Reclaim orphans; open one connection per worker; mount with
               `AutoUnmount` and `default_permissions`.
            5. Write the pid and ready files; notify systemd.

            ## Bounded waits

            | Setting | Value |
            |---|---|
            | `connect_timeout` | 5 s |
            | `statement_timeout` | 30 s (housekeeping 60 s) |
            | `lock_timeout` | 5 s |
            | `idle_in_transaction_session_timeout` | 60 s |
            | TCP keepalives | 10 s idle, 5 s interval, 3 probes; `tcp_user_timeout` 30 s |

            ## Connection states

            | State | New requests |
            |---|---|
            | Connected | served |
            | Broken, Reconnecting, Revalidating | EIO at once; the in-flight request waits for revalidation |
            | Fenced | EIO until remount |

            ## Exit status

            | Status | Meaning |
            |---|---|
            | 0 | clean unmount, every buffer committed |
            | 1 | the run ended but some buffered data could not be committed |
            | 2 | could not start (error chain printed as `pgfs: …`) |
        """)

    def explanation(self):
        return md(r"""
            ## Fail fast, recover by revalidating

            When the database goes away, a mount that queues requests builds a
            backlog that keeps it overloaded after the database returns. pgfs
            fails new requests at once with EIO, lets only the request in flight
            wait, and reconnects with jittered backoff. Before serving again it
            checks that the database is the same one with the same history: the
            system identifier, the timeline, the schema and `fs_uuid`, its own
            lease row, and that every worker's op_log slot holds at least the last
            mutation it acknowledged. If an acknowledged commit was lost, the
            mount fences itself rather than serve a rewound filesystem.

            ## A watchdog that can see a stuck worker

            A heartbeat thread can only notice that the heartbeat thread is stuck.
            pgfs records each worker's in-flight request in an atomic slot, and a
            separate thread watches the slots' ages, so a worker stuck in a query
            is reported, dumped and cancelled, never turned into a lost mount.
        """)

class ObservableDaemon(Feat):
    """Operators can see what the daemon is doing, how fast it is doing
    it, and why something failed, from the log alone: structured spans
    for every operation, periodic metrics, an on-demand state dump, and
    self-contained error chains and crash reports."""

    deps = [
        UsesStructuredTracing,
        InstrumentsFuseCallbacks,
        InstrumentsDbOperations,
        DistinguishesExpectedErrors,
        ExportsPeriodicMetrics,
        DumpsStateOnSigusr1,
        WritesHealthFiles,
        ReportsCrashes,
    ]

    def title(self):
        return "Observability & Diagnostics"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "Tracing with spans per callback (worker and view included) "
            "and per database call (statement, inode, backend pid, round "
            "trip), the nine 60-second metrics lines with every errno "
            "counted where it is replied, the SIGUSR1 dump written by the "
            "signal thread from atomics, and crash reports all ship. A "
            "test keeps the instrumented callback list in step with the "
            "`Filesystem` impl."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will read the daemon's log, metrics and state dump
            while it works. We need a mount started with `./scripts/pgfs.sh up`.

            ## Step 1: watch spans

            ```bash
            pkill -f 'pgfs mount main' ; RUST_LOG=pgfs=debug ./target/debug/pgfs mount main testdata/mnt 2> testdata/obs.log &
            sleep 1; ls testdata/mnt > /dev/null
            grep 'fuse{op="getattr"' testdata/obs.log | head -2
            ```

            Every callback runs in a `fuse` span with its operation, inode, worker
            and view; database calls nest inside it as `db` spans with the
            statement, backend pid and round-trip time.

            ## Step 2: ask for a state dump

            ```bash
            kill -USR1 "$(pgrep -f 'pgfs mount main testdata/mnt')"
            grep 'state|' testdata/obs.log | tail -12
            ```

            The signal thread writes the dump itself, so it appears even when
            every worker is stuck.

            ## Step 3: read the metrics

            Every 60 s the daemon logs nine `metrics|…` lines: operations, errnos,
            replica, FUSE latency, database latency, connection state, in-flight
            age, space and integrity.
        """)

    def how_to(self):
        return md(r"""
            ## Get JSON logs

            ```bash
            RUST_LOG_FORMAT=json pgfs mount main /srv/pgfs 2> pgfs.jsonl
            ```

            ## Turn up one module

            ```bash
            RUST_LOG=pgfs=info,pgfs::pg=debug pgfs mount main /srv/pgfs
            ```

            ## Match a slow span to the database

            The `db` span records the backend pid; look it up in
            `pg_stat_activity`.
        """)

    def reference(self):
        return md(r"""
            ## Log levels

            | Level | Used for |
            |---|---|
            | ERROR | unexpected failures with their full chain, fencing, crash reports, checksum mismatches |
            | WARN | degraded but working: retries, timeouts with their thresholds, slow database, standby fallbacks |
            | INFO | lifecycle, state transitions, metrics, state dumps |
            | DEBUG | expected errnos, per-operation detail |

            ## Metrics lines (every 60 s)

            | Line | Fields |
            |---|---|
            | `metrics\|ops\|…` | one counter per callback |
            | `metrics\|errors\|…` | one counter per errno in the catalogue |
            | `metrics\|replica\|…` | reads, fallbacks, lag_bytes, lag_ms, ticket_waits |
            | `metrics\|latency\|fuse=…` | p50 / p95 / p99 of callbacks |
            | `metrics\|latency\|db=…` | p50 / p95 / p99 of database calls |
            | `metrics\|db\|…` | state, reconnects, fenced, slow_db |
            | `metrics\|inflight\|oldest_ms=…` | age of the oldest in-flight request |
            | `metrics\|space\|…` | live, physical, inline and snapshot-only bytes; backlog; dead tuples; WAL per user byte |
            | `metrics\|integrity\|checksum_failures=…` | checksum failures |

            ## State dump (SIGUSR1)

            `state|mount=… view=… fence=… layer=… db=… uptime_secs=…`, one
            `state|worker=N` line per worker (operation, inode, age, backend pid),
            `state|cache=… handles=… buffered_bytes=…`, the replica line, and the
            latency summaries. Any lock that cannot be taken at once prints
            `locked`.

            ## Threads

            | Thread | Role |
            |---|---|
            | `worker-N` | dispatch workers, one database connection each |
            | `renewer` | lease renewal and self-fencing |
            | `reconnector` | reconnect and revalidation |
            | `housekeeping` | usage folding, reclaim, GC, change pruning and notifications, auto-flatten |
            | `control` | owner-drained snapshots and branches |
            | `watchdog` | in-flight ages, stall warnings and cancels, systemd watchdog |
            | `signal-waiter` | SIGINT/SIGTERM/SIGHUP shutdown, SIGUSR1 dump, SIGUSR2 profiling |
            | `metrics-export` | the 60 s metrics lines |
        """)

    def explanation(self):
        return md(r"""
            ## Everything an operator needs is in the log

            FUSE can only return an errno to the program; the reason for an EIO
            lives in the daemon. pgfs therefore logs every unexpected failure with
            its whole error chain, keeps expected errnos out of the log, and counts
            both, so a spike in "file not found" can be told apart from a database
            failing.

            ## Signals that predict failure

            Counting what happened is not enough: slow failures matter more. The
            metrics include the age of the oldest request, the connection state,
            a slow-database gauge, replica lag, the reclaim backlog and write
            amplification, so trouble shows before it becomes an outage.
        """)
