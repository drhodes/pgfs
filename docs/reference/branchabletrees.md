# Reference: Branchable Trees

!!! success "Status: Stable"
    Snapshots, branches (also of a subdirectory), copy-on-write through layered lookup, owner-drained and crash-consistent snapshots, read-only snapshot mounts, the depth cap with background auto-flatten, online flatten, normal and forced promote, deletion with a grace period, retained epochs and `pgfs diff` are built and covered by `tests/storage.rs`, the model-conformance suite and `test_snapshots.sh`.

> Freeze any pgfs tree in constant time, branch a writable copy of it in constant time, and atomically publish a branch as the new contents of a tree. Branches share every unchanged byte with their base and store only what they change.

[Tutorial](../tutorials/branchabletrees.md) · [How-to](../how-to/branchabletrees.md) · **Reference** · [Explanation](../explanation/branchabletrees.md)

---

## Commands

| Command | Effect | Cost |
|---|---|---|
| `pgfs snapshot TREE@NAME` | freeze TREE, after the owner drains | constant |
| `pgfs snapshot --crash-consistent TREE@NAME` | freeze without draining | constant |
| `pgfs snapshot list TREE` | list snapshots | — |
| `pgfs snapshot delete TREE@NAME` | remove the name; reclaim later | constant |
| `pgfs branch SOURCE NEW` | new writable tree from a snapshot or tree | constant |
| `pgfs branch SOURCE:/DIR NEW` | branch rooted at DIR | constant |
| `pgfs promote [--force] BRANCH TREE` | make BRANCH the contents of TREE | rows changed in BRANCH (constant with `--force`) |
| `pgfs diff TREE@A TREE@B` | what changed between two views | rows changed |
| `pgfs tree list` / `delete` / `flatten` | manage trees | constant / constant / metadata size |
| `pgfs mount TREE[@SNAP][:/DIR] MOUNTPOINT` | mount a view | — |

The same operations are SQL functions: `pgfs.snapshot`,
`pgfs.branch`, `pgfs.promote`, `pgfs.delete_snapshot`,
`pgfs.delete_tree`.

## Errors

| Error | When |
|---|---|
| `EEXIST` | the tree or snapshot name exists |
| `ENOENT` | the source does not exist |
| `EMLINK` | the branch would be deeper than 8 |
| `ESTALE` | promote: the tree changed after the branch point (without `--force`) |
| `EBUSY` | promote or delete: an active mount session, snapshot or dependent branch exists |
| `EINVAL` | promote: the branch is not a matching branch of the target tree |
| `ETIMEDOUT` | the owning daemon did not drain in time |
| `EROFS` | a write to a snapshot |

## Limits and defaults

| Setting | Value |
|---|---|
| maximum branch depth | 8 |
| automatic flatten above depth | 4 (`--auto-flatten-depth`) |
| drain timeout | 30 s (`--timeout`) |
| name rule | `[A-Za-z0-9._-]{1,64}`, not starting with `.` |


## Specification requirements

`BranchableTrees` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "TreesTable"
    `spec.branch.TreesTable`

    `trees(name text primary key, layer bigint, root_ino bigint,
    created_at)` names the writable trees of a filesystem. mkfs creates
    one tree, `main`. A tree name is 1 to 64 characters from
    `[A-Za-z0-9._-]` and does not start with `.`. A tree always points to
    exactly one layer, and no two trees point to the same layer.

    **Invariant.** Each layer is the live layer of at most one tree.

    **Errors.** An invalid tree name is rejected with a message giving
    the rule. Creating a tree whose name exists fails with EEXIST.

??? abstract "LayersTable"
    `spec.branch.LayersTable`

    `layers(layer bigint primary key, base_layer bigint, base_epoch
    bigint, depth integer, epoch bigint, state text)` records each
    layer. A layer with no base has depth 0. A branch layer's base is the
    snapshot `(base_layer, base_epoch)`, and its depth is one more than
    its base's. `epoch` is the layer's current epoch; it starts at 1 and
    only grows. `state` is `active` or `deleting`.

    **Invariant.** The base graph is acyclic. A base epoch is always
    lower than the base layer's current epoch, so the base is frozen.

??? abstract "EpochVersionedRows"
    `spec.branch.EpochVersionedRows`

    Every row of `inodes`, `dentries`, `file_blocks` and `xattrs`
    carries `born` and `died` epochs of its layer. A live row has
    `died = ∞` (`9223372036854775807`). A view `(layer, E)` sees a row
    when `born ≤ E < died`.

    Mutations separate two questions:

    - **Within the layer**, the epoch decides whether the previous row
      is retired (`died = current_epoch`) or changed in place. A row
      born in the layer's current epoch is updated or (when
      `shadows_base = false`) deleted in place, because no snapshot of
      this layer can see it. A row born in an earlier epoch is never
      changed: it is retired by setting `died` to the current epoch, and
      a new row born in the current epoch replaces it if the operation
      needs one.
    - **Between layers**, `shadows_base boolean` on `dentries`, `xattrs`
      and `inodes` rows (and `block_no < inode.inherit_limit` on
      `file_blocks`) decides whether removing a key leaves a whiteout.
      It is `true` when the frozen base view `(base_layer, base_epoch)`
      can see the key. Deleting a key with `shadows_base = true` always
      leaves a live whiteout (`died = ∞` — null `ino`, null `value`, or
      an `nlink = 0` inode row), even when `born = current_epoch`
      (updated in place to a whiteout). Deleting a key with
      `shadows_base = false` removes a row born in the current epoch in
      place (or retires a row born in an earlier epoch without inserting
      a replacement).

    Setting `shadows_base` never probes the base chain on delete: rows
    made by copy-up, by replacing an inherited entry, or by updating an
    existing whiteout get `true`; a fresh insert after a negative
    `LayeredLookup` (or a newly allocated `ino`) gets `false`. Because
    `(base_layer, base_epoch)` is frozen, the flag stays valid until the
    base changes: `FlattensBranch` detach resets every `shadows_base` to
    `false`, and `PromotesBranch` fold-back sets
    `new.shadows_base = COALESCE(tree_row.shadows_base, branch_row.shadows_base)`
    and `inherit_limit = LEAST(tree.inherit_limit, branch.inherit_limit)`.

    Every mutating function first takes the layer gate: a shared
    transaction-level advisory lock on the layer. It then reads the
    current epoch. Snapshot, branch, promote and mount takeover take the
    gate exclusively, so no mutation can read an epoch that a concurrent
    snapshot is about to close.

    **Invariant.** A view `(layer, E)` with `E` below the current epoch
    never changes. Within a layer, the live rows are exactly the rows of
    the view at the current epoch.

    **Concurrency.** The gate is shared, so mutations do not block one
    another on it. A snapshot waits for the mutations already holding it
    and blocks new ones only for the length of its own transaction,
    which touches two rows.

    **Bounds.** A layer that has never been snapshotted creates no
    retired rows. Live reads use partial indexes on `died = ∞`, so a
    tree pays nothing for history it does not have.

    **Sources.** Petal r37, Oracle FSS r91, Spanner r30 (reads at a
    timestamp).

??? abstract "LayeredLookup"
    `spec.branch.LayeredLookup`

    In a view of a branch, every key is resolved through the layer
    chain. A key is a dentry `(parent_ino, name)`, an inode `ino`, a
    block `(ino, block_no)` or an xattr `(ino, name)`. Resolution checks
    the view's own layer at the view's epoch, then the base at its base
    epoch, and so on. The first row found wins:

    - An inode row with `nlink = 0` (when not in `orphans`) is a
      whiteout: the inherited inode is deleted, and resolution stops.
    - A dentry row with a null `ino` is a whiteout: the name is absent,
      and resolution stops.
    - A block map row with a null `blob_id` is a hole: the block reads
      as zero, and resolution stops.
    - A block number at or above the resolved inode's `inherit_limit`
      is never inherited.
    - An xattr row with a null `value` is a whiteout: the attribute is
      absent, and resolution stops.

    `LayeredLookup` never resolves names under a whited-out directory
    (`storage.DentryTable`).

    readdir merges the visible dentries of every layer in the chain for
    the directory, keeps the shallowest row for each name, drops
    whiteouts, and orders by name, so that keyset cookies keep working
    (`ListsDirectories`). `listxattr` merges the visible xattrs of every
    layer in the chain for the inode, keeps the shallowest row for each
    name, and drops whiteouts.

    **Postcondition.** A view of a new branch is byte-for-byte identical
    to its base snapshot: names, inode numbers, attributes, data and
    xattrs.

    **Bounds.** A point lookup costs at most one index probe per layer
    in the chain, sent in one statement. Chain depth is capped
    (`LimitsBranchDepth`).

    **Sources.** BetrFS r55 (lazy clones), Docker and overlayfs
    whiteouts.

??? abstract "CopiesUpOnWrite"
    `spec.branch.CopiesUpOnWrite`

    The first change in a branch to anything inherited copies only
    what changed into the branch's layer:

    - **Attributes, a rename, or a link count.** The inode row, or the
      dentry row, is copied with the same `ino` and `shadows_base = true`,
      and then changed.
    - **Data.** Only the written blocks get map rows in the branch, each
      pointing to a new blob. The untouched blocks of a 10 GB file stay
      inherited.
    - **Truncation.** A downward truncate to size `s` lowers the inode's
      `inherit_limit` to `min(inherit_limit, floor(s / block_size))` (an
      extension never raises `inherit_limit`). If `s` is not
      block-aligned and `floor(s / block_size)` was below the previous
      `inherit_limit`, that partial block is written into the branch
      with its tail zeroed. Blocks past `s` can never reappear after a
      later extension.
    - **A punched hole** over inherited blocks writes hole rows.
    - **Unlinking or renaming a name, or removing an xattr, with
      `shadows_base = true` (or inherited from the base)** leaves a live
      whiteout (`died = ∞`, `shadows_base = true`) for the old name or
      xattr, updating in place if `born = current_epoch`.
    - **Deleting an inode with `shadows_base = true` (or inherited from
      the base).** When `nlink` reaches 0 and no open handle remains, its
      row in the branch is kept as an inode whiteout (`nlink = 0,
      size = 0, inherit_limit = 0, inline_data = NULL,
      shadows_base = true`) and any `file_blocks`, `xattrs` and `orphans`
      rows in the branch's layer are removed (or retired if
      `born < current_epoch`).

    **Postcondition.** After any sequence of operations, a view of the
    branch equals the base snapshot with those operations applied.
    `verify.ModelConformance` checks this against the reference model.

    **Bounds.** Changing one block of an inherited file costs one block
    of new data, one inode row and one map row, whatever the file's
    size.

??? abstract "CreatesSnapshot"
    `spec.branch.CreatesSnapshot`

    `pgfs snapshot TREE@NAME` and `pgfs.snapshot(tree, name)` freeze a
    tree. In one transaction, holding the layer gate exclusively, the
    function inserts
    `snapshots(tree, name, layer, epoch, xid, created_at, digest)` with
    the layer's current epoch and `pg_current_xact_id()` (with `digest`
    initially null), and advances the epoch by one. A
    snapshot of a branch is a snapshot of the branch's layer. Snapshot
    names follow the tree-name rule and are unique per tree.

    **Postcondition.** The snapshot's view shows every operation
    committed before the snapshot, and none committed after it. When the
    tree is mounted, that includes every write the daemon had
    acknowledged (`DrainsOwnerBeforeSnapshot`).

    **Durability.** The snapshot commits with
    `synchronous_commit = on` (verified against `SyncRepWaitForLSN`
    warnings and retried via `pgfs.sync_barrier`,
    `durability.FsyncIsDurable`), so a snapshot that was reported as
    taken survives any crash.

    **Bounds.** Constant time: two rows, whatever the size of the tree.

    **Errors.** An existing name fails with EEXIST. An unknown tree
    fails with ENOENT.

??? abstract "DrainsOwnerBeforeSnapshot"
    `spec.branch.DrainsOwnerBeforeSnapshot`

    When the tree has a read-write mount, the CLI asks that mount's
    daemon to take the snapshot (or branch). The request travels through
    the database, so it works from any machine. The CLI inserts a
    `pending` row into `control_requests` and sends `NOTIFY pgfs_control`.
    The owning daemon then executes a drain barrier:

    1. tells all workers to hold new mutating FUSE requests (each worker
       finishes its current request, then defers new mutating requests
       without replying while still answering FUSE `INTERRUPT` with
       EINTR; this short pause — bounded by the flush of at most
       `--buffer-total` = 64 MiB plus one two-row transaction — is an
       explicit exception to `failure.DatabaseStateMachine`);
    2. each worker commits its dirty inode buffers and confirms;
    3. in one transaction within its fencing epoch, locks the
       `control_requests` row (`SELECT ... FOR UPDATE`): if `state` is
       still `'pending'`, calls `pgfs.snapshot` (or `pgfs.branch`),
       records the result, and sets `state = 'done'`; if `state` is
       `'cancelled'`, does nothing;
    4. releases the workers.

    The CLI waits up to `--timeout` (default 30 s). With no read-write
    mount, the CLI calls `pgfs.snapshot` itself.

    **Errors.** If the owner does not answer in time, the CLI runs
    `UPDATE pgfs.control_requests SET state = 'cancelled' WHERE
    request_id = $1 AND state = 'pending'` in `READ COMMITTED`. If 1 row
    is updated, nothing was or will be created and the CLI exits
    non-zero with ETIMEDOUT; if 0 rows are updated, the snapshot
    transaction already committed `'done'` and the CLI reports success.
    `--crash-consistent` bypasses the owner (`CrashConsistentSnapshot`).

    **Sources.** Frangipani 12 (a barrier gives a filesystem-consistent
    backup), AFS 05 (clone after a brief freeze).

??? abstract "CrashConsistentSnapshot"
    `spec.branch.CrashConsistentSnapshot`

    `pgfs snapshot --crash-consistent` calls `pgfs.snapshot` directly,
    without asking the owning daemon. The snapshot then holds exactly
    what a crash at that instant would have preserved: every committed
    operation, but no writes still buffered in a daemon. This is the
    same guarantee as an LVM or cloud-volume snapshot, and the fallback
    when the owner is unreachable.

    **Postcondition.** The view equals a state in the crash set of the
    tree at the instant of the snapshot (`durability.PrefixCrashConsistency`).

??? abstract "SnapshotsAreImmutable"
    `spec.branch.SnapshotsAreImmutable`

    Nothing can change a snapshot. A mount of a snapshot is
    read-only, and every mutating callback replies EROFS. Every mutating
    SQL function refuses a view whose epoch is below its layer's current
    epoch, so psql and buggy clients cannot change one either. Only
    `pgfs snapshot delete` removes a snapshot.

    **Invariant.** The bytes, names and attributes visible in a snapshot
    never change while the snapshot exists. `snapshots.digest` starts
    null so `CreatesSnapshot` stays constant-time; the first
    `pgfs check` (or `pgfs snapshot --seal`) computes and stores the
    digest over the snapshot's visible rows, and every later
    `pgfs check` verifies that the snapshot's rows still match that
    stored digest (`integrity.PgfsCheck`).

??? abstract "MountsSnapshots"
    `spec.branch.MountsSnapshots`

    `pgfs mount TREE@NAME MOUNTPOINT` mounts a snapshot read-only. Any
    number of daemons on any machines may mount the same snapshot at
    once. Each registers a read-only session in `mounts`
    (`mode = 'ro_snapshot'`) on the primary and renews it on the lease
    schedule so `DeletesSnapshotsAndTrees`, `space.ReclaimsRetiredRows`
    and `ops.MigratesSchema` see the active reader
    (`consistency.ExclusiveTreeLease`). If renewal fails past
    `expires_at`, the daemon fails closed with EIO
    (`consistency.FencesStaleWriters`), and on every reconnect it
    re-checks that its `snapshots(tree, name)` row still exists
    (`failure.RevalidatesAfterReconnect`). Entry and attribute TTLs are
    unbounded while the session is valid, because nothing in a snapshot
    can change. Reads may go to any standby on which the snapshot's row
    in `snapshots` is visible (`replica.SnapshotReadsFromAnyStandby`).

    **Errors.** A write replies EROFS.

??? abstract "CreatesBranch"
    `spec.branch.CreatesBranch`

    `pgfs branch SOURCE NEW` and `pgfs.branch(source, new)` create a
    writable tree named NEW. The source is a snapshot (`main@nightly`)
    or a tree (`main`). When the source is a live tree, after the owning
    daemon has drained (`DrainsOwnerBeforeSnapshot`), the function
    holds the source layer's gate exclusively, advances the source
    layer's `epoch` by one, and uses the closed epoch as `base_epoch`
    without inserting a named row into `snapshots` (`RetainedEpochs`
    retains `base_epoch` for as long as `NEW.layer` references it, and
    releases it when `NEW` is deleted or folded back). The function
    inserts a layer whose base is `(base_layer, base_epoch)` and a
    `trees` row pointing to it.

    **Postcondition.** A mount of NEW shows exactly the base epoch's
    view (`LayeredLookup`). Writes to NEW never change the source, and
    writes to the source never appear in NEW.

    **Durability.** The branch commits with `synchronous_commit = on`
    (verified via `pgfs.sync_barrier` on a `SyncRepWaitForLSN` warning,
    `durability.FsyncIsDurable`).

    **Bounds.** Constant time: two rows, whatever the size of the
    tree.

    **Errors.** EEXIST if NEW exists. ENOENT if the source does not.
    EMLINK if the new depth would exceed the cap (`LimitsBranchDepth`).

??? abstract "ViewsRootedAtDirectory"
    `spec.branch.ViewsRootedAtDirectory`

    A mount or a branch may be rooted at a directory inside a tree:
    `pgfs mount main:/projects/web MOUNTPOINT`, or
    `pgfs branch main@nightly:/projects/web job-17`. The view's root is
    that directory, and kernel inode 1 maps to it. Database inode 1,
    the tree root, is never reachable from inside such a view, so the
    swap is safe.

    A branch rooted at a directory is a branch of the whole tree whose
    `trees.root_ino` names the directory. Branching any subtree
    therefore costs the same as branching a tree.

    When an inherited file inside a subtree branch is copied up, or when
    `unlink` decrements its `nlink`, if no remaining live dentry for the
    inode is reachable from `trees.root_ino`, `nlink` drops to 0 so hard
    links outside the subtree never leak space inside the branch.
    `FlattensBranch`, `space.TracksUsage` and `integrity.PgfsCheck`
    scope a subtree branch to the rows reachable from `trees.root_ino`.
    If the root directory of a live subtree mount (`pgfs mount
    main:/sub/path`) is deleted on another mount, operations on the
    mount fail with ESTALE.

    **Postcondition.** `..` at the view's root is the root itself. An
    inode outside the subtree cannot be reached.

    **Errors.** A path that is not a directory fails with ENOTDIR.

??? abstract "LimitsBranchDepth"
    `spec.branch.LimitsBranchDepth`

    A branch of a branch adds one layer to the chain that every
    lookup walks. Depth is capped at 8: creating a branch that would be
    deeper fails with EMLINK and suggests `pgfs tree flatten`. Creating
    a branch whose depth exceeds `--auto-flatten-depth` (default 4)
    logs a WARN, and any tree above `--auto-flatten-depth` is flattened
    in the background (`FlattensBranch`). If a background auto-flatten
    batch fails (for example on a lock timeout, serialization failure, or
    ENOSPC), housekeeping logs WARN and retries with backoff on the next
    cycle while foreground operations continue uninterrupted.

    **Bounds.** No lookup probes more than 9 layers.

??? abstract "FlattensBranch"
    `spec.branch.FlattensBranch`

    `pgfs tree flatten TREE` gives a branch its own copy of every
    metadata row it inherits, then detaches it from its base. It works
    whether or not the branch's layer has retained epochs
    (`RetainedEpochs`), and runs online while the tree is mounted and
    written, in bounded batches:

    1. Each batch copies up to 10 000 visible rows from
       `(base_layer, base_epoch)` reachable from `trees.root_ino` that
       have not yet been rebased into this layer (recomputing `nlink`
       from in-subtree dentries when `trees.root_ino ≠ 1`). Each
       inherited row is inserted with `born = 0` and `died` set to the
       earliest `born` of that key in this layer, or `∞` if the key was
       never touched in this layer (`ON CONFLICT DO NOTHING` so a
       concurrent mutation always wins). Inherited `file_blocks` rows at
       or above an `inherit_limit` lowered at epoch `e` get
       `born = 0, died = e`. Every snapshot or child branch of the layer
       at epoch `E ≥ 1` therefore sees through `born = 0` exactly what
       it previously saw through the base.
    2. When no rows remain to rebase, one transaction that holds the
       gate exclusively sets `base_layer` to null and `depth` to 0,
       resets `shadows_base = false` on all rows in the layer and
       `inherit_limit = ∞` on live inodes, and deletes whiteout rows
       (`dentries` with `ino IS NULL`, `inodes` with `nlink = 0` not in
       `orphans`, `file_blocks` with `blob_id IS NULL`, and `xattrs`
       with `value IS NULL`).

    Data is never copied. Every blob referenced by a copied map row is
    marked `shared` (`space.CollectsSharedBlobs`).

    **Postcondition.** Every live and historical view on the layer is
    unchanged at every step.

    **Crash.** A crash between batches leaves extra `born = 0` rows that
    are identical to what they shadow. A later run resumes.

    **Bounds.** Each batch is one bounded transaction (Oracle FSS r91).

??? abstract "PromotesBranch"
    `spec.branch.PromotesBranch`

    `pgfs promote [--force] BRANCH TREE` and
    `pgfs.promote(branch, tree, force default false)` make a branch the
    new contents of a tree, atomically. The promote succeeds only if all
    of these hold:

    - BRANCH was branched from TREE (or a snapshot of TREE), with
      `BRANCH.root_ino = TREE.root_ino`;
    - unless `--force` is given, BRANCH was branched directly from
      TREE's current layer (`TREE.layer = BRANCH.base_layer`) and TREE
      has had no change since the branch point
      (`TREE.epoch = BRANCH.base_epoch + 1`, and no row in `TREE.layer`
      has `born = TREE.epoch` or `died = TREE.epoch`);
    - neither TREE nor BRANCH has a read-write mount, BRANCH has no
      named snapshots of its own (`snapshots.tree = BRANCH`), and no
      other layer uses `BRANCH.layer` as its base.

    In one transaction holding both gates exclusively:

    - **Normal promote (without `--force`) folds `BRANCH`'s delta back
      into `TREE.layer`:**
      1. records TREE's previous contents as the snapshot
         `TREE@pre-promote-<UTC timestamp>` at `TREE.layer`'s current
         epoch `E`, and advances `TREE.layer.epoch` to `E + 1`;
      2. folds `BRANCH`'s rows into `TREE.layer` at `E + 1` (after the
         epoch advance, so the `pre-promote` snapshot at `E` keeps the
         old state): `TREE.layer` rows replaced or whited out by the
         branch get `died = E + 1`, and the branch's live rows are
         inserted with `born = E + 1`,
         `shadows_base = COALESCE(tree_row.shadows_base, branch_row.shadows_base)`,
         and `inherit_limit = LEAST(tree.inherit_limit, branch.inherit_limit)`
         (if `TREE.layer` has a base and `shadows_base = true`, a branch
         whiteout also leaves a live whiteout at `E + 1`);
      3. writes one net change row per touched inode into `changes` on
         `TREE.layer` under the promote transaction's `xid` and fresh
         `seq` values (`changefeed.ResumableCursors`);
      4. removes the name BRANCH and deletes `BRANCH.layer`, then prints
         the number of rows folded.
      `TREE` stays on `TREE.layer` at its existing depth, and
      `SnapshotDiff` across the promote keeps working.
    - **`--force` promote** records `TREE@pre-promote-<UTC timestamp>`,
      advances `TREE.layer`'s epoch by one, points TREE at
      `BRANCH.layer`, and removes the name BRANCH.

    This is how pgfs offers multi-file atomicity: prepare the change in
    a branch, then publish it with one compare-and-swap.

    **Postcondition.** A new mount of TREE shows exactly what BRANCH
    showed. Read-only live mounts on TREE pick up a normal fold-back
    within their cache TTL; after a `--force` layer move, open read-only
    mounts on the old layer keep showing its frozen state until they
    remount.

    **Durability.** Commits with `synchronous_commit = on` (verified via
    `pgfs.sync_barrier` on a `SyncRepWaitForLSN` warning,
    `durability.FsyncIsDurable`).

    **Errors.** ESTALE if TREE changed after the branch point (or moved
    to another layer) and `--force` was not given. EBUSY if TREE or
    BRANCH has a read-write mount, if BRANCH has named snapshots, or if
    a layer uses BRANCH as its base. EINVAL if BRANCH was not branched
    from TREE or has a different `root_ino`.

    **Bounds.** Normal promote is `O(|delta|)` in the rows changed in
    BRANCH, with no row limit (a large fold holds back cluster snapshot
    `xmin` for the duration of the transaction, delaying `VACUUM` and
    `changefeed.HorizonOrderedDelivery` until it commits); `--force`
    promote is constant time.

    **Sources.** Compare-and-swap publication of versioned state, as in
    Delos r73. Windows TxF is the counterexample: general filesystem
    transactions were deprecated for lack of use.

??? abstract "DeletesSnapshotsAndTrees"
    `spec.branch.DeletesSnapshotsAndTrees`

    `pgfs snapshot delete TREE@NAME` and `pgfs tree delete TREE`
    remove a name in constant time. Space comes back later through
    `space.ReclaimsRetiredRows`. A snapshot that is the base of a branch
    or has an active session in `mounts` cannot be deleted. Neither can
    a tree with any active session in `mounts` (read-write or
    read-only), or a tree whose layer is a base of another layer.

    **Errors.** EBUSY, with a message listing the branches or mounts
    that depend on the name.

??? abstract "RetainedEpochs"
    `spec.branch.RetainedEpochs`

    The retained epochs of a layer are the epochs of its named
    snapshots, the base epochs of the layers that use it as a base, and
    its current epoch. A retired row is still needed exactly when some
    retained epoch `E` satisfies `born ≤ E < died`. A row that no
    retained epoch can see is garbage. This definition is the contract
    between this module and `space.ReclaimsRetiredRows`.

    **Invariant.** The reclaimer never removes a row that a retained
    epoch can see. `verify.CrashConformance` checks this after crashes
    during reclaim.

??? abstract "SnapshotDiff"
    `spec.branch.SnapshotDiff`

    `pgfs diff TREE@A TREE@B` (or `TREE@A TREE` for the live tree)
    lists what changed between two views of one layer, computed from
    epochs rather than from the change feed: the rows born or retired
    between the two epochs. The output lists each path as added,
    removed, modified or renamed, and the files whose blocks changed. It
    works after the feed's retention has passed, across normal
    fold-back promotes (`PromotesBranch`), and for branches against
    their base. If a `--force` promote moved `TREE` to a different
    layer between the two views, `pgfs diff` refuses with an error that
    names the `--force` promote and the `TREE@pre-promote-<ts>` snapshot
    on the old layer.

    **Bounds.** Proportional to the number of rows that changed, not to
    the size of the tree. This uses indexes on `(layer, born)` and
    `(layer, died)`.

    **Sources.** AFS 05 (incremental clones), BetrFS r55.

---

<small>Generated from `spec.branch.BranchableTrees` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
