"""
Concurrency inside the daemon and against the database.

Decided in design/design-decisions.md, topic 8. Single-threaded dispatch
is the worst FUSE configuration measured (To FUSE or Not r12). pgfs
dispatches on several workers and shards them by inode, so per-file
order and buffer coherence need no locks (uFS r130). Operations that
touch several inodes lock in one fixed order (HopsFS r11, RefFS r65).
Directory renames meet at one serialisation point, where loops are
checked (InfiniFS r16, Mantle r127). No daemon lock is held across a
database round trip (CetoFS r42).

The `OptimizationRoadmap` feature in `optimize.py` documents these
requirements.
"""

from .err import Req
from .branch import (
    EpochVersionedRows,
)
from .consistency import (
    ConsistencyModel,
)


class InodeShardedWorkers(Req):
    """The daemon runs `--workers` dispatch workers (default: the number
    of CPUs, at most 16). Every request on an inode (`getattr`,
    `setattr`, `open`, `read`, `write`, `flush`, `release`, `fsync`,
    `readdir`, `readdirplus`, `*xattr`) goes to the worker that owns
    its inode, `ino mod workers`. That worker owns the inode's open
    handles and write buffers.

    The daemon's metadata cache (inode attributes, dentries, and an
    atomic per-inode summary `(buffered_size, buffered_mtime_ns,
    open_handles)`) is shared across workers so `getattr` and
    `readdirplus` read buffered sizes without cross-worker round trips
    (`consistency.DrainsBuffersBeforeExternalising`), and the
    change-feed listener thread (`changefeed.ChangeNotifications`) can
    evict entries directly.

    A request on a single name (`lookup`, `create`, `mkdir`, `mknod`,
    `symlink`, `link`, `unlink`, `rmdir`) goes to `hash(parent_ino,
    name) mod workers` (using `(newparent_ino, newname)` for `link`), so
    operations on the same name are serialised while concurrent creates
    or unlinks of different names in one directory fan out across all
    workers. A request on two names (`rename`) goes to the worker of the
    lower `(parent_ino, name)` hash. `copy_file_range` goes to `ino_out
    mod workers`. Before starting a `rename`, `link`, `unlink` or
    `copy_file_range` transaction, the worker checks the daemon's
    metadata cache for the affected inode(s) (`source_ino` and any
    overwritten `target_ino`, `link`'s `ino`, or `ino_in`) and, if any
    has buffered writes on another worker, messages that worker (`ino
    mod workers`) to commit its buffer first (`NoLockAcrossRoundTrip`).
    In addition, `pgfs.rename` and `pgfs.unlink` take the set of inodes
    that currently have dirty buffers anywhere on the mount (from the
    shared per-inode summary, or `NULL` when none are dirty): after
    `LockOrder` step 3, if a resolved `source_ino` or `target_ino` is in
    that dirty set and was not pre-flushed (for example because a cache
    miss skipped pre-resolution or another worker concurrently renamed a
    dirty file onto the target name), the SQL function returns
    `retry: inos = …` without committing, and the worker flushes those
    inodes and calls again (`consistency.OneRoundTripPerOperation`).

    **Postcondition.** Requests on one inode, or on one `(parent_ino,
    name)`, are applied in the order the kernel sent them.

    **Concurrency.** No lock protects per-inode write buffers or handle
    tables, because one worker thread owns each inode's mutable state.

    **Sources.** uFS r130, To FUSE or Not r12, RFUSE r17.
    """

    deps = [ConsistencyModel]


class WorkerOwnsConnection(Req):
    """Each worker owns one database connection, with its prepared
    statements (`PreparedStatementReq`), for its whole life. Background
    tasks (lease renewal, housekeeping, the change-feed listener) have
    connections of their own. The total on the primary is `workers + 3`
    (plus `workers + 1` standby connections when `--replica` is enabled,
    `replica.ServesReadsFromReplica`), and the startup log reports it.

    **Bounds.** Connections per mount are fixed and stated. A mount
    never opens connections on demand.
    """

    deps = [InodeShardedWorkers]


class LockOrder(Req):
    """Inside a transaction that locks several rows, rows are locked in
    this order:

    1. the layer gate, shared (`branch.EpochVersionedRows`);
    2. the rename lock, for directory renames across parents
       (`RenameSerialisationPoint`);
    3. inode rows, in ascending `ino`;
    4. dentry rows, in ascending `(parent_ino, name)`;
    5. block map rows, in ascending `(ino, block_no)`;
    6. blob rows, in ascending `blob_id`.

    Functions that need a row later in this order never wait for one
    earlier in it. Because name-based operations (`unlink`, `rmdir`,
    `rename`) do not know the child or overwritten target `ino`
    beforehand, they execute a two-phase discover-sort-lock-revalidate
    protocol (Frangipani 12, HopsFS r11, Chardonnay r66):

    1. **Discover:** read the relevant `dentries` row(s) without
       `FOR UPDATE` (and for `create`/`mkdir`, allocate the new `ino`
       from the sequence first) to learn all participating inode
       numbers (`parent_ino`, `new_parent_ino`, `child_ino`,
       `target_ino`).
    2. **Lock inodes:** sort all participating existing inode numbers in
       ascending `ino` order and lock their `inodes` rows with
       `SELECT … FOR UPDATE`.
    3. **Lock and revalidate dentries:** lock or insert the `dentries`
       row(s) in ascending `(parent_ino, name)` order and verify that
       each resolved `(parent_ino, name)` still maps to the `ino`
       discovered in step 1 (if a concurrent operation changed it, abort
       and retry from step 1).

    Housekeeping follows the same order.

    **Invariant.** No deadlock between pgfs transactions. One between a
    pgfs transaction and an external client is resolved by Postgres and
    retried (`failure.RetriesSerializationFailures`).

    **Sources.** Frangipani 12, HopsFS r11, RefFS r65, Chardonnay r66,
    LockFS r133.
    """

    deps = [EpochVersionedRows]


class RenameSerialisationPoint(Req):
    """A rename that moves a directory to a different parent takes a
    transaction-level advisory lock for the layer, then checks that the
    destination is not inside the source by walking the destination's
    ancestors. The check and the update are in one transaction, so two
    concurrent renames cannot form a cycle. Renames within one directory,
    and renames of non-directories, do not take this lock.

    **Errors.** EINVAL if the destination is inside the source.

    **Bounds.** The ancestor walk is one recursive query, proportional
    to the destination's depth.

    **Sources.** InfiniFS r16, Mantle r127.
    """

    deps = [LockOrder]


class NoLockAcrossRoundTrip(Req):
    """No daemon-side lock or guard is held while a database query runs.
    Work that needs other workers' state, for example commits of their
    buffers before a rename, is requested by message and awaited before
    the transaction starts. While a worker waits on another worker, it
    keeps servicing incoming commit-buffer requests from its peer
    mailbox, never new FUSE requests (serving new FUSE requests would
    reorder requests on one inode and make the wait re-entrant; handling
    a commit-buffer request never sends a cross-worker message, so wait
    chains have depth 1 and cannot form a cycle).

    **Sources.** CetoFS r42, Chardonnay r66 (read before you lock).
    """

    deps = [InodeShardedWorkers]


class BoundedWriteBuffers(Req):
    """Per-inode write buffers are bounded. A buffer is committed when
    it holds `--buffer-per-file` (default 8 MiB). When the mount's total
    reaches `--buffer-total` (default 256 MiB), the oldest buffers are
    committed before new writes are accepted (workers awaiting room
    continue servicing peer commit-buffer messages so two workers under
    buffer pressure cannot deadlock, `NoLockAcrossRoundTrip`). A write
    waits for that room rather than growing memory without limit.

    **Bounds.** Buffered data on a mount never exceeds the total plus
    one write.

    **Sources.** ScaleCheck r57.
    """

    deps = [InodeShardedWorkers]
