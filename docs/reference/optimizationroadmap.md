# Reference: Performance Optimizations

!!! warning "Status: Partially implemented"
    Inode-sharded workers with their own connections, the lock order, the rename serialisation point, cross-worker flush by message, bounded buffers, prepared statements and block-granular buffered writes ship. `make bench` runs portable workloads; fio, mdtest and the passthrough-FUSE ratio run only where installed, and the CI regression gate is not built. The writeback cache stays off by design.

> The request path from the kernel to PostgreSQL is made fast where it costs most: concurrent dispatch sharded by inode, prepared statements, buffered writes and, later, the kernel's writeback cache. A fixed benchmark suite measures every change.

[Tutorial](../tutorials/optimizationroadmap.md) · [How-to](../how-to/optimizationroadmap.md) · **Reference** · [Explanation](../explanation/optimizationroadmap.md)

---

## Options

| Option | Default | Effect |
|---|---|---|
| `--workers` | CPUs, at most 16 | inode-sharded dispatch workers |
| `--buffer-per-file` | 8 MiB | commit a file's buffer at this size |
| `--buffer-total` | 256 MiB | commit the oldest buffers past this total |
| `--attr-ttl` | 3 600 s read-write, 1 s read-only | kernel entry and attribute TTL |

## Status

| Requirement | State |
|---|---|
| `InodeShardedWorkers`, `WorkerOwnsConnection`, `NoLockAcrossRoundTrip` | built |
| `LockOrder`, `RenameSerialisationPoint` | built, in the SQL functions |
| `BoundedWriteBuffers`, `BufferedWriteFlushReq`, `PreparedStatementReq` | built |
| `KernelWritebackCacheReq` | off by design until its preconditions are covered by crash tests |
| `BenchmarkSuite` | portable workloads in `scripts/bench.sh`; fio, mdtest and the passthrough ratio where installed |
| `BenchmarkRegressionGate` | not built |


## Specification requirements

`OptimizationRoadmap` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "InodeShardedWorkers"
    `spec.concurrency.InodeShardedWorkers`

    The daemon runs `--workers` dispatch workers (default: the number
    of CPUs, at most 16). Every request on an inode (`getattr`,
    `setattr`, `open`, `read`, `write`, `flush`, `release`, `fsync`,
    `readdir`, `readdirplus`, `*xattr`) goes to the worker that owns
    its inode, `ino mod workers`. That worker owns the inode's open
    handles and write buffers.

    The daemon's metadata cache (inode attributes, dentries, and an
    atomic per-inode summary `(buffered_size, buffered_mtime_ns,
    open_handles)`) is shared across workers so `getattr` and
    `readdirplus` read buffered sizes without cross-worker round trips
    (`consistency.DrainsBuffersBeforeExternalising`), and the
    change-feed listener thread (`changefeed.ChangeNotifications`) can
    evict entries directly.

    A request on a single name (`lookup`, `create`, `mkdir`, `mknod`,
    `symlink`, `link`, `unlink`, `rmdir`) goes to `hash(parent_ino,
    name) mod workers` (using `(newparent_ino, newname)` for `link`), so
    operations on the same name are serialised while concurrent creates
    or unlinks of different names in one directory fan out across all
    workers. A request on two names (`rename`) goes to the worker of the
    lower `(parent_ino, name)` hash. `copy_file_range` goes to `ino_out
    mod workers`. Before starting a `rename`, `link`, `unlink` or
    `copy_file_range` transaction, the worker checks the daemon's
    metadata cache for the affected inode(s) (`source_ino` and any
    overwritten `target_ino`, `link`'s `ino`, or `ino_in`) and, if any
    has buffered writes on another worker, messages that worker (`ino
    mod workers`) to commit its buffer first (`NoLockAcrossRoundTrip`).
    In addition, `pgfs.rename` and `pgfs.unlink` take the set of inodes
    that currently have dirty buffers anywhere on the mount (from the
    shared per-inode summary, or `NULL` when none are dirty): after
    `LockOrder` step 3, if a resolved `source_ino` or `target_ino` is in
    that dirty set and was not pre-flushed (for example because a cache
    miss skipped pre-resolution or another worker concurrently renamed a
    dirty file onto the target name), the SQL function returns
    `retry: inos = …` without committing, and the worker flushes those
    inodes and calls again (`consistency.OneRoundTripPerOperation`).

    **Postcondition.** Requests on one inode, or on one `(parent_ino,
    name)`, are applied in the order the kernel sent them.

    **Concurrency.** No lock protects per-inode write buffers or handle
    tables, because one worker thread owns each inode's mutable state.

    **Sources.** uFS r130, To FUSE or Not r12, RFUSE r17.

??? abstract "WorkerOwnsConnection"
    `spec.concurrency.WorkerOwnsConnection`

    Each worker owns one database connection, with its prepared
    statements (`PreparedStatementReq`), for its whole life. Background
    tasks (lease renewal, housekeeping, the change-feed listener) have
    connections of their own. The total on the primary is `workers + 3`
    (plus `workers + 1` standby connections when `--replica` is enabled,
    `replica.ServesReadsFromReplica`), and the startup log reports it.

    **Bounds.** Connections per mount are fixed and stated. A mount
    never opens connections on demand.

??? abstract "LockOrder"
    `spec.concurrency.LockOrder`

    Inside a transaction that locks several rows, rows are locked in
    this order:

    1. the layer gate, shared (`branch.EpochVersionedRows`);
    2. the rename lock, for directory renames across parents
       (`RenameSerialisationPoint`);
    3. inode rows, in ascending `ino`;
    4. dentry rows, in ascending `(parent_ino, name)`;
    5. block map rows, in ascending `(ino, block_no)`;
    6. blob rows, in ascending `blob_id`.

    Functions that need a row later in this order never wait for one
    earlier in it. Because name-based operations (`unlink`, `rmdir`,
    `rename`) do not know the child or overwritten target `ino`
    beforehand, they execute a two-phase discover-sort-lock-revalidate
    protocol (Frangipani 12, HopsFS r11, Chardonnay r66):

    1. **Discover:** read the relevant `dentries` row(s) without
       `FOR UPDATE` (and for `create`/`mkdir`, allocate the new `ino`
       from the sequence first) to learn all participating inode
       numbers (`parent_ino`, `new_parent_ino`, `child_ino`,
       `target_ino`).
    2. **Lock inodes:** sort all participating existing inode numbers in
       ascending `ino` order and lock their `inodes` rows with
       `SELECT … FOR UPDATE`.
    3. **Lock and revalidate dentries:** lock or insert the `dentries`
       row(s) in ascending `(parent_ino, name)` order and verify that
       each resolved `(parent_ino, name)` still maps to the `ino`
       discovered in step 1 (if a concurrent operation changed it, abort
       and retry from step 1).

    Housekeeping follows the same order.

    **Invariant.** No deadlock between pgfs transactions. One between a
    pgfs transaction and an external client is resolved by Postgres and
    retried (`failure.RetriesSerializationFailures`).

    **Sources.** Frangipani 12, HopsFS r11, RefFS r65, Chardonnay r66,
    LockFS r133.

??? abstract "RenameSerialisationPoint"
    `spec.concurrency.RenameSerialisationPoint`

    A rename that moves a directory to a different parent takes a
    transaction-level advisory lock for the layer, then checks that the
    destination is not inside the source by walking the destination's
    ancestors. The check and the update are in one transaction, so two
    concurrent renames cannot form a cycle. Renames within one directory,
    and renames of non-directories, do not take this lock.

    **Errors.** EINVAL if the destination is inside the source.

    **Bounds.** The ancestor walk is one recursive query, proportional
    to the destination's depth.

    **Sources.** InfiniFS r16, Mantle r127.

??? abstract "NoLockAcrossRoundTrip"
    `spec.concurrency.NoLockAcrossRoundTrip`

    No daemon-side lock or guard is held while a database query runs.
    Work that needs other workers' state, for example commits of their
    buffers before a rename, is requested by message and awaited before
    the transaction starts. While a worker waits on another worker, it
    keeps servicing incoming commit-buffer requests from its peer
    mailbox, never new FUSE requests (serving new FUSE requests would
    reorder requests on one inode and make the wait re-entrant; handling
    a commit-buffer request never sends a cross-worker message, so wait
    chains have depth 1 and cannot form a cycle).

    **Sources.** CetoFS r42, Chardonnay r66 (read before you lock).

??? abstract "BoundedWriteBuffers"
    `spec.concurrency.BoundedWriteBuffers`

    Per-inode write buffers are bounded. A buffer is committed when
    it holds `--buffer-per-file` (default 8 MiB). When the mount's total
    reaches `--buffer-total` (default 256 MiB), the oldest buffers are
    committed before new writes are accepted (workers awaiting room
    continue servicing peer commit-buffer messages so two workers under
    buffer pressure cannot deadlock, `NoLockAcrossRoundTrip`). A write
    waits for that room rather than growing memory without limit.

    **Bounds.** Buffered data on a mount never exceeds the total plus
    one write.

    **Sources.** ScaleCheck r57.

??? abstract "PreparedStatementReq"
    `spec.optimize.PreparedStatementReq`

    Every single-statement read the daemon issues (lookup, getattr,
    readdir pages, block reads, xattr reads) and every call to a `pgfs`
    function is prepared once per connection, when the worker opens it
    (`concurrency.WorkerOwnsConnection`). The hot path never sends SQL
    text to be parsed and planned. Because each worker owns its
    connection for life, a statement is never prepared lazily, and never
    prepared twice.

    **Bounds.** Parsing and planning happen once per statement per
    connection.

??? abstract "BufferedWriteFlushReq"
    `spec.optimize.BufferedWriteFlushReq`

    Writes are buffered in a single per-inode write buffer owned by
    the inode's worker (`concurrency.InodeShardedWorkers`). Every write
    to an inode advances a per-inode monotonic `write_seq` and records
    the handle's `last_write_seq` and `seen_err_seq` (`errseq_t`,
    `durability.FsyncFailureSemantics`). The inode buffer is committed
    as a unit by `flush`, `release`, `fsync`, any operation that names
    the file (`consistency.DrainsBuffersBeforeExternalising`), and
    buffer pressure (`concurrency.BoundedWriteBuffers`). A write of a
    whole aligned block, or an `O_SYNC` / `O_DSYNC` / `O_DIRECT` write,
    commits any overlapping dirty buffered data (or drains the inode
    buffer first) so an earlier buffered byte can never overwrite a
    later direct or aligned write (`durability.SyncHandles`).

    Buffers are block-granular, at the filesystem's block size
    (`storage.FixedBlockSize`), with dirty byte intervals (or a dirty
    bitmap) per block. Committing a partly dirty block reads the
    committed block first and overlays the dirty intervals on it. A
    block with nothing committed starts as zeros. This read-modify-write
    rule is the fix for Bug 1.

    **Postcondition.** After commit, the file equals the committed
    content with every buffered write applied in arrival order, across
    all open descriptors on the inode.

    **Concurrency.** Every read, getattr and listing on the mount sees
    the buffered data (`consistency.DrainsBuffersBeforeExternalising`).

    **Errors.** A failed commit keeps the uncommitted data in the inode
    buffer and reports once per descriptor that wrote (`last_write_seq`)
    via `errseq_t` (`durability.FsyncFailureSemantics`).

    **Sources.** Skyros r129, r90.

??? abstract "KernelWritebackCacheReq"
    `spec.optimize.KernelWritebackCacheReq`

    The kernel writeback cache (`FUSE_WRITEBACK_CACHE`) is the largest
    single win on the data path (To FUSE or Not r12). It stays off
    (`posix.WritebackCacheDisabled`) until each of these holds and is
    covered by a crash test:

    - mtime and size are owned by the kernel while the cache is on, and
      the daemon accepts them from setattr;
    - writes that arrive after `release`, from pages the kernel flushes
      later, are accepted and committed;
    - `O_APPEND` positioning is left to the kernel;
    - `fsync` covers pages still in the kernel, because the kernel
      flushes them before calling the daemon;
    - `CrashConformance` runs a matrix with the cache on.

    Splice for reads and writes is negotiated with the cache.

    **Sources.** To FUSE or Not r12, r77.

??? abstract "BenchmarkSuite"
    `spec.perf.BenchmarkSuite`

    `make bench` runs a fixed set of named workloads on a fresh
    filesystem, and writes the results as JSON with the commit, the
    machine and the Postgres version:

    | Workload | Measures |
    |---|---|
    | fio sequential read and write, 1 MiB | data-path throughput |
    | fio random 4 KiB read and write | small I/O and read-modify-write |
    | mdtest create, stat, unlink, 100k files | metadata operations per second |
    | `git clone`, then `git status`, of a pinned repository | real tool workload (Bento r53) |
    | untar of a pinned Linux kernel tarball | small-file creation (IndexFS r09) |
    | snapshot, branch, promote and diff on a 100k-file tree | the killer feature's constant-time claim |

    Each workload also runs on a passthrough FUSE filesystem over ext4
    on the same machine, and results are reported as a ratio to it.
    Comparing with ext4 directly would charge pgfs for FUSE itself
    (Bento r53). Every run records WAL bytes per byte of user data
    (`space.ReportsSpace`) and the p50 and p99 latency of each
    operation.

    **Sources.** Bento r53, IndexFS r09, To FUSE or Not r12.

??? abstract "BenchmarkRegressionGate"
    `spec.perf.BenchmarkRegressionGate`

    CI runs `BenchmarkSuite` on a fixed runner and compares each
    result with the stored baseline. A workload more than 10% slower,
    or with a write amplification more than 10% higher, fails the run,
    unless the commit updates the baseline with a stated reason.
    Absolute targets are written into the baseline after the first run
    of the inode-keyed design.

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
