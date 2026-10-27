"""
Database-Native Kernel Offloading & Acceleration specifications for pgfs.

Derived from POSIX kernel acceleration capabilities (copy_file_range, fallocate,
reflink CoW, server-side search, and subtree cloning).

These granular Requirement classes define how pgfs leverages PostgreSQL's relational
engine to intercept standard file operations and execute them as set-based database
mutations, eliminating userspace byte streaming and delivering near-instantaneous
operation throughput.
"""

from .err import PARTIAL, Feat, Req, md


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

    deps = [
        CopyFileRangeReq,
        FallocatePunchHoleReq,
        ReflinkBlockSharingReq,
        CloneSubtreeReq,
        ContentSearchOffloadReq,
    ]

    def title(self):
        return "Kernel Offloading"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "Server-side `copy_file_range` and `fallocate` are implemented in "
            "the storage backend and unit-tested, but the FUSE layer does not "
            "call them yet. On a mount, copies stream through the daemon and "
            "`fallocate` fails with `EOPNOTSUPP`. Reflink, subtree clone and "
            "content search are planned."
        )

    def tutorial(self):
        return md(r"""
            The offloading callbacks are not connected to the mount yet,
            but the idea behind them can be tried today with plain SQL. In
            this tutorial we will copy a file without its bytes ever
            leaving PostgreSQL, then run the unit test that exercises the
            real implementation. We need a running mount (see [Your first
            pgfs mount](projectlocalenvironment.md)).

            ## Step 1: create a file through the mount

            ```bash
            mkdir testdata/mnt/offload
            head -c 300000 /dev/urandom > testdata/mnt/offload/big.bin
            ```

            ## Step 2: copy it inside the database

            One transaction copies the file's metadata row and its five
            block rows. No data passes through pgfs or through `psql`:

            ```bash
            psql -h "$PWD/testdata" -d pgfs -v ON_ERROR_STOP=1 <<'SQL'
            BEGIN;
            INSERT INTO entries (parent, name, kind, size)
              SELECT parent, 'clone.bin', kind, size FROM entries
              WHERE parent = 'offload' AND name = 'big.bin';
            INSERT INTO blocks (parent, name, block_no, data)
              SELECT parent, 'clone.bin', block_no, data FROM blocks
              WHERE parent = 'offload' AND name = 'big.bin';
            COMMIT;
            SQL
            ```

            ## Step 3: see the copy appear on the mount

            Wait a second for the kernel's attribute cache to expire, then:

            ```bash
            ls -l testdata/mnt/offload
            cmp testdata/mnt/offload/big.bin testdata/mnt/offload/clone.bin && echo identical
            ```

            ```text
            -rw-r--r-- 1 alice alice 300000 Oct 26 17:13 big.bin
            -rw-r--r-- 1 alice alice 300000 Oct 26 17:13 clone.bin
            identical
            ```

            This is what `copy_file_range` offloading does on the
            inode-keyed schema: one set-based `INSERT … SELECT` instead of
            reading and rewriting every byte.

            ## Step 4: run the real implementation's test

            ```bash
            cargo test tests_offload
            ```

            ```text
            test db::tests_offload::test_backend_offloaded_copy_file_range_and_fallocate ... ok
            ```

            The test copies 128 KiB between two files with
            `copy_file_range`, then punches it back out with `fallocate`.

            ## Step 5: clean up

            ```bash
            rm -r testdata/mnt/offload
            ```

            ## What we did

            We copied a file entirely inside PostgreSQL and saw it through
            the mount, then ran the backend code that will do the same for
            `cp` once it is wired to FUSE. The
            [explanation](../explanation/kerneloffloadingacceleration.md)
            covers the rest of the plan.
        """)

    def how_to(self):
        return md(r"""
            ## Copy a file

            `cp` works and the copy is exact, but today every byte passes
            through the daemon. `cp` calls `copy_file_range(2)`; pgfs does
            not implement the FUSE callback, so the kernel falls back to
            reading and writing.

            ## Copy a large file or tree faster today

            Copy inside the database, as in the tutorial, while nothing is
            writing to the source:

            ```sql
            BEGIN;
            INSERT INTO entries (parent, name, kind, size)
              SELECT parent, 'copy.bin', kind, size FROM entries WHERE parent = 'dir' AND name = 'orig.bin';
            INSERT INTO blocks (parent, name, block_no, data)
              SELECT parent, 'copy.bin', block_no, data FROM blocks WHERE parent = 'dir' AND name = 'orig.bin';
            COMMIT;
            ```

            The new file appears on the mount once the attribute cache
            expires (`--attr-ttl`, default 1 s).

            ## Preallocate or punch holes

            Not available on a mount: `fallocate` fails with
            `Operation not supported`. To make a file sparse, truncate it
            and rewrite the parts you need, or extend it with
            `truncate -s`, which never stores the new bytes.

            ## Clone with `cp --reflink`

            Not available: `cp --reflink=always` fails with
            `Operation not supported`. Plain `cp` (or
            `cp --reflink=auto`) works.

            ## Search file contents in the database

            Not available as a filesystem interface. For text files you
            can query blocks directly:

            ```sql
            SELECT DISTINCT parent || '/' || name FROM blocks
            WHERE position(convert_to('needle', 'UTF8') in data) > 0;
            ```

            This scans every block, and it misses matches that span a
            block boundary.
        """)

    def reference(self):
        return md(r"""
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
        """)

    def explanation(self):
        return md(r"""
            ## Moving work to where the data is

            In a normal copy, a program reads a file into its own memory
            and writes it back out. On pgfs, that means every byte travels
            from PostgreSQL to the daemon, up to the program through the
            kernel, back down through the kernel to the daemon, and into
            PostgreSQL again: four copies to produce data the database
            already has.

            Linux provides calls that describe intent instead of moving
            bytes. `copy_file_range` means "make this range of that file
            appear here". `fallocate` means "reserve this space" or "this
            range is now empty". A filesystem that understands them can do
            the work where the data lives. For pgfs that is the query
            engine: a copy becomes one `INSERT … SELECT` over block rows,
            and punching a hole becomes one `DELETE`.

            ## The roadmap

            The same idea extends further. With reference-counted blocks,
            a reflink copy (`cp --reflink`) would copy only references,
            and blocks would be duplicated lazily on the first write.
            Cloning a whole directory tree could be one transaction of
            set-based inserts. Content search could use PostgreSQL's
            trigram indexes instead of reading every file through FUSE.
            Each of these is specified as a requirement; none is built
            yet.

            ## Why it is not wired in yet

            The offloading code is written against the inode-keyed
            storage interface, the future of pgfs storage. The mount still
            runs on the path-keyed layout. Exposing the callbacks before
            the FUSE layer moves to the new interface would mean
            implementing them twice, against two schemas and two block
            sizes. The [Dual-Interface Architecture](dualinterfacearchitecture.md)
            page describes that migration.

            ## Correctness before speed

            Offloaded operations must be indistinguishable from the slow
            path: same bytes, same sizes, same holes. That is why the
            backend falls back to block-by-block copying whenever ranges
            are not block-aligned. A fast path that changed results would
            be worse than no fast path.
        """)
