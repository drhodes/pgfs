# Reference: Consistency and Mount Ownership

!!! success "Status: Stable"
    One read-write lease per tree with fencing, read-only live mounts, the kernel cache lifetimes, one transaction per operation, bounded transactions with the rest handed to the reclaimer, and housekeeping in its own transactions are built and tested (`tests/storage.rs`, `test_lease.sh`). A commit of a partly written block reads that block first, because only the daemon can checksum the new content.

> Within a mount, every operation sees every earlier one. Each tree has at most one read-write mount, which holds a lease that a stalled daemon cannot outlive. Every operation is one bounded, atomic transaction.

[Tutorial](../tutorials/consistentmounts.md) · [How-to](../how-to/consistentmounts.md) · **Reference** · [Explanation](../explanation/consistentmounts.md)

---

## Mount kinds

| Mount | Session in `mounts` | Sees |
|---|---|---|
| read-write | exclusive lease per tree (`read_only = false`) | everything, immediately |
| `--read-only` live | shared session (`read_only = true`) | committed state, up to `--attr-ttl` late |
| snapshot | shared session (`read_only = true`) | a frozen state |

## Options

| Option | Default | Effect |
|---|---|---|
| `--lease` | 30 s | lease period; renewed every third |
| `--takeover` | off | wait for an expired lease and take it |
| `--attr-ttl` | 3 600 s read-write, 1 s read-only | kernel entry and attribute TTL |

## Errors

| Error | When |
|---|---|
| `EBUSY` | the tree has a read-write mount |
| `EIO` | this daemon lost its lease; remount |


## Specification requirements

`ConsistentMounts` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "ConsistencyModel"
    `spec.consistency.ConsistencyModel`

    Operations through one mount are linearizable. Each operation
    takes effect at one instant between its request and its reply, and
    sees the effect of every operation that replied before it was sent,
    including writes still buffered in the daemon.

    Across mounts, a read-write mount of a tree and read-only mounts of
    the same live tree are related only by `ReadOnlyLiveMounts`. Mounts
    of snapshots never change. B (v0.2) will add close-to-open
    consistency between read-write mounts on different machines.

    **Sources.** Sprite r40, AFS 05, Skyros r129, SwitchΔ r133.

??? abstract "DrainsBuffersBeforeExternalising"
    `spec.consistency.DrainsBuffersBeforeExternalising`

    Before an operation reveals file state, it accounts for the
    pending writes in that inode's write buffer
    (`optimize.BufferedWriteFlushReq`). The operations are `read`,
    `getattr`, `lookup` (for size), `readdirplus`, `setattr`, `rename`,
    `link`, `copy_file_range`, `lseek` with `SEEK_DATA` or `SEEK_HOLE`,
    and `fsync`. Reads merge pending bytes over committed ones, and
    attribute replies report the size including pending writes. Operations
    that change the file's name or links commit the pending writes first
    (`durability.FlushesBeforeNaming`). A truncate trims the inode's
    pending writes beyond the new size, so a later flush cannot bring
    them back.

    **Postcondition.** No reply depends on which handle a write came
    through.

    **Sources.** Skyros r129 and SwitchΔ r133 (deferred state must be
    visible to every reader). This is the rule whose absence caused
    Bug 3, rename losing buffered writes.

??? abstract "ExclusiveTreeLease"
    `spec.consistency.ExclusiveTreeLease`

    Every mount — read-write, read-only live (`ReadOnlyLiveMounts`), or
    snapshot (`branch.MountsSnapshots`) — registers a session row in
    `mounts(mount_id uuid primary key, tree text, snapshot text,
    read_only boolean, fence bigint, host text, pid integer, acquired_at,
    lease_until)` through a `pgfs` SQL function on the primary
    (`--conn`). A partial unique index enforces at most one unexpired
    read-write row (`read_only = false`) per `tree`.

    For a read-write mount, acquiring or taking over the lease runs in
    one transaction with `SET LOCAL synchronous_commit = on`
    (`durability.DurabilityPoints`): it succeeds only if no unexpired
    read-write row for `tree` exists, increments `fence`, replaces any
    expired read-write row for `tree` (together with its `op_log` rows),
    and pre-inserts one `op_log` row per worker
    `(mount_id, worker_id, 0, NULL)` (`failure.IdempotentMutations`).
    Every mount renews its row every third of `--lease` (default 30 s) on
    the primary, and deletes its row (and `op_log` slots, for a
    read-write mount) with `synchronous_commit = on` at clean shutdown.

    Expired read-only and snapshot rows (`read_only = true`) are reaped
    by housekeeping (`HousekeepingInSeparateTransactions`). Read-write
    rows (`read_only = false`) and their `op_log` slots are never reaped
    by housekeeping; they are replaced only on `--takeover` or deleted on
    clean unmount or `pgfs tree delete`, so idempotency and the reconnect
    continuity check (`failure.RevalidatesAfterReconnect`) never expire
    during a long network outage.

    **Errors.** Mounting a tree read-write while its lease is held fails
    with EBUSY, naming the holder's host and pid. `--takeover` waits for
    the read-write lease to expire, then acquires it.

    **Invariant.** At most one unexpired read-write lease per tree.

    **Sources.** Chubby r04, Leases r21.

??? abstract "FencesStaleWriters"
    `spec.consistency.FencesStaleWriters`

    Every mutating SQL function receives the caller's `fence` and,
    holding the layer gate (`branch.EpochVersionedRows`), checks it
    against `mounts.fence`. A mismatch raises SQLSTATE `PF001`
    (`fenced`) and changes nothing.

    Every mount (read-write, read-only live, or snapshot) also fences
    itself when its session renewal lapses: if a renewal on the primary
    has not succeeded within the lease period minus a 5 s margin,
    measured on its monotonic clock from when the renewal was sent, a
    read-write mount stops sending mutations and any mount stops serving
    reads that the kernel cache cannot answer, replying EIO until the
    connection reconnects and passes `failure.RevalidatesAfterReconnect`.
    Failing closed on read-only and snapshot mounts prevents a mount
    reading from a standby while disconnected from the primary from
    reading rows after its session expired and `pgfs snapshot delete` or
    `pgfs tree delete` proceeded (`branch.DeletesSnapshotsAndTrees`).

    A permanently fenced daemon (`Fenced` state) replies EIO and logs
    ERROR once. If another mount took over the read-write lease, or if
    `mounts` no longer holds this mount's exact `(mount_id, fence)` row,
    or if the `op_log` continuity check fails, the daemon stays fenced
    until it is remounted (`failure.RevalidatesAfterReconnect`).

    **Invariant.** A daemon that has lost its lease commits nothing,
    however long it paused.

    **Sources.** Chubby r04 (sequencers), Delos r73 (seal, then switch),
    Nifty r71 (partial partitions).

??? abstract "ReadOnlyLiveMounts"
    `spec.consistency.ReadOnlyLiveMounts`

    `pgfs mount --read-only TREE` mounts a live tree with a shared
    read-only session in `mounts` (`ExclusiveTreeLease`), so `migrate`,
    `tune` and `tree delete` can detect it while any number of read-only
    mounts run alongside the read-write mount. Each serves the committed
    state, and may lag the writer by its attribute TTL (`--attr-ttl`,
    default 1 s for read-only live mounts). A file reopened after a
    change is read fresh: the open compares the inode's `version` and
    drops the page cache if it changed. A read-only live mount observes a
    `--force` promote of its tree only after a remount.

    Because a read-only live mount has no `op_log` and can observe
    asynchronous commits on the primary before their WAL reaches disk
    (`durability.AsyncCommitWindow`), on every reconnect
    (`failure.RevalidatesAfterReconnect`) it treats history as
    potentially rewound: it forgets every cached inode `version` (so the
    next open of each file drops its kernel page cache), sends
    `notify_inval_inode` for every nodeid the kernel holds, and clears
    its own metadata cache. Open file descriptors are protected against
    `ino` reuse by `inodes.gen` (`storage.PersistentInodes`).

    **Postcondition.** Staleness is bounded by the TTL for attributes
    and names, and by the last open for file contents. Nothing stronger
    is promised.

??? abstract "KernelCacheContract"
    `spec.consistency.KernelCacheContract`

    The daemon sets kernel cache lifetimes from what can change
    behind the kernel's back:

    | Mount | Entry and attribute TTL | Page cache on open |
    |---|---|---|
    | read-write (exclusive) | `--attr-ttl`, default 3 600 s | kept (`FOPEN_KEEP_CACHE`) |
    | read-only live | `--attr-ttl`, default 1 s | kept only if `version` is unchanged |
    | snapshot | unbounded | kept |

    A read-write mount can cache for a long time because every change
    to its tree passes through it. Negative lookups are cached for the
    same TTL. When the daemon itself changes something the kernel did
    not see, it sends `notify_inval_inode` or `notify_inval_entry`. An
    example is an orphan reclaimed at mount.

    **Sources.** To FUSE or Not r12, Leases r21, Chubby r04 (invalidate
    rather than expire), ExtFUSE r13 (negative caching).

??? abstract "ExternalSqlWritesUnsupported"
    `spec.consistency.ExternalSqlWritesUnsupported`

    Reading pgfs tables and views directly is supported
    (`storage.SqlReadViews`). Writing to them outside the `pgfs`
    functions is not: it bypasses the lease, the epochs and the kernel's
    caches. The runtime role cannot do it (`security.LeastPrivilegeRoles`),
    and `pgfs check` reports the invariants it breaks.

??? abstract "OneOperationOneTransaction"
    `spec.consistency.OneOperationOneTransaction`

    Each FUSE operation that changes state is exactly one database
    transaction, and that transaction contains only that operation. The
    operations include `create`, `mkdir`, `mknod`, `symlink`, `link`,
    `unlink`, `rmdir`, `rename`, `setattr`, `setxattr`, `removexattr`,
    `fallocate`, `copy_file_range`, a buffer flush, and `release` of an
    orphan. No operation commits part of its effect.

    **Crash.** A crash leaves each operation either wholly applied or
    not applied at all (`durability.PrefixCrashConsistency`).

    **Sources.** WOFS r61, DaisyNFS r68, Oracle FSS r91.

??? abstract "BoundedTransactions"
    `spec.consistency.BoundedTransactions`

    No filesystem operation's transaction grows with the size of the
    tree. A rename changes one dentry row, plus at most one replaced
    target and two parent inodes. A truncate's transaction is bounded by
    the number of blocks it removes, in batches of at most 4 096 blocks.
    Larger truncates and unlinks hand the remaining blocks to the
    reclaimer (`HousekeepingInSeparateTransactions`). Snapshot, branch
    and `--force` promote are constant-size; a normal promote folds the
    branch's delta into the target tree's layer in `O(|delta|)` rows,
    with no read-write mount open on either tree (`branch.PromotesBranch`).

    **Bounds.** At most a few thousand rows per foreground filesystem
    transaction, whatever the size of the tree or the file; `O(|delta|)`
    for a normal promote.

    **Sources.** Oracle FSS r91. The first pgfs schema broke this rule:
    a directory rename rewrote every descendant row in one transaction.

??? abstract "OneRoundTripPerOperation"
    `spec.consistency.OneRoundTripPerOperation`

    Each filesystem operation sends its statements to Postgres in one
    round trip: a single call to a `pgfs` function
    (`storage.OperationsAsSqlFunctions`) or one prepared statement. The
    daemon never reads a row only to decide which statement to send
    next, when one statement can decide.

    **Bounds.** One network round trip per operation, plus one retry
    when a concurrent operation swaps an inode with uncommitted dirty
    buffers onto a `rename` or `unlink` target
    (`concurrency.InodeShardedWorkers`), and one read of a block that a
    commit, truncate, hole punch or copy covers only partly: the daemon
    must checksum the block's new content (`integrity.BlobChecksums`),
    which no single statement can do.

    **Sources.** r82, Lockify r41.

??? abstract "HousekeepingInSeparateTransactions"
    `spec.consistency.HousekeepingInSeparateTransactions`

    Background work runs in its own small transactions, never inside
    a user operation:

    - reclaiming retired rows (`space.ReclaimsRetiredRows`);
    - collecting shared blobs (`space.CollectsSharedBlobs`);
    - removing the blocks of large deleted files;
    - compacting usage deltas;
    - pruning `changes`, emitting coalesced `NOTIFY pgfs_changes` hints
      (`changefeed.ChangeNotifications`), and reaping expired read-only
      and snapshot rows in `mounts` (read-write lease rows and their
      `op_log` slots are never reaped by housekeeping,
      `ExclusiveTreeLease`);
    - flattening branches (`branch.FlattensBranch`).

    Each piece of work is idempotent, so a crash part-way through is
    repaired by running it again.

    **Concurrency.** Housekeeping takes the layer gate shared, like any
    mutation, and yields to foreground work through a fixed rate limit.

    **Sources.** Oracle FSS r91, Dayu r95 (rate-limited background
    movement).

---

<small>Generated from `spec.consistency.ConsistentMounts` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
