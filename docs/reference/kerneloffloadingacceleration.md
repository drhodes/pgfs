# Reference: Kernel Offloading

!!! warning "Status: Partially implemented"
    `copy_file_range` maps whole blocks to the source's blobs (shared, collected by GC) and copies only partial edge blocks, in one bounded transaction; `fallocate` extends, punches holes and zeroes ranges by changing map rows. Content search is planned.

> Copies and hole punching run inside PostgreSQL. `cp` shares blocks instead of copying bytes, and `fallocate` punches holes and zeroes ranges by changing block-map rows, so no file data passes through the daemon. Content search through database indexes is planned.

[Tutorial](../tutorials/kerneloffloadingacceleration.md) · [How-to](../how-to/kerneloffloadingacceleration.md) · **Reference** · [Explanation](../explanation/kerneloffloadingacceleration.md)

---

## `copy_file_range`

| Case | What happens |
|---|---|
| destination blocks fully covered, `off_in ≡ off_out (mod block size)` | the destination map rows point at the source's blobs, which are marked `shared` |
| partly covered head and tail blocks, or misaligned offsets | read, modify and written as new blobs by the daemon |
| holes in the source | holes in the destination |
| either file inline | the inline bytes move to block 0 when the copy crosses `inline_max` |
| ranges overlapping in one file | EINVAL |
| more than 4 096 blocks | one batch per call; the reply is the bytes copied |

Pending writes of both files are committed first, and the whole call
is one transaction (`pgfs.copy_range`).

## `fallocate`

| Mode | Effect |
|---|---|
| `0` | `size = max(size, offset + length)`; nothing is reserved, so a later write can still fail with ENOSPC |
| `FALLOC_FL_KEEP_SIZE` | nothing |
| `FALLOC_FL_PUNCH_HOLE \| FALLOC_FL_KEEP_SIZE` | map rows inside the range removed (hole rows on a branch), edge blocks rewritten with the range zeroed |
| `FALLOC_FL_ZERO_RANGE` (± `KEEP_SIZE`) | as punch hole, and extends the size unless `KEEP_SIZE` |
| anything else | EOPNOTSUPP |

## Content search

Planned (`ContentSearchOffloadReq`).


## Specification requirements

`KernelOffloadingAcceleration` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "CopyFileRangeReq"
    `spec.offload.CopyFileRangeReq`

    The FUSE `copy_file_range` callback copies a byte range between
    two files of the same tree inside PostgreSQL. `cp` (coreutils 9 and
    later), `sendfile` and `copy_file_range(2)` reach it. Pending writes
    of both files are committed first. Then, in one transaction, it:

    - if either file still stores its content in `inline_data`
      (`storage.InlinesSmallFiles`), updates `inline_data` in place when
      the destination remains at most `inline_max`, or promotes
      `inline_data` to block 0 first when the copy crosses `inline_max`
      or shares blocks;
    - maps each destination block that the range covers entirely, at
      the same offset within the block as in the source, to the
      source's blob, and marks that blob `shared`
      (`space.CollectsSharedBlobs`). No data is copied. This is a
      reflink in all but name; `cp --reflink=always` itself cannot reach
      pgfs (`posix.DeclaresUnsupportedOps`);
    - copies partly covered head and tail blocks, and ranges whose
      offsets differ within a block, by read-modify-write into new
      blobs, with the same rule as `WritesFiles`;
    - leaves holes in the source as holes in the destination;
    - updates the destination's size, mtime and ctime, and records the
      change.

    **Postcondition.** The destination range equals the source range
    byte for byte. Bytes of the destination outside the range are
    unchanged, as for `WritesFiles`.

    **Crash.** All or nothing: one transaction, bounded by batching
    (`consistency.BoundedTransactions`). A copy larger than one batch
    replies with the bytes copied so far, which `copy_file_range(2)`
    permits, and the caller continues.

    **Errors.** EXDEV across trees. EINVAL for overlapping ranges in one
    file. EISDIR, EBADF, EFBIG and ENOSPC as for writes.

    **Sources.** BetrFS clones r55. The earlier backend ran two
    statements outside a transaction, which
    `consistency.OneOperationOneTransaction` forbids.

??? abstract "FallocatePunchHoleReq"
    `spec.offload.FallocatePunchHoleReq`

    The FUSE `fallocate` callback implements these modes in one
    transaction:

    - **default** (no flags): extends `size` to `offset + length` if that
      is larger. pgfs does not reserve space, so later writes may still
      fail with ENOSPC, which the documentation states.
    - **`FALLOC_FL_KEEP_SIZE`**: changes nothing, because pgfs has no
      preallocation.
    - **`FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE`**: removes the map
      rows of blocks inside the range, writing hole rows on a branch
      (`branch.CopiesUpOnWrite`), and rewrites partly covered edge
      blocks with the covered bytes zeroed.
    - **`FALLOC_FL_ZERO_RANGE`**: as punch hole, and extends `size` unless
      `KEEP_SIZE` is set.

    On an inline file (`storage.InlinesSmallFiles`), punching or zeroing
    within `inline_max` zeroes the slice in `inline_data`, and extending
    past `inline_max` promotes `inline_data` to block 0 first. Any mode
    that modifies data or `size` updates `mtime` and `ctime` and records
    a `modify` row in `changes` (`changefeed.RecordsChanges`). Other
    modes reply EOPNOTSUPP (`posix.DeclaresUnsupportedOps`).

    **Postcondition.** Every byte in the range reads as zero. Bytes
    outside it are unchanged. With `KEEP_SIZE` the size is unchanged.
    Space for removed blocks is freed as in
    `space.FreesUnsharedBlobsInline`.

    **Errors.** EOPNOTSUPP for unsupported modes. EINVAL for a negative
    or overflowing range. EROFS on a snapshot.

??? abstract "ContentSearchOffloadReq"
    `spec.offload.ContentSearchOffloadReq`

    Planned after the first release. `pgfs search` and a `pgfs` SQL
    function find files by content through a `pg_trgm` GIN index over
    block content, or over text extracted from it, and return matching
    paths without reading any file through FUSE.

    The design must settle these before it is built:

    - Indexing each block on its own misses a match that crosses a block
      boundary. The index either covers overlapping windows, or the
      documentation states the limit.
    - A change-feed consumer (`changefeed.ResumableCursors`) maintains
      the index, not triggers on the write path, so writes do not pay
      for it. Results lag by the consumer's delay.
    - Queries may run on a standby, keeping the primary free (TiDB r34:
      a learner for analytics).

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
