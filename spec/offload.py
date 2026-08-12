"""
Database-Native Kernel Offloading & Acceleration specifications for pgfs.

Derived from POSIX kernel acceleration capabilities (copy_file_range, fallocate,
reflink CoW, server-side search, and subtree cloning).

These granular Requirement classes define how pgfs leverages PostgreSQL's relational
engine to intercept standard file operations and execute them as set-based database
mutations, eliminating userspace byte streaming and delivering near-instantaneous
operation throughput.
"""

from .err import Feat, Req


class CopyFileRangeReq(Req):
    """Intercepts the FUSE `copy_file_range` callback triggered by `cp`, `sendfile`,
    or `copy_file_range(2)` syscalls, executing server-side block duplication directly
    inside PostgreSQL.

    Technical Details:
    - Intercepts `copy_file_range(ino_in, off_in, ino_out, off_out, len, flags)`.
    - Executes `INSERT INTO blocks SELECT ...` or CoW block ref-linking inside PostgreSQL
      without streaming payload bytes through FUSE daemon memory buffers.
    - Updates target entry size, `mtime`, and `ctime` metadata atomically in a single transaction."""
    pass


class FallocatePunchHoleReq(Req):
    """Intercepts the FUSE `fallocate` callback for block allocation and sparse hole
    punching (`FALLOC_FL_PUNCH_HOLE`).

    Technical Details:
    - For `FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE`, deletes block records in the target
      byte range `DELETE FROM blocks WHERE ino = $1 AND block_no >= $2 AND block_no <= $3`.
    - For standard pre-allocation (`FALLOC_FL_KEEP_SIZE`), creates zero-filled block placeholders
      or updates entry size metadata without physical byte copying."""
    pass


class ReflinkBlockSharingReq(Req):
    """Implements reference-counted Copy-on-Write (CoW) block storage for instant file
    cloning (`cp --reflink=always`).

    Technical Details:
    - Introduces a `block_id` and `ref_count` column or block reference mapping table.
    - Offloaded file copies simply copy block references and increment `ref_count` in $O(1)$ time.
    - Write operations on shared blocks perform lazy Copy-on-Write allocation (`write_block`),
      allocating a private block copy only when modified."""
    pass


class CloneSubtreeReq(Req):
    """Enables server-side recursive directory tree cloning via custom `ioctl` or extended
    attribute (`xattr`) calls.

    Technical Details:
    - Intercepts subtree clone requests for a target directory inode.
    - Executes recursive set-based `INSERT INTO entries SELECT ...` and `INSERT INTO blocks SELECT ...`
      queries inside a single PostgreSQL transaction.
    - Clones complex multi-gigabyte directory hierarchies in milliseconds."""
    pass


class ContentSearchOffloadReq(Req):
    """Offloads file content searching (`grep`, `find`) to PostgreSQL GIN and `pg_trgm`
    trigram indices.

    Technical Details:
    - Maintains GIN/trigram indices over `blocks.data` or extracted text.
    - Exposes custom search queries (via `xattr` or virtual `/proc`/control paths) returning
      matching inode lists or line offsets directly from database index scans, bypassing
      full file reads over FUSE."""
    pass


# ── Features ──────────────────────────────────────────────────────────

class KernelOffloadingAcceleration(Feat):
    """pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing
    copy_file_range, fallocate, reflink CoW, and server-side search to eliminate
    userspace byte streaming and deliver sub-millisecond execution for large-scale
    file operations."""

    def tutorial(self):
        return (
            "In this tutorial, we will use kernel offloading features in pgfs to clone files and directories instantly.\n"
            "1. Create a large source file: `head -c 100M /dev/urandom > ./mnt/large.bin`.\n"
            "2. Perform an instant copy using `cp --reflink=always ./mnt/large.bin ./mnt/clone.bin`.\n"
            "3. Notice that the copy completes instantaneously (<5ms) because PostgreSQL copied block reference IDs.\n"
            "4. Punch a sparse hole using `fallocate -p -o 1M -l 10M ./mnt/clone.bin`.\n"
            "5. Verify hole punching freed space in PostgreSQL without re-writing the rest of the file."
        )

    def how_to(self):
        return (
            "To leverage database-native kernel offloading:\n"
            "- Use standard `copy_file_range(2)` or `cp --reflink=always` for zero-copy file duplication.\n"
            "- Use `fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, offset, len)` to deallocate block ranges.\n"
            "- Execute content searches via GIN/trigram index queries rather than scanning entire files across FUSE."
        )

    def reference(self):
        return (
            "KernelOffloadingAcceleration Callbacks & Requirements:\n"
            "- `CopyFileRangeReq`: Executes `INSERT INTO blocks SELECT ...` inside PostgreSQL.\n"
            "- `FallocatePunchHoleReq`: Deletes block rows `DELETE FROM blocks WHERE ino = $1 AND block_no BETWEEN $2 AND $3`.\n"
            "- `ReflinkBlockSharingReq`: Reference-counted Copy-on-Write (`ref_count` increment on reflink, lazy copy on write).\n"
            "- `CloneSubtreeReq`: Recursive single-transaction set-based subtree copy (`INSERT INTO entries SELECT ...`).\n"
            "- `ContentSearchOffloadReq`: PostgreSQL GIN & `pg_trgm` index search pushdown."
        )

    def explanation(self):
        return (
            "KernelOffloadingAcceleration moves file manipulation operations into PostgreSQL's query engine.\n"
            "Instead of reading bytes from disk into FUSE daemon memory and writing them back over socket buffers,\n"
            "pgfs intercepts Linux VFS acceleration callbacks (`copy_file_range`, `fallocate`) and translates them into\n"
            "set-based database transactions, executing multi-gigabyte operations in sub-millisecond time."
        )
