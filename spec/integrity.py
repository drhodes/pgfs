"""
Integrity, checking, scrubbing, backup and portable export.

Decided in design/design-decisions.md, topic 12. Redundancy does not
imply fault tolerance: storage stacks propagate corruption faithfully
(CORDS r121, Torturing Databases r120). pgfs therefore checks its data
end to end, verifies its own invariants with an fsck written as SQL
(pFSCK r54, T2C r58), scrubs in the background, and uses Postgres's own
backup machinery rather than inventing a second one.
"""

from .err import PARTIAL, Feat, Req, md
from .branch import (
    CreatesSnapshot,
    RetainedEpochs,
    SnapshotsAreImmutable,
)
from .consistency import (
    BoundedTransactions,
)
from .space import (
    TableTuning,
    TracksUsage,
)
from .storage import (
    ImmutableBlobs,
)


class BlobChecksums(Req):
    """The daemon computes the CRC32C of every blob before inserting it,
    and stores it in `blobs.crc32c`. It verifies the checksum on every
    read before returning data. A mismatch:

    - replies EIO;
    - logs ERROR with the tree, path, inode, block number, blob id, and
      the expected and actual checksums;
    - increments `integrity_checksum_failures`.

    It is never retried against the same blob, and never repaired
    silently.

    **Invariant.** No byte reaches a reader unless it matches the
    checksum computed when it was written.

    **Bounds.** CRC32C runs at memory speed with hardware support. Its
    cost is reported by the benchmark suite.

    **Sources.** CORDS r121, Torturing Databases r120, BlueStore r33
    (full data checksums).
    """

    deps = [ImmutableBlobs]


class PgfsCheck(Req):
    """`pgfs check [TREE]` verifies the stored state with SQL queries and
    reports every violation, with the rows involved:

    1. every live dentry resolves to a live inode, its `kind` matches,
       and no live dentry (including a whiteout) has a whited-out
       directory as its parent (`storage.DentryTable`);
    2. every live inode other than `trees.root_ino` is reachable from
       `trees.root_ino` (for a subtree branch, reachability and link
       counts are scoped to `trees.root_ino`,
       `branch.ViewsRootedAtDirectory`), every directory except
       `trees.root_ino` has exactly one name, and the directory graph
       has no cycle;
    3. `nlink` equals the names, or the subdirectories plus 2;
    4. no inode with non-null `inline_data` has map rows or
       `size > inline_max`, no map row lies at or past its inode's size,
       and every non-null `blob_id` exists;
    5. no unshared blob is unreferenced, apart from blobs queued for
       collection;
    6. every row's `born` is less than its `died`, and no retained
       epoch sees two live versions of one key;
    7. every layer's base exists, depth equals the base's depth plus 1,
       and the base epoch is frozen;
    8. for each snapshot, if `snapshots.digest` is null, computes and
       stores the digest over its visible rows; otherwise verifies that
       the snapshot's row digest equals the stored `snapshots.digest`
       (`branch.SnapshotsAreImmutable`);
    9. usage totals match the live rows (`space.TracksUsage`);
    10. table settings match `space.TableTuning`.

    It also runs `amcheck` (`bt_index_check`) on every pgfs index.
    `--repair` fixes only classes 5 (queues the blobs) and 9 (recomputes
    the totals), and requires that the tree is not mounted read-write.
    Every other violation needs a person.

    **Concurrency.** Read-only. It runs at REPEATABLE READ, so it checks
    one consistent state while mounts keep working.

    **Sources.** pFSCK r54, T2C r58, LazyFS r135 (amcheck finds what
    restart misses).
    """

    deps = [RetainedEpochs, SnapshotsAreImmutable, TracksUsage, TableTuning]


class Scrub(Req):
    """`pgfs scrub` reads every blob and verifies its checksum, at a
    limited rate (`--rate`, default 50 MiB/s). It remembers its
    position, so it resumes after an interruption. With `--continuous`
    it keeps running in the background (for example as a systemd
    service next to the mount) and completes one pass per
    `--scrub-period` (default 30 days). Failures are reported exactly
    as read failures are (`BlobChecksums`).

    **Sources.** NVMe failures in the field r88, CORDS r121.
    """

    deps = [BlobChecksums]


class BackupIsPostgresBackup(Req):
    """pgfs keeps no durable state outside its database. The daemon's
    memory holds only caches and uncommitted buffers. So any consistent
    Postgres backup is a consistent pgfs backup: `pg_basebackup` with
    WAL archiving, pgBackRest, or a storage snapshot of the data
    directory. Point-in-time recovery restores the filesystem as of any
    moment, in the crash-consistent sense of
    `durability.PrefixCrashConsistency`. The Integrity how-to gives the
    procedure. A restored copy keeps the original's `fs_uuid`.
    `pgfs tune --new-uuid` gives it a new one, which is required before
    both are mounted on one machine, because the kernel and NFS key
    caches by the filesystem id. A restored copy also starts with the
    original's leases, which expire normally.
    """


class ExportsSnapshots(Req):
    """`pgfs export TREE@SNAP [--path DIR]` writes a POSIX tar stream
    (pax format) of a snapshot to standard output. It preserves names as
    bytes, modes, owners, nanosecond times, hard links, symlinks,
    special files, sparse regions and xattrs. It reads from a snapshot,
    so the archive is consistent while the tree keeps changing.

    **Postcondition.** Extracting the archive on ext4 reproduces the
    snapshot. `verify.ModelConformance` checks this round trip.
    """

    deps = [CreatesSnapshot]


class ImportsArchives(Req):
    """`pgfs import TREE[:/DIR]` reads a tar stream on standard input
    into a directory of a tree that has no read-write mount. It commits
    in bounded batches, and makes the result visible only at the end by
    renaming a hidden staging directory into place. A failed import
    therefore leaves nothing visible.

    **Bounds.** Each batch is one bounded transaction
    (`consistency.BoundedTransactions`).
    """

    deps = [BoundedTransactions]


# ── Feature ───────────────────────────────────────────────────────────

class DataIntegrity(Feat):
    """Every block pgfs returns is checked against a checksum taken when
    it was written. `pgfs check` verifies the filesystem's invariants,
    `pgfs scrub` finds silent corruption before a reader does, and
    backups use Postgres's own tools or a portable tar export of a
    snapshot."""

    deps = [
        BlobChecksums,
        PgfsCheck,
        Scrub,
        BackupIsPostgresBackup,
        ExportsSnapshots,
        ImportsArchives,
    ]

    def title(self):
        return "Integrity and Backup"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "Blob checksums are verified on every read, `pgfs check` "
            "covers every invariant class plus amcheck and seals "
            "snapshots, `pgfs scrub` resumes from its position and runs "
            "continuously with `--continuous`, and `pgfs export` / `pgfs "
            "import` round-trip names, modes, owners, times, links, "
            "special files and xattrs. Not yet: pax sparse headers "
            "(export writes holes as zero bytes; import turns zero blocks "
            "back into holes)."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will check a filesystem, scrub it, and
            take a portable backup of a snapshot.

            ## Step 1: check

            ```bash
            pgfs check
            ```

            ```text
            checked 3 trees, 41 210 inodes, 9 snapshots: no violations
            amcheck: 14 indexes ok
            ```

            ## Step 2: scrub

            ```bash
            pgfs scrub --rate 200M
            ```

            ```text
            scrubbed 18.4 GiB in 94 s: 0 checksum failures
            ```

            ## Step 3: export a snapshot

            ```bash
            pgfs snapshot main@backup-1
            pgfs export main@backup-1 | zstd > main-backup-1.tar.zst
            ```

            The archive is a plain tar file that any system can
            extract.
        """)

    def how_to(self):
        return md(r"""
            ## Back up the whole filesystem

            Back up the Postgres cluster with WAL archiving, for example
            with pgBackRest or `pg_basebackup`. Nothing else needs
            saving.

            ## Restore to a point in time

            Restore the cluster with point-in-time recovery to the
            moment you want, then mount it. The filesystem is exactly
            as a crash at that moment would have left it. To mount the
            restored copy next to the original on one machine, first
            give it a new identity:

            ```bash
            pgfs tune --conn "$RESTORED" --new-uuid
            ```

            ## Move a tree to another pgfs filesystem

            ```bash
            pgfs snapshot main@move
            pgfs export main@move | ssh other pgfs import main
            ```

            ## Respond to a checksum failure

            The error log names the path and block. Restore that file
            from a snapshot or a backup, and run `pgfs scrub` to find
            any others. Check the database host's disks.
        """)

    def reference(self):
        return md(r"""
            ## pgfs check classes

            | # | Invariant | `--repair` |
            |---|---|---|
            | 1 | dentries resolve to inodes of the right kind | no |
            | 2 | one name per directory; no cycles | no |
            | 3 | link counts | no |
            | 4 | block maps within size; blobs exist | no |
            | 5 | no unreferenced unshared blobs | queues them |
            | 6 | epoch ranges well formed | no |
            | 7 | layer chains well formed | no |
            | 8 | snapshot digests unchanged | no |
            | 9 | usage totals | recomputes |
            | 10 | table settings | no |

            ## Exit status

            | Status | Meaning |
            |---|---|
            | 0 | no violations |
            | 1 | violations found |
            | 2 | could not run |

            ## Scrub options

            | Option | Default |
            |---|---|
            | `--rate` | 50 MiB/s |
            | `--continuous` | off |
            | `--scrub-period` | 30 days |
        """)

    def explanation(self):
        return md(r"""
            ## Why checksum on top of Postgres

            Postgres's page checksums catch a damaged page on disk. They
            do not catch a bug that writes the wrong bytes, a damaged
            TOAST chunk read through a valid page, or a standby that
            replicated the damage. Studies of real storage stacks found
            that redundancy usually spreads corruption rather than
            stopping it. A checksum computed by the daemon when data is
            written, and checked when it is read, covers the whole path.

            ## Why fsck is SQL

            Every invariant of a pgfs filesystem is a statement about
            rows, so each can be written as a query that returns the
            rows that break it. The checker runs on a consistent
            snapshot of the database while the filesystem stays
            mounted.

            ## Why backup is Postgres's job

            pgfs's whole state is in its database. Postgres already has
            mature, tested backup and point-in-time recovery, and a
            second backup system would only add a second way to be
            wrong. pgfs adds what Postgres cannot: a portable,
            file-level export of a consistent snapshot.
        """)
