# Reference: Atomic Rename

!!! success "Status: Stable"
    A rename moves one dentry in constant time with no name matching (Bug 2), commits the source's buffered writes first (Bug 3), takes the rename lock for directory moves, and honours `RENAME_NOREPLACE` and `RENAME_EXCHANGE`. Covered by `tests/storage.rs`, the model-conformance suite, `test_rename.sh`, `test_posix.sh` and the git workload.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

[Tutorial](../tutorials/atomicrename.md) · [How-to](../how-to/atomicrename.md) · **Reference** · [Explanation](../explanation/atomicrename.md)

---

## Outcome matrix

| Source | Target | Result |
|---|---|---|
| file | absent | moved |
| file | file | target atomically replaced (deleted, or orphaned if open) |
| file | directory | EISDIR |
| directory | absent | moved, descendants untouched |
| directory | empty directory | target atomically replaced |
| directory | non-empty directory | ENOTEMPTY |
| directory | inside itself | EINVAL |
| either | the same inode | success, nothing changes |

## Flags

| Flag | Behaviour |
|---|---|
| `RENAME_NOREPLACE` | EEXIST if the target exists |
| `RENAME_EXCHANGE` | swap; ENOENT if either is missing |
| `RENAME_WHITEOUT`, unknown | EINVAL |


## Specification requirements

`AtomicRename` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "RenamesEntries"
    `spec.app.RenamesEntries`

    A FUSE `rename` calls `pgfs.rename`, which moves one dentry from
    `(old parent, old name)` to `(new parent, new name)`. It follows
    POSIX semantics:

    - If the target exists and is a non-directory, it is replaced
      atomically. Its `nlink` drops, and it is deleted or orphaned as in
      `UnlinksFiles`.
    - If the target is an empty directory, it is replaced atomically.
    - If the target is a non-empty directory, it replies ENOTEMPTY.
    - If the source is a non-directory and the target a directory, it
      replies EISDIR.
    - If the source is a directory and the target a non-directory, it
      replies ENOTDIR.
    - A directory keeps its inode and its descendants. Only its dentry
      row moves (preserving `shadows_base` on the old and new keys,
      `branch.EpochVersionedRows`), and the parents' `nlink`, mtime and
      ctime change. The renamed inode's ctime is set, and the change is
      recorded (`changefeed.RecordsChanges`).

    **Invariant.** Exactly one dentry changes, plus at most one replaced
    target. No other row is touched, and no name matching of any kind is
    involved. This is what makes Bug 2 impossible: the old rename
    matched descendants with `LIKE`, so `_` and `%` acted as wildcards.

    **Crash.** All or nothing: after a crash, the source name or the
    target name resolves to the inode, never both and never neither.

    **Concurrency.** Pending writes of the source and of a replaced
    target are committed first (`durability.FlushesBeforeNaming`,
    verified against the mount's dirty-inode set via
    `concurrency.InodeShardedWorkers`). That is Bug 3: rename lost
    buffered writes. A directory moving to a different parent takes the
    rename lock (`concurrency.RenameSerialisationPoint`). Rows lock in
    the order of `concurrency.LockOrder`.

    **Errors.** ENOENT if the source is missing. EXDEV across trees.
    EROFS on a snapshot. EBUSY for the root of a view.

    **Bounds.** Constant work, whatever the size of the subtree (Oracle
    FSS r91, InfiniFS r16).

??? abstract "RejectsInvalidRenames"
    `spec.app.RejectsInvalidRenames`

    `rename` validates before it changes anything:

    - moving a directory into itself or one of its descendants replies
      EINVAL, checked under the rename lock;
    - renaming a name onto a name of the same inode succeeds and does
      nothing, as POSIX requires;
    - `RENAME_NOREPLACE` and `RENAME_EXCHANGE` are supported
      (`posix.RenameFlags`);
    - `RENAME_WHITEOUT` and unknown flags reply EINVAL;
    - `.` and `..` as either name reply EINVAL (the kernel also checks).

??? abstract "SupportsGitInit"
    `spec.app.SupportsGitInit`

    Git works on a mount, as a regression workload for the POSIX
    details it depends on. On a mounted tree, `git init`, `git clone`
    of a pinned repository, `git status`, `git commit`, `git gc` and
    `git checkout` of another branch all succeed, and `git fsck` passes.
    This exercises:

    - rename-over for lockfiles;
    - `O_EXCL` creates of `*.lock` (`posix.ExclusiveCreate`);
    - `chmod +x` on hooks (`posix.SetsAttributes`);
    - `fsync` of objects and refs under `core.fsync`
      (`durability.FsyncIsDurable`);
    - hard links and open-after-unlink during `gc`.

    The same workload is a benchmark (`perf.BenchmarkSuite`).

    **Sources.** Bento r53 (git through FUSE as the yardstick), ALICE
    r118.

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
