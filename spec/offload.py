"""
Kernel offloading: operations the kernel hands to the filesystem as
intent rather than bytes, executed inside PostgreSQL.

`copy_file_range` becomes block-map rows that share blobs, and
`fallocate` becomes map-row deletes and hole rows, so no file data
passes through the daemon. Content search through Postgres indexes is
planned after the first release. Reflink copies and subtree clones are
not separate requirements: the first is what `copy_file_range` does,
and the second is a branch rooted at a directory
(`branch.ViewsRootedAtDirectory`).
"""

from .err import PARTIAL, Feat, Req, md
from .changefeed import (
    RecordsChanges,
    ResumableCursors,
)
from .consistency import (
    BoundedTransactions,
)
from .space import (
    CollectsSharedBlobs,
    FreesUnsharedBlobsInline,
)
from .storage import (
    BlockMap,
    ImmutableBlobs,
)


class CopyFileRangeReq(Req):
    """The FUSE `copy_file_range` callback copies a byte range between
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
    `consistency.OneOperationOneTransaction` forbids."""

    deps = [ImmutableBlobs, CollectsSharedBlobs, BoundedTransactions]


class FallocatePunchHoleReq(Req):
    """The FUSE `fallocate` callback implements these modes in one
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
    or overflowing range. EROFS on a snapshot."""

    deps = [BlockMap, FreesUnsharedBlobsInline, RecordsChanges]


class ContentSearchOffloadReq(Req):
    """Planned after the first release. `pgfs search` and a `pgfs` SQL
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
    """

    deps = [ResumableCursors]


# ── Features ──────────────────────────────────────────────────────────

class KernelOffloadingAcceleration(Feat):
    """Copies and hole punching run inside PostgreSQL. `cp` shares blocks
    instead of copying bytes, and `fallocate` punches holes and zeroes
    ranges by changing block-map rows, so no file data passes through
    the daemon. Content search through database indexes is planned."""

    deps = [
        CopyFileRangeReq,
        FallocatePunchHoleReq,
        ContentSearchOffloadReq,
    ]

    def title(self):
        return "Kernel Offloading"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "`copy_file_range` maps whole blocks to the source's blobs "
            "(shared, collected by GC) and copies only partial edge "
            "blocks, in one bounded transaction; `fallocate` extends, "
            "punches holes and zeroes ranges by changing map rows. "
            "Content search is planned."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will copy a large file with `cp` and watch pgfs
            share its blocks instead of copying them, then punch a hole in a file
            and see the space go. We need a mounted filesystem (see
            [Your first pgfs mount](projectlocalenvironment.md)).

            ## Step 1: make a file and copy it

            ```bash
            head -c 2000000 /dev/urandom > testdata/mnt/orig.bin
            cp testdata/mnt/orig.bin testdata/mnt/copy.bin
            cmp testdata/mnt/orig.bin testdata/mnt/copy.bin && echo identical
            ```

            ```text
            identical
            ```

            coreutils 9 `cp` calls `copy_file_range(2)`, which reaches pgfs's
            `copy_file_range` callback. Whole blocks at the same offset become
            references to the source's blobs.

            ## Step 2: see the sharing

            ```bash
            psql -h "$PWD/testdata" -d pgfs -c "SELECT count(*) AS shared_blobs FROM pgfs.blobs WHERE shared"
            ```

            ```text
             shared_blobs
            --------------
                       30
            ```

            Thirty 64 KiB blocks are referenced by both files; only the partly
            covered last block was written again.

            ## Step 3: punch a hole

            ```bash
            fallocate --punch-hole --offset 65536 --length 655360 testdata/mnt/copy.bin
            du -k testdata/mnt/copy.bin
            ```

            The ten blocks in the range read as zero and no longer count in `du`.

            ## What we did

            We saw a copy that moved no data through the daemon, and a hole
            punched by removing map rows. The
            [explanation](../explanation/kerneloffloadingacceleration.md) says why
            these run inside PostgreSQL.
        """)

    def how_to(self):
        return md(r"""
            ## Copy without moving data

            Use any tool that calls `copy_file_range(2)`: `cp` from coreutils 9,
            `sendfile`, or Python's `os.copy_file_range`. Whole blocks are shared
            when the source and destination offsets are equal modulo the block
            size; other bytes are copied by the daemon. A copy larger than 4 096
            blocks replies with the bytes copied so far, and the caller continues.

            ## Punch holes and zero ranges

            ```bash
            fallocate --punch-hole --offset OFFSET --length LENGTH FILE
            fallocate --zero-range --offset OFFSET --length LENGTH FILE
            fallocate --length LENGTH FILE        # extend the size; no preallocation
            ```

            `FALLOC_FL_COLLAPSE_RANGE` and `FALLOC_FL_INSERT_RANGE` reply
            EOPNOTSUPP.

            ## See how much is shared

            ```sql
            SELECT count(*) FILTER (WHERE shared) AS shared, count(*) AS blobs FROM pgfs.blobs;
            SELECT gc_queue FROM pgfs.backlog();
            ```

            Shared blobs are freed by the background collector once no map row
            references them (`space.CollectsSharedBlobs`).
        """)

    def reference(self):
        return md(r"""
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
        """)

    def explanation(self):
        return md(r"""
            ## Why copies and holes run in the database

            A FUSE copy normally reads every byte into the kernel, then into the
            daemon, then into the database, and back out to write it. pgfs stores
            file data as immutable blobs behind a block map, so a copy of whole
            blocks is a set of new map rows pointing at the same blobs: no data
            moves, and the copy costs a few rows per megabyte. This is a reflink
            in all but name. `cp --reflink=always` itself cannot reach pgfs
            because FUSE cannot pass `FICLONE` through, but `copy_file_range`
            gets the same result.

            Sharing has a cost: a blob with two references can no longer be freed
            when one of them goes. Shared blobs are marked once and collected in
            the background after a reference check, so the common path (files
            never copied) still frees space at once.

            A hole is the absence of a map row, so punching one is deleting rows,
            and zeroing a range is the same plus rewriting the partly covered
            edges.
        """)
