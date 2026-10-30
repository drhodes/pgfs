# Reference: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete run on the inode-keyed schema: persistent inode numbers with generations, buffered writes visible to every handle on the mount, read-modify-write of partial blocks (Bug 1), single-transaction unlink and checksummed immutable blocks. Covered by `tests/storage.rs`, the model-conformance suite and `scripts/test_*.sh`.

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

[Tutorial](../tutorials/flat_filesystem.md) · [How-to](../how-to/flat_filesystem.md) · **Reference** · [Explanation](../explanation/flat_filesystem.md)

---

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


## Specification requirements

`FlatFilesystem` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "ConnectsToPostgres"
    `spec.app.ConnectsToPostgres`

    The daemon connects to Postgres with a libpq connection string
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

??? abstract "CreatesFiles"
    `spec.app.CreatesFiles`

    A FUSE `create` (or `mknod` of a regular file) calls
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

??? abstract "ReadsFiles"
    `spec.app.ReadsFiles`

    A FUSE `read` returns bytes `[offset, min(offset + size,
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

??? abstract "WritesFiles"
    `spec.app.WritesFiles`

    A FUSE `write` places `data` at `offset` in a regular file. Small
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

??? abstract "TruncatesFiles"
    `spec.app.TruncatesFiles`

    A FUSE `setattr` with a size truncates or extends a regular file.
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

??? abstract "UnlinksFiles"
    `spec.app.UnlinksFiles`

    A FUSE `unlink` calls `pgfs.unlink`, which in one transaction
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

??? abstract "LooksUpEntries"
    `spec.app.LooksUpEntries`

    A FUSE `lookup` of `(parent ino, name)` returns the child's
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

??? abstract "ReportsAttributes"
    `spec.app.ReportsAttributes`

    A FUSE `getattr` returns kind, mode, nlink, uid, gid, rdev, size,
    `blocks` (512-byte units of stored data, holes excluded), `blksize`
    (the block size), and atime, mtime, ctime and birth time in
    nanoseconds. Size and mtime include writes still buffered on the
    mount (`consistency.DrainsBuffersBeforeExternalising`). The reply's
    TTL follows `consistency.KernelCacheContract`. Inode 1 is the root
    directory of the mounted view.

    **Postcondition.** The values reflect every operation that replied
    before the request.

??? abstract "DistinguishesExpectedErrors"
    `spec.app.DistinguishesExpectedErrors`

    Expected conditions reply their errno to the kernel and are logged
    at DEBUG at most. These are the errnos in `posix.ErrnoCatalogue`
    other than EIO. Unexpected failures are logged in full through
    `log_and_reply!`: database errors that map to EIO, checksum
    failures, invariant violations and fencing. Database errors are
    classified by SQLSTATE in one table (`failure.MapsDatabaseErrors`),
    never by matching message text.

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
