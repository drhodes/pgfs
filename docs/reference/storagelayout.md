# Reference: Storage Layout

!!! success "Status: Stable"
    `pgfs mkfs` creates the versioned `pgfs` schema: inode and dentry rows keyed by persistent inode numbers with random generations, byte-exact names, inline small files, fixed-size immutable checksummed blobs behind a block map, epoch-versioned xattrs, every operation as a SQL function, and the read-only views.

> Every byte of a pgfs filesystem is a row in PostgreSQL: inodes, names, attributes and data blocks. Inode numbers are permanent, names are exact bytes, small files take one row, and file data is immutable, checksummed and shareable between files, snapshots and branches.

[Tutorial](../tutorials/storagelayout.md) · [How-to](../how-to/storagelayout.md) · **Reference** · [Explanation](../explanation/storagelayout.md)

---

## Tables

| Table | Key | Holds |
|---|---|---|
| `meta` | `key` | schema version and filesystem identity |
| `layers` | `layer` | one row per layer: base, depth, current epoch |
| `trees` | `name` | named trees and the layer each points to |
| `snapshots` | `(tree, name)` | named snapshots: layer, epoch, xid and optional sealed digest |
| `inodes` | `(layer, ino, born)` | `gen`, attributes, inline data, symlink targets, `shadows_base` (`nlink = 0` is a whiteout when not in `orphans`) |
| `dentries` | `(layer, parent_ino, name, born)` | names and `shadows_base`; a null `ino` is a whiteout |
| `file_blocks` | `(layer, ino, block_no, born)` | block map; a null `blob_id` hides an inherited block |
| `blobs` | `blob_id` | immutable block content with CRC32C |
| `xattrs` | `(layer, ino, name, born)` | extended attributes and `shadows_base`; a null `value` is a whiteout |
| `mounts` | `mount_id` | active mount sessions (read-write, read-only live, snapshot), leases and fencing epochs |
| `control_requests` | `request_id` | snapshot and branch requests for an owning daemon (`pending`, `done`, `cancelled`) |
| `orphans` | `(layer, ino)` | unlinked files that are still open |
| `op_log` | `(mount_id, worker_id)` | latest idempotency record (`op_seq, result`) per worker |
| `changes` | `seq` | the change feed |
| `change_consumers` | `name` | registered feed consumers, their target `tree`, and their watermark tokens |
| `gc_queue` | `blob_id` | shared blobs waiting for a reference check |
| `usage_deltas` | `seq` | space accounting deltas |
| `usage_totals` | `layer` | folded logical totals per layer; the filesystem-wide physical total on layer 0 |
| `epoch_holds` | `(layer, epoch)` | epochs of deleted snapshots, retained until every reader has failed closed |
| `change_prune` | `layer` | the transaction id below which changes were pruned, for expired cursors |
| `scrub_state` | `id` | where `pgfs scrub` resumes |

`died = 9223372036854775807` marks a live row.

## Fixed at mkfs

| Setting | Default | Range |
|---|---|---|
| block size | 65 536 | powers of two, 4 096 – 1 048 576 |
| inline limit | 2 048 | 0 – 2 048 |

## Limits

| Limit | Value |
|---|---|
| name length | 1 – 255 bytes; no `/`, no NUL |
| file size | 2^63 − 1 bytes |
| xattr name / value | 255 bytes / 65 536 bytes |
| timestamp resolution | 1 ns |


## Specification requirements

`StorageLayout` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "SchemaVersion"
    `spec.storage.SchemaVersion`

    The table `meta(key text primary key, value text not null)` holds
    a `schema_version` row. Every daemon and CLI command reads it before
    doing anything else and refuses to run against a version it does not
    know, naming both versions in the error. The schema changes only
    through `ops.MigratesSchema`; no command adds a column or index on
    the fly.

    **Invariant.** Every table, index and function in the `pgfs` schema
    is exactly the set that `schema_version` names.

    **Errors.** A missing `meta` table means the database is unformatted
    (`ops.RefusesUnformattedDatabase`). An unknown version exits with
    status 2 and an error naming the version found and the versions this
    binary supports.

    **Sources.** Chubby r04 (explicit versioned state), Raft r25.

??? abstract "FilesystemIdentity"
    `spec.storage.FilesystemIdentity`

    `pgfs mkfs` writes these `meta` rows once, and nothing changes
    them afterwards:

    | key | meaning |
    |---|---|
    | `fs_uuid` | random UUID naming this filesystem; changeable only with `pgfs tune --new-uuid` on an unmounted filesystem, for restored copies |
    | `block_size` | bytes per data block (`FixedBlockSize`) |
    | `inline_max` | largest file stored inline (`InlinesSmallFiles`) |
    | `capacity_bytes` | reported capacity (`space.ReportsStatfs`), changeable only with `pgfs tune` |
    | `scratch` | `'true'` when formatted with `mkfs --scratch`, `'false'` otherwise (`durability.ScratchMode`) |
    | `created_at` | creation time, RFC 3339 |
    | `root_uid`, `root_gid`, `root_mode` | owner and mode of a tree's root directory, used again by `pgfs.scratch_reset()` |

    The daemon names its mount after `fs_uuid` (`app.MountsCleanly`) and
    checks on every reconnect that it has not changed
    (`failure.RevalidatesAfterReconnect`).

    **Invariant.** `block_size`, `inline_max` and `scratch` never change
    after mkfs. `fs_uuid` never changes while any daemon has the
    filesystem mounted (`ops.TunesFilesystem`).

??? abstract "InodeTable"
    `spec.storage.InodeTable`

    `inodes` holds one row per inode version:

    | column | type | meaning |
    |---|---|---|
    | `layer` | bigint | owning layer (`branch.LayersTable`) |
    | `ino` | bigint | inode number (`PersistentInodes`) |
    | `gen` | bigint | random 63-bit generation number (`PersistentInodes`) |
    | `born`, `died` | bigint | epoch range (`branch.EpochVersionedRows`) |
    | `kind` | "char" | `f` file, `d` directory, `l` symlink, `p` FIFO, `s` socket, `c` char device, `b` block device |
    | `mode` | integer | permission and set-id bits, without the type bits |
    | `uid`, `gid` | bigint | owner |
    | `nlink` | integer | link count (`posix.LinkCounts`) |
    | `size` | bigint | file length in bytes |
    | `atime_ns`, `mtime_ns`, `ctime_ns`, `btime_ns` | bigint | nanoseconds since the Unix epoch |
    | `rdev` | bigint | device number for `c` and `b` |
    | `inline_data` | bytea | content of small files (`InlinesSmallFiles`) |
    | `target` | bytea | symlink target |
    | `inherit_limit` | bigint | first block number not inherited from the base (`branch.CopiesUpOnWrite`) |
    | `shadows_base` | boolean | whether the layer's base view can see this `ino`, i.e. the row was copied up (`branch.EpochVersionedRows`) |
    | `version` | bigint | incremented by every change to content or attributes |

    Timestamps are integers because `timestamptz` keeps only
    microseconds. The table is created with `fillfactor = 80` so that
    attribute updates stay heap-only tuple (HOT) updates
    (`space.TableTuning`).

    **Invariant.** Within a layer, at most one row per `ino` is live
    (`died = ∞`), enforced by a partial unique index on `(layer, ino)`.

??? abstract "DentryTable"
    `spec.storage.DentryTable`

    `dentries(layer, parent_ino, name bytea, born, died, ino, kind,
    shadows_base boolean)` maps a name in a directory to an inode. `ino`
    is null for a whiteout (`branch.LayeredLookup`). `kind` repeats the
    child's kind so that readdir needs no join. `shadows_base` is true
    when the layer's base view can see `(parent_ino, name)`
    (`branch.EpochVersionedRows`).

    **Invariant.** Within a layer, at most one live row per
    `(parent_ino, name)`, enforced by a partial unique index. Every live
    dentry with a non-null `ino` resolves, through the layer chain, to a
    live inode, and `parent_ino` of every live dentry (including
    whiteouts) resolves to a live directory. No directory has more than
    one live dentry pointing to it. The directory graph is a tree rooted
    at ino 1 (or `trees.root_ino` for a subtree branch,
    `branch.ViewsRootedAtDirectory`).

    **Concurrency.** Uniqueness is decided by the index inside the
    inserting statement, never by an earlier existence read (Lockify
    r41), so two creates of one name cannot both succeed.

    **Sources.** TableFS r07, IndexFS r09, HopsFS r11, pFSCK r54.

??? abstract "PersistentInodes"
    `spec.storage.PersistentInodes`

    The inode number the kernel sees is the database `ino`, paired
    with a random 63-bit generation number `inodes.gen` returned as the
    FUSE entry generation. New inos come from one sequence; `gen` is set
    at creation from a cryptographic or OS random source and never from a
    sequence, because sequences roll back with lost WAL after an
    asynchronous-commit crash (`durability.AsyncCommitWindow`). The root
    directory of every full tree is ino 1 (or `trees.root_ino` for a
    subtree branch, `branch.ViewsRootedAtDirectory`). Inherited files in
    a branch and copied-up rows keep both `ino` and `gen`; files created
    in the branch get new ones.

    The daemon remembers the `gen` it returned for every nodeid the
    kernel holds (`ForgetsInodes`) and stores `(ino, gen)` in every file
    handle. Every operation by nodeid or handle checks `gen` in its query
    (for example `WHERE ino = $1 AND gen = $2` in mutating functions). On
    a mismatch it replies ESTALE and invalidates the kernel's inode cache
    entry, turning any post-rollback `ino` reuse into a clean error
    rather than silent cross-file reads or writes.

    **Invariant.** No two live inodes in one view share an `ino`.
    `(ino, gen)`, once allocated, never names a different file. An `ino`
    alone may be reused only after a database crash, restore or failover
    that loses acknowledged asynchronous commits
    (`failure.RevalidatesAfterReconnect`).

    **Errors.** ESTALE when a nodeid or file handle's `gen` does not
    match the stored inode row.

    **Bounds.** The daemon holds no ino-to-path map. It caches inode
    rows and evicts them on `forget` (`ForgetsInodes`).

    **Sources.** Inversion r00, HopsFS r11, AFS 05 (fids are never
    reused), XFUSE r35 and Ananke r46 (restart needs stable numbers).

??? abstract "ForgetsInodes"
    `spec.storage.ForgetsInodes`

    The daemon implements `forget` and `batch_forget`. Each inode in
    the daemon's cache carries the kernel's lookup count; when the count
    reaches zero the entry is evicted, together with any per-inode state
    that has no open handle. A forgotten inode that is an orphan
    (`posix.OpenAfterUnlink`) and has no open handle is reclaimed.

    **Bounds.** Daemon memory for inode state is proportional to the
    number of inodes the kernel holds, not to the number ever looked up.

    **Sources.** ScaleCheck r57.

??? abstract "ByteExactNames"
    `spec.storage.ByteExactNames`

    A name is a `bytea` of 1 to 255 bytes that contains neither `/`
    nor NUL and is not `.` or `..`. Names are compared byte for byte; no
    collation, case folding or Unicode normalisation is applied, and any
    byte sequence that is a legal Unix name works, UTF-8 or not.

    **Errors.** A name longer than 255 bytes replies ENAMETOOLONG. An
    empty name, or one containing `/` or NUL, replies EINVAL. The check
    runs before any query.

    **Sources.** Unsafe at Any Copy r52 (copy tools fail when name
    semantics differ).

??? abstract "InlinesSmallFiles"
    `spec.storage.InlinesSmallFiles`

    A regular file whose size is at most `inline_max` keeps its
    content in `inodes.inline_data` and has no block-map rows. mkfs sets
    `inline_max` so that an inode row stays below Postgres's TOAST
    threshold; the default is 2048 bytes. A write, truncate, `fallocate`
    or `copy_file_range` that takes the file past `inline_max` (or
    shares blobs) moves the content into `file_blocks` and sets
    `inline_data` to null in the same transaction. A truncate below
    `inline_max` does not move it back; `inline_data` and `file_blocks`
    rows are mutually exclusive (`integrity.PgfsCheck`).

    **Postcondition.** Inline and block storage are indistinguishable
    through the filesystem: same bytes, same size, same holes (read as
    zero).

    **Bounds.** Creating, writing and closing a file of up to
    `inline_max` bytes touches one inode row and one dentry row.

    **Sources.** TableFS r07, small files in HopsFS r38.

??? abstract "FixedBlockSize"
    `spec.storage.FixedBlockSize`

    File data is divided into blocks of `meta.block_size` bytes,
    fixed at mkfs. The default is 65 536; mkfs accepts any power of two
    from 4 096 to 1 048 576. Block `n` covers bytes
    `[n × block_size, (n + 1) × block_size)`.

    **Invariant.** Every stored blob is at most `block_size` bytes long.
    The last block of a file may be shorter; bytes between a blob's
    end and the block's end read as zero.

??? abstract "ImmutableBlobs"
    `spec.storage.ImmutableBlobs`

    Block content is stored in
    `blobs(blob_id bigint, len integer, crc32c integer, shared boolean,
    data bytea)`. A blob is written once and never updated; changing
    any byte of a block inserts a new blob and points the block map at
    it. `data` uses `COMPRESSION lz4`.

    **Invariant.** A blob's `data`, `len` and `crc32c` never change
    after insert. `crc32c` is the CRC32C of `data`
    (`integrity.BlobChecksums`).

    **Concurrency.** Readers never see a partly written block, because
    a block is replaced by moving one map row.

    **Sources.** CalvinFS r10 (immutable blocks plus a range map),
    BlueStore r33 (shared blob records separate from extents), Petal
    r37.

??? abstract "BlockMap"
    `spec.storage.BlockMap`

    `file_blocks(layer, ino, block_no, born, died, blob_id)` maps a
    file block to a blob. A block with no row is a hole and reads as
    zero. A row whose `blob_id` is null is a hole that hides an
    inherited block (`branch.CopiesUpOnWrite`).

    **Invariant.** Every non-null `blob_id` references an existing blob
    (foreign key). No live row has `block_no × block_size ≥ size` for
    its inode once the transaction that changed `size` commits.

    **Bounds.** Reading or writing a byte range touches only the map
    rows for the blocks in that range.

??? abstract "DataBeforeMetadata"
    `spec.storage.DataBeforeMetadata`

    Within each transaction, a blob is inserted before the map row
    that references it, and a map row before the inode `size` that
    exposes it. In the single-machine filesystem all three are in one
    transaction, so the order matters only to code structure. In the
    multi-machine filesystem (v0.2), where blobs live in a data set and
    maps in the metadata database, the same order makes a blob
    with no reference the only possible debris of a crash, and such a
    blob is harmless.

    **Crash.** A crash never leaves a map row pointing at a missing
    blob, or a size covering a block whose map row is missing.

    **Sources.** SwitchΔ r133, Tectonic r14, PoWER r60.

??? abstract "ExtendedAttributeTable"
    `spec.storage.ExtendedAttributeTable`

    `xattrs(layer, ino, name bytea, value bytea, born, died,
    shadows_base boolean)` stores extended attributes
    (`posix.ExtendedAttributes`), versioned like every other row, so
    snapshots and branches include them. A null `value` is a whiteout
    hiding an inherited attribute (`branch.LayeredLookup`), distinct from
    an empty value (`''::bytea`). `shadows_base` is true when the layer's
    base view can see `(ino, name)` (`branch.EpochVersionedRows`).

    **Bounds.** A name is at most 255 bytes and a value at most 65 536
    bytes, the Linux limits. Larger values reply E2BIG.

??? abstract "OperationsAsSqlFunctions"
    `spec.storage.OperationsAsSqlFunctions`

    Every mutating filesystem operation, and every operation that
    needs more than one statement, is a function in the `pgfs` schema:
    for example `pgfs.create`, `pgfs.rename`, `pgfs.write_blocks`,
    `pgfs.snapshot`, `pgfs.branch` and `pgfs.promote`. The daemon, the
    CLI and any future daemon call the same functions. Single-statement
    reads may be prepared statements instead. The functions belong to
    the schema version (`SchemaVersion`).

    **Postcondition.** A function either applies the whole operation or
    raises, leaving nothing changed.

    **Bounds.** Each filesystem operation is one round trip
    (`consistency.OneRoundTripPerOperation`).

    **Sources.** DynamoDB transactions r82, Oracle FSS r91.

??? abstract "SqlReadViews"
    `spec.storage.SqlReadViews`

    Read-only views give SQL users a stable interface that does not
    change when the tables do:

    - `pgfs.files(tree, path, ino, kind, size, mode, uid, gid, mtime)`,
      with `path` decoded as text and rendered with `\x` escapes for
      bytes that are not UTF-8;
    - `pgfs.tree_usage(tree, files, bytes)`;
    - `pgfs.snapshot_list(tree, name, created_at, bytes_unique)`.

    The views resolve layers and epochs, so they show exactly what a
    mount of the tree shows. Writes through SQL are not supported
    (`consistency.ExternalSqlWritesUnsupported`).

---

<small>Generated from `spec.storage.StorageLayout` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
