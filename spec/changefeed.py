"""
The change feed: a causally ordered, resumable, exactly-once stream of
every change to a tree (conflicting changes arrive in commit order;
changes to unrelated objects may arrive in any order). The difference
between two snapshots is computed from epochs instead, and is specified
in `branch.SnapshotDiff`.

Decided in design/design-decisions.md, topic 1b. The feed serves sync,
search indexing, backup and audit today. It is also the cache-coherence
channel the multi-machine version needs. It is a transactional outbox:
each change is a row written in the same transaction as the change, so
the feed can never disagree with the filesystem.

Ordering subtlety: neither `seq` nor `xid` alone equals commit order —
a transaction can allocate an earlier `xid` and commit after another
transaction on the same inode, or allocate a higher `seq` on an
unrelated inode and commit before a transaction with `xid ≥ h`. Delivery
therefore advances by a transaction-id horizon `h = pg_snapshot_xmin(S)`
combined with a sequence cap `s_cap` read from the **same** snapshot `S`
(holding back any row with `xid < h` whose `seq` exceeds that of an
already-visible row with `xid ≥ h`). Change feeds without gap detection
silently lose events (Skybridge r59), so a consumer that falls behind
retention or whose tree moves layers via `--force` promote is told,
never skipped forward.
"""

from .err import STABLE, Feat, Req, md
from .consistency import (
    OneOperationOneTransaction,
)
from .storage import (
    OperationsAsSqlFunctions,
)


class RecordsChanges(Req):
    """Every mutating function inserts rows into
    `changes(seq bigint, layer, xid xid8, at, op, ino, parent_ino, name,
    new_parent_ino, new_name, path, new_path, kind)` in its own
    transaction. `seq` comes from a sequence allocated after all row
    locks (`concurrency.LockOrder`) are held, so conflicting operations
    receive `seq` in their lock and commit serialisation order, and
    `xid` is `pg_current_xact_id()`. `path` (and `new_path` for
    `rename` and `exchange`) is resolved inside the mutating
    transaction while the ancestor chain is live, so a later rename or
    directory deletion never breaks or alters an earlier event's path.
    The `op` values are `create`, `mkdir`, `mknod`, `symlink`, `link`,
    `unlink`, `rmdir`, `rename`, `exchange`, `setattr`, `setxattr`,
    `removexattr` and `modify`. Data writes record one `modify` per
    inode per transaction, not one per write.

    **Invariant.** A change row exists if and only if its change
    committed.

    **Bounds.** At most one feed row per inode touched by an operation.
    """

    deps = [OperationsAsSqlFunctions, OneOperationOneTransaction]


class HorizonOrderedDelivery(Req):
    """The feed is read in batches by transaction-id horizon and
    sequence cap, represented by an opaque watermark token
    `(layer, low_xid, high_xid, s_cap, last_seq, held_back)`.

    When starting a new window (`low_xid = high_xid`), the reader takes
    one MVCC snapshot `S` and in that same snapshot reads both the
    horizon `h = pg_snapshot_xmin(S)` and the sequence cap `s_cap`, the
    minimum `seq` among visible rows in `layer` with `xid ≥ h` (or null
    if none exist). Computing `s_cap` in the same snapshot `S` that
    defines `h` is what makes the cap sound: every row with `xid < h` is
    already committed and visible in `S`, and any conflicting row with a
    lower `seq` had to commit before it and is therefore also visible in
    `S`.

    Each page call reads from the candidate set
    `held_back ∪ {low_xid ≤ xid < high_xid}` the rows with
    `seq > last_seq AND (s_cap IS NULL OR seq < s_cap)`, ordered by
    `seq`, `LIMIT limit`, and returns a token advancing `last_seq` (with
    `s_cap` kept fixed for the window). When the last page of the window
    is exhausted, the token advances to `low_xid = high_xid`,
    `last_seq = 0`, and `held_back` set to the window's rows that had
    `xid < high_xid` and `seq ≥ s_cap` so they are delivered in the next
    window once the `xid ≥ h` transactions below them have cleared.

    **Postcondition.** Every committed change is delivered exactly once
    to a consumer that acknowledges each batch, and no aborted change is
    delivered. Conflicting changes (touching a common inode or directory
    entry) are delivered in commit order across batches as well as
    within a batch; changes to unrelated objects may arrive in any
    order.

    **Bounds.** The delay of a change is the age of the oldest open
    write transaction in the cluster (`h` does not advance past an open
    `xid`, including a large fold-back `branch.PromotesBranch`
    transaction; read-only transactions such as `pgfs check` and
    `pgfs export` allocate no `xid` and never hold back `h`).
    Housekeeping and flatten transactions are short bounded batches for
    that reason (`consistency.HousekeepingInSeparateTransactions`).
    """

    deps = [RecordsChanges]


class ResumableCursors(Req):
    """A consumer registers a name bound to a tree with
    `pgfs changes register [--at-snapshot TREE@NAME] NAME TREE`, stored
    in `change_consumers(name, tree, layer, watermark, updated_at)`
    (with `--at-snapshot`, `watermark` starts at an initial token
    anchored at `snapshots.xid`, `branch.CreatesSnapshot`).
    `pgfs.changes_next(name, limit)` returns the next page and its
    opaque watermark token, and `pgfs.changes_ack(name, watermark)`
    advances the stored token. A consumer that restarts resumes from its
    last acknowledged watermark token, so it may see the final
    unacknowledged page again, but never misses or skips rows when a
    window exceeds `limit`.

    A normal fold-back `branch.PromotesBranch` keeps `TREE` on the same
    `layer` and writes the branch's net changes into `changes` on
    `TREE.layer` under the promote's `xid` and `seq`, so consumers
    continue seamlessly. If a `--force` promote moves `TREE` to a
    different layer (`trees.layer ≠ change_consumers.layer`),
    `pgfs.changes_next` fails with SQLSTATE `PF011` (`tree moved: resync
    from TREE@pre-promote-<ts>`); delivery never stops silently.
    """

    deps = [HorizonOrderedDelivery]


class ChangeNotifications(Req):
    """Mutating transactions do not execute `NOTIFY` (in Postgres,
    `PreCommit_Notify` acquires a cluster-wide `AccessExclusiveLock` on
    database 0 across the commit WAL flush and synchronous replication
    wait, which would serialise every notifying commit on the server).
    Instead, each mount's housekeeping connection sends at most one
    coalesced `NOTIFY pgfs_changes` with the layer id as payload per
    50–100 ms interval when `changes` has advanced, and CLI commands
    that mutate without a mount (`snapshot`, `branch`, `promote`,
    `import`) send one `NOTIFY` when they finish. Consumers `LISTEN` and
    wake to fetch the next batch. The notification is only a hint: it
    carries no data, consumers also poll every `--poll` (default 5 s),
    and a lost notification only delays delivery.

    **Sources.** ZooKeeper r24 (watches as one-shot hints), Skybridge
    r59.
    """

    deps = [RecordsChanges]


class ChangeRetention(Req):
    """Housekeeping deletes change rows that are both older than
    `--change-retention` (default 7 days) and below the `low_xid` (and
    not in `held_back`) of every registered consumer of their layer. A
    consumer whose watermark references rows older than the oldest
    retained row is expired. Its next fetch fails with SQLSTATE `PF010`
    (`cursor expired`), and it must resync from a snapshot. Delivery is
    never skipped forward silently.

    A consumer that is idle for longer than `--consumer-expiry`
    (default 30 days) is unregistered, so an abandoned consumer cannot
    hold the feed forever. Its removal is logged at WARN.

    **Sources.** Skybridge r59 (gap detection).
    """

    deps = [ResumableCursors]


class ChangesCommand(Req):
    """`pgfs changes TREE [--since WATERMARK] [--follow] [--consumer
    NAME]` prints changes as JSON lines, one object per change. Each
    object carries the fields of `RecordsChanges`, with `path` and
    `new_path` as recorded at the moment of the change, and the page's
    opaque `watermark` token (`--since` accepts a previously printed
    token). `--follow` keeps reading as changes arrive. With
    `--consumer`, each page is acknowledged after it is printed.
    """

    deps = [ResumableCursors]


# ── Feature ───────────────────────────────────────────────────────────

class ChangeFeed(Feat):
    """Every change to a tree is recorded in the same transaction that
    makes it, and delivered in causal order, exactly once, to consumers
    that can stop and resume. Snapshot comparison (`pgfs diff`,
    specified in Branchable Trees) complements it for changes older than
    the feed's retention."""

    deps = [
        RecordsChanges,
        HorizonOrderedDelivery,
        ResumableCursors,
        ChangeNotifications,
        ChangeRetention,
        ChangesCommand,
    ]

    def title(self):
        return "Change Feed"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "Every mutating function records its change with paths "
            "resolved in the transaction; delivery by transaction-id "
            "horizon and sequence cap, resumable consumers, retention, "
            "coalesced notifications from housekeeping and `pgfs changes` "
            "are built and tested in `tests/storage.rs`."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will follow changes to a tree as they
            happen, then compare two snapshots. We need `main` mounted at
            `/mnt/main`.

            ## Step 1: start following

            ```bash
            pgfs changes main --follow
            ```

            ## Step 2: make changes

            In another terminal:

            ```bash
            mkdir /mnt/main/docs
            echo draft > /mnt/main/docs/a.md
            mv /mnt/main/docs/a.md /mnt/main/docs/b.md
            ```

            The first terminal prints:

            ```text
            {"op":"mkdir","path":"/docs","ino":1040,"watermark":"v1:1:771:771:0:101:"}
            {"op":"create","path":"/docs/a.md","ino":1041,"watermark":"v1:1:772:772:0:102:"}
            {"op":"modify","path":"/docs/a.md","ino":1041,"watermark":"v1:1:773:773:0:103:"}
            {"op":"rename","path":"/docs/a.md","new_path":"/docs/b.md","ino":1041,"watermark":"v1:1:774:774:0:104:"}
            ```

            ## Step 3: compare snapshots

            ```bash
            pgfs snapshot main@before
            rm /mnt/main/docs/b.md
            pgfs diff main@before main
            ```

            ```text
            removed  /docs/b.md
            ```

            ## What we did

            We watched changes arrive in order and compared two points
            in time, without walking the tree.
        """)

    def how_to(self):
        return md(r"""
            ## Keep an external index in sync

            ```bash
            pgfs changes register search-indexer main
            pgfs changes main --consumer search-indexer --follow | my-indexer
            ```

            After a restart the indexer resumes where it left off.

            ## Recover a consumer that fell too far behind

            If a fetch fails with `cursor expired`, rebuild from a
            snapshot and restart from its watermark:

            ```bash
            pgfs snapshot main@resync
            # rebuild the index from a read-only mount of main@resync
            pgfs changes register --at-snapshot main@resync search-indexer main
            ```

            ## Read the feed from SQL

            ```sql
            SELECT * FROM pgfs.changes_next('search-indexer', 1000);
            SELECT pgfs.changes_ack('search-indexer', 'v1:1:774:774:0:104:');
            ```
        """)

    def reference(self):
        return md(r"""
            ## Change record

            | Field | Meaning |
            |---|---|
            | `op` | create, mkdir, mknod, symlink, link, unlink, rmdir, rename, exchange, setattr, setxattr, removexattr, modify |
            | `ino`, `kind` | the inode and its kind |
            | `path`, `new_path` | path at the moment of the change |
            | `at` | commit time |
            | `watermark` | the page's opaque watermark token, for acknowledgement or `--since` |

            ## Options

            | Option | Default |
            |---|---|
            | `--change-retention` | 7 days |
            | `--consumer-expiry` | 30 days |
            | `--poll` | 5 s |

            ## Errors

            | Error | Meaning |
            |---|---|
            | `PF010 cursor expired` | the consumer fell behind retention; resync from a snapshot |
            | `PF011 tree moved` | `--force` promote moved the tree to another layer; resync from `TREE@pre-promote-<ts>` |
        """)

    def explanation(self):
        return md(r"""
            ## Written with the change, not after it

            Each change record is inserted by the same transaction that
            makes the change. If the change commits, so does its record;
            if it rolls back, neither exists. A feed built any other way
            can disagree with the filesystem after a crash.

            ## Why a horizon instead of a sequence number

            Sequence numbers are handed out when rows are inserted, but
            transactions commit in a different order. A consumer that
            reads "everything after 100" can miss 99 if its transaction
            commits a moment later. The feed advances by the oldest
            transaction still running instead: below that point nothing
            can change any more, so each batch is final.

            ## Why expired consumers are told

            A consumer that silently skips events builds a wrong index
            and never knows. pgfs keeps changes until every registered
            consumer has them, up to a limit, and tells a consumer that
            passed the limit to resync.
        """)
