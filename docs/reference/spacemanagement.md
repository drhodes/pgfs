# Reference: Space Management

!!! success "Status: Stable"
    statfs reports capacity and usage from delta-accounted totals, writes past the capacity reply ENOSPC, unshared blobs are freed inline, shared blobs are collected, retired rows and deleted layers are reclaimed after a grace period, orphans are reclaimed at lease acquisition, and space, backlog and amplification are reported. Without `--capacity` the free figure is a nominal 1 TiB. Per-tree quotas remain planned.

> pgfs reports real capacity and usage, refuses writes when full, and reclaims space from deleted files, snapshots and trees in the background, with the backlog and the write amplification always visible.

[Tutorial](../tutorials/spacemanagement.md) · [How-to](../how-to/spacemanagement.md) · **Reference** · [Explanation](../explanation/spacemanagement.md)

---

## statfs fields

| Field | Value |
|---|---|
| `f_bsize`, `f_frsize` | block size |
| `f_blocks` | capacity / block size |
| `f_bfree`, `f_bavail` | (capacity − used) / block size |
| `f_files`, `f_ffree` | 2^48, 2^48 − inodes used |
| `f_namemax` | 255 |

## Background work

| Task | Batch | Trigger |
|---|---|---|
| retired-row reclaim | 10 000 rows | continuous, rate-limited |
| shared-blob collection | 1 000 blobs | `gc_queue` not empty |
| usage folding | all deltas | every 10 s |
| orphan reclaim | one orphan | lease acquired |


## Specification requirements

`SpaceManagement` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "ReportsStatfs"
    `spec.space.ReportsStatfs`

    `statfs` reports:

    - `f_bsize` and `f_frsize` as the block size;
    - `f_blocks` as `meta.capacity_bytes` divided by the block size;
    - `f_bfree` and `f_bavail` as capacity minus used;
    - `f_files` as 2^48;
    - `f_ffree` as `f_files` minus the inodes in use;
    - `f_namemax` as 255.

    Used space is the sum of blob lengths, inline data and a fixed
    per-inode overhead, taken from the usage counters
    (`TracksUsage`). With no capacity set, capacity is reported as used
    plus the database's free disk space, when the daemon can read it
    (`pg_tablespace_size` and a `statvfs` of a local data directory).
    Otherwise it is used plus 1 TiB, and the mount logs that the figure
    is nominal.

    **Bounds.** One cached read, refreshed at most every 5 s.

    **Sources.** LFS r22, DisCoGC r44.

??? abstract "TracksUsage"
    `spec.space.TracksUsage`

    Every mutating function and reclamation batch appends its change in
    bytes and inodes to `usage_deltas(seq, layer, logical_bytes,
    physical_bytes, inodes)`, keyed by a sequence `seq`, rather than
    updating a shared counter row:

    - `logical_bytes` and `inodes` record changes to the live space and
      live inode count of `layer` (scoped to `trees.root_ino` for a
      subtree branch, `branch.ViewsRootedAtDirectory`), used by
      `pgfs.tree_usage` and `TreeQuotas`;
    - `physical_bytes` records stored bytes added or freed across the
      filesystem when blobs, inline data or inode rows are inserted or
      deleted (`FreesUnsharedBlobsInline`, `CollectsSharedBlobs`,
      `ReclaimsRetiredRows`), used by `ReportsStatfs` and
      `CapacityAndEnospc`.

    Housekeeping folds the deltas every 10 s into per-layer logical
    totals and the filesystem-wide physical total. Readers add the
    folded totals and the deltas not yet folded.

    **Invariant.** For each layer, the folded logical total plus
    unfolded deltas equals the space and inodes the layer's live rows
    reference; across the filesystem, the folded physical total plus
    unfolded deltas equals the stored blob lengths, inline data and
    inode overhead. `pgfs check` verifies both.

    **Concurrency.** No two workers or mounts update the same row to
    account for space.

    **Sources.** Mantle r127 (delta records), SingularFS r36.

??? abstract "CapacityAndEnospc"
    `spec.space.CapacityAndEnospc`

    When `meta.capacity_bytes` is set, a write or create that would
    take used space past it replies ENOSPC. The check reads the cached
    usage, so concurrent writers can overshoot by at most
    `workers × buffer-per-file`, and that bound is stated in the docs.
    Postgres's own `53100 disk_full` also replies ENOSPC
    (`failure.MapsDatabaseErrors`). Deleting files, snapshots or trees
    always works when the filesystem is full.

??? abstract "FreesUnsharedBlobsInline"
    `spec.space.FreesUnsharedBlobsInline`

    When a transaction removes or replaces the last live map row of a
    blob whose `shared` flag is false, and that row was born in the
    current epoch, the same transaction deletes the blob. A row from an
    earlier epoch is retired instead, and its blob is freed when the
    reclaimer removes the row (`ReclaimsRetiredRows`).

    **Invariant.** An unshared blob is referenced by exactly one map row
    version in its lifetime, so deleting it with that row is safe.

    **Bounds.** No GC backlog on the common path: overwrites and deletes
    of files that were never copied or flattened.

??? abstract "CollectsSharedBlobs"
    `spec.space.CollectsSharedBlobs`

    `copy_file_range` (`offload.CopyFileRangeReq`) and branch
    flattening (`branch.FlattensBranch`) set `shared` on every blob they
    make a second reference to. The flag never clears. When a map row
    referencing a shared blob is deleted, the blob id goes into
    `gc_queue`. A background worker takes queued ids in batches and
    deletes each blob that no `file_blocks` row references. The check
    uses an index on `file_blocks(blob_id)`.

    **Invariant.** No blob that any map row references is ever deleted.
    `verify.CrashConformance` covers crashes during collection.

    **Bounds.** Batches of at most 1 000 blobs, rate-limited.

    **Sources.** BlueStore r33 (shared blob records), f4 r31.

??? abstract "ReclaimsRetiredRows"
    `spec.space.ReclaimsRetiredRows`

    A background reclaimer deletes retired rows (`died ≠ ∞`) that no
    retained epoch can see (`branch.RetainedEpochs`), and the blobs that
    only those rows referenced. The scan uses an index on
    `(layer, died)`. Deleting a snapshot or tree is constant-time; the
    reclaimer recovers its space afterwards. Rows released by deleting a
    snapshot or tree (or a layer in state `deleting`) are reclaimed only
    after no session in `mounts` references that snapshot or tree and
    the longest session expiry plus a grace period has elapsed, so any
    reader whose session renewal failed has already failed closed with
    EIO (`consistency.FencesStaleWriters`) before its rows can disappear.
    A layer in state `deleting` is reclaimed whole and then removed from
    `layers`.

    **Crash.** Each batch is one transaction, so a crash leaves a
    smaller backlog, never a dangling reference.

    **Bounds.** Batches of at most 10 000 rows, rate-limited, yielding
    to foreground work.

    **Sources.** Oracle FSS r91 (housekeeping as separate
    transactions), Dayu r95.

??? abstract "ReclaimsOrphans"
    `spec.space.ReclaimsOrphans`

    When a daemon acquires a tree's lease, it reclaims every orphan of
    that tree (`posix.OpenAfterUnlink`). The previous holder is gone, so
    no handle can still be open on one. Each orphan is reclaimed in its
    own transaction, and large ones go through the reclaimer. When the
    orphan's `inodes` row has `shadows_base = true`, reclamation removes
    its `orphans`, `file_blocks` and `xattrs` rows in the layer and
    turns the `inodes` row into an inode whiteout (`nlink = 0, size = 0,
    inherit_limit = 0, inline_data = NULL, shadows_base = true`,
    `branch.EpochVersionedRows`) rather than deleting it.

??? abstract "ReportsSpace"
    `spec.space.ReportsSpace`

    `pgfs stats TREE` and the periodic metrics report:

    - the bytes used by live data, by snapshots only, and by inline
      data;
    - the reclaim backlog in rows and bytes, and the GC queue length;
    - the dead tuples and bloat estimate of `blobs`, `file_blocks` and
      `inodes`;
    - write amplification: WAL bytes (`pg_stat_wal`) per byte of user
      data written, over the last minute and since mount.

    **Sources.** BlueStore r33, Pangu 2.0 r51 (amplification as a
    first-class metric), LFS r22.

??? abstract "TableTuning"
    `spec.space.TableTuning`

    mkfs creates tables with settings chosen for their access
    pattern, and `pgfs check` reports any that drifted:

    | Table | Setting | Why |
    |---|---|---|
    | `inodes`, `op_log` | `fillfactor = 80` | in-place attribute and per-worker slot updates stay heap-only tuple (HOT) updates |
    | `blobs` | `data` `COMPRESSION lz4`, `toast_tuple_target = 8160` | cheap compression; whole blocks in TOAST |
    | `blobs`, `file_blocks` | `autovacuum_vacuum_scale_factor = 0.02` | vacuum before bloat grows |
    | `changes`, `usage_deltas` | `autovacuum_vacuum_scale_factor = 0.01`; pruned in bulk | insert-delete queues |

    **Sources.** BlueStore r33.

??? abstract "TreeQuotas"
    `spec.space.TreeQuotas`

    Planned: a per-tree byte and inode limit, checked like the
    capacity (`CapacityAndEnospc`) and replying EDQUOT. Not part of the
    first release.

---

<small>Generated from `spec.space.SpaceManagement` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
