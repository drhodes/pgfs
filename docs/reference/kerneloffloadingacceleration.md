# Reference: Kernel Offloading

!!! warning "Status: Partially implemented"
    Server-side `copy_file_range` and `fallocate` are implemented in the storage backend and unit-tested, but the FUSE layer does not call them yet. On a mount, copies stream through the daemon and `fallocate` fails with `EOPNOTSUPP`. Reflink, subtree clone and content search are planned.

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing copy_file_range, fallocate, reflink CoW, and server-side search to eliminate userspace byte streaming and deliver sub-millisecond execution for large-scale file operations.

[Tutorial](../tutorials/kerneloffloadingacceleration.md) · [How-to](../how-to/kerneloffloadingacceleration.md) · **Reference** · [Explanation](../explanation/kerneloffloadingacceleration.md)

---

## Requirement status

| Requirement | Status | Behaviour on a mount today |
|---|---|---|
| `CopyFileRangeReq` | backend only (`Db` as `PgfsBackend`) | `copy_file_range` is not a FUSE callback in pgfs; the kernel falls back to read/write |
| `FallocatePunchHoleReq` | backend only | `fallocate` replies `EOPNOTSUPP` |
| `ReflinkBlockSharingReq` | planned | `cp --reflink=always` fails with `EOPNOTSUPP` |
| `CloneSubtreeReq` | planned | not available |
| `ContentSearchOffloadReq` | planned | not available |

## Backend `copy_file_range`

`copy_file_range(ino_in, off_in, ino_out, off_out, len) -> u64`

| Case | Behaviour |
|---|---|
| `len == 0` | returns 0 |
| `off_in`, `off_out` and `len` all multiples of 131 072 | one `INSERT INTO blocks … SELECT … FROM blocks WHERE ino = ino_in` with `ON CONFLICT DO UPDATE`, then `UPDATE entries SET size = GREATEST(size, off_out + len), mtime = now()`; returns `len` |
| otherwise | block-by-block `read_block` / `write_block`; returns bytes copied; source blocks with no row are skipped |

The two statements of the aligned path are not wrapped in one
transaction.

## Backend `fallocate`

`fallocate(ino, offset, length, mode)`

| `mode` bits | Behaviour |
|---|---|
| `FALLOC_FL_PUNCH_HOLE` (0x02) | `DELETE FROM blocks WHERE ino = $1 AND block_no BETWEEN first AND last`, covering every 128 KiB block the range touches, including partially covered ones |
| without `FALLOC_FL_KEEP_SIZE` (0x01) | `size = GREATEST(size, offset + length)`; no blocks are written |
| `FALLOC_FL_KEEP_SIZE` only | no change |

## Block granularity

The backend implementation uses 131 072-byte blocks; the
running FUSE path uses 65 536-byte blocks (see
[Dual-Interface Architecture](dualinterfacearchitecture.md)).


## Specification requirements

`KernelOffloadingAcceleration` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "CopyFileRangeReq"
    `spec.offload.CopyFileRangeReq`

    Intercepts the FUSE `copy_file_range` callback triggered by `cp`, `sendfile`,
    or `copy_file_range(2)` syscalls, executing server-side block duplication directly
    inside PostgreSQL.

    Technical Details:

    - Intercepts `copy_file_range(ino_in, off_in, ino_out, off_out, len, flags)`.
    - Executes `INSERT INTO blocks SELECT ...` or CoW block ref-linking inside PostgreSQL
      without streaming payload bytes through FUSE daemon memory buffers.
    - Updates target entry size, `mtime`, and `ctime` metadata atomically in a single transaction.

??? abstract "FallocatePunchHoleReq"
    `spec.offload.FallocatePunchHoleReq`

    Intercepts the FUSE `fallocate` callback for block allocation and sparse hole
    punching (`FALLOC_FL_PUNCH_HOLE`).

    Technical Details:

    - For `FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE`, deletes block records in the target
      byte range `DELETE FROM blocks WHERE ino = $1 AND block_no >= $2 AND block_no <= $3`.
    - For standard pre-allocation (`FALLOC_FL_KEEP_SIZE`), creates zero-filled block placeholders
      or updates entry size metadata without physical byte copying.

??? abstract "ReflinkBlockSharingReq"
    `spec.offload.ReflinkBlockSharingReq`

    Implements reference-counted Copy-on-Write (CoW) block storage for instant file
    cloning (`cp --reflink=always`).

    Technical Details:

    - Introduces a `block_id` and `ref_count` column or block reference mapping table.
    - Offloaded file copies simply copy block references and increment `ref_count` in $O(1)$ time.
    - Write operations on shared blocks perform lazy Copy-on-Write allocation (`write_block`),
      allocating a private block copy only when modified.

??? abstract "CloneSubtreeReq"
    `spec.offload.CloneSubtreeReq`

    Enables server-side recursive directory tree cloning via custom `ioctl` or extended
    attribute (`xattr`) calls.

    Technical Details:

    - Intercepts subtree clone requests for a target directory inode.
    - Executes recursive set-based `INSERT INTO entries SELECT ...` and `INSERT INTO blocks SELECT ...`
      queries inside a single PostgreSQL transaction.
    - Clones complex multi-gigabyte directory hierarchies in milliseconds.

??? abstract "ContentSearchOffloadReq"
    `spec.offload.ContentSearchOffloadReq`

    Offloads file content searching (`grep`, `find`) to PostgreSQL GIN and `pg_trgm`
    trigram indices.

    Technical Details:

    - Maintains GIN/trigram indices over `blocks.data` or extracted text.
    - Exposes custom search queries (via `xattr` or virtual `/proc`/control paths) returning
      matching inode lists or line offsets directly from database index scans, bypassing
      full file reads over FUSE.

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
