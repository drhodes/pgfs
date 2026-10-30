# Reference: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and `readdirplus` keep link counts and directory times, page by name with keyset cookies, and check emptiness in the same transaction as the delete. Covered by `tests/storage.rs`, the model-conformance suite and `test_dirs.sh`.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

[Tutorial](../tutorials/hierarchicaldirectories.md) · [How-to](../how-to/hierarchicaldirectories.md) · **Reference** · [Explanation](../explanation/hierarchicaldirectories.md)

---

## FUSE operations

| Operation | SQL | Errors |
|---|---|---|
| `mkdir` | `pgfs.mkdir` | EEXIST, ENOENT, ENOTDIR, ENAMETOOLONG, EINVAL, ENOSPC, EROFS |
| `rmdir` | `pgfs.rmdir` | ENOTEMPTY, ENOENT, ENOTDIR, EBUSY (root of the view), EROFS |
| `readdir`, `readdirplus` | `pgfs.readdir`, `pgfs.readdirplus`, 256 names per page | ENOTDIR |

## Attributes

| Field | Value |
|---|---|
| `nlink` | 2 + subdirectories |
| `mtime`, `ctime` | updated by every create, remove and rename in the directory |


## Specification requirements

`HierarchicalDirectories` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "CreatesDirectories"
    `spec.app.CreatesDirectories`

    A FUSE `mkdir` calls `pgfs.mkdir`, which in one transaction
    following `concurrency.LockOrder`:

    - allocates the new `ino` and a 63-bit random `gen`
      (`storage.PersistentInodes`), locks the parent directory's inode
      row, increments the parent's `nlink`, and updates the parent's
      mtime and ctime;
    - inserts a directory inode (`shadows_base = false`), with the
      requested mode after umask and the caller's owner (set-group-id
      inherited, `security.OwnershipFromCaller`);
    - inserts its dentry, with `ON CONFLICT DO NOTHING` (or replaces an
      existing whiteout with `shadows_base = true`,
      `branch.EpochVersionedRows`);
    - records the change.

    **Postcondition.** The name resolves to an empty directory with
    `nlink = 2`, and the parent's `nlink` grew by one (`posix.LinkCounts`).

    **Crash.** All or nothing.

    **Errors.** EEXIST, ENOENT, ENOTDIR, ENAMETOOLONG, EINVAL, ENOSPC,
    EROFS, as for `CreatesFiles`.

??? abstract "RemovesDirectories"
    `spec.app.RemovesDirectories`

    A FUSE `rmdir` calls `pgfs.rmdir`. In one transaction following
    the discover-sort-lock-revalidate protocol of
    `concurrency.LockOrder`, it discovers the directory's `ino`, locks
    `parent_ino` and the directory's `ino` in ascending `ino` order,
    checks that no live dentry names the directory as its parent, and
    then:

    - decrements the parent's `nlink` and updates the parent's mtime
      and ctime;
    - deletes or retires any child dentry whiteouts under the directory
      in this layer so no live child row remains under a whited-out
      directory (`storage.DentryTable`);
    - deletes the directory's inode (or turns it into an inode whiteout
      when `shadows_base = true`, `branch.CopiesUpOnWrite`);
    - removes the directory's dentry, or writes a whiteout when
      `shadows_base = true`;
    - records the change.

    On a branch, the emptiness check sees the merged view, so a
    directory whose inherited entries are all whited out is empty.

    **Concurrency.** A create in the directory locks the same inode row
    (`concurrency.LockOrder`), so no create can land between the check
    and the delete.

    **Crash.** All or nothing.

    **Errors.** ENOTEMPTY if any entry remains. ENOENT if absent.
    ENOTDIR if not a directory. EBUSY for the root of a view. EROFS on a
    snapshot.

??? abstract "ListsDirectories"
    `spec.app.ListsDirectories`

    A FUSE `readdir` or `readdirplus` returns `.` and `..` and then
    the directory's entries in byte order of name. Each entry has its
    inode number and type, and readdirplus also returns attributes. The
    offset of each entry is a cookie that the directory handle maps to
    the entry's name (a 64-bit offset cannot hold a 255-byte name). The
    next call resumes with `name > cookie`, using keyset pagination over
    the `(layer, parent_ino, name)` index. On a branch, the merged view of
    `branch.LayeredLookup` is paginated the same way.

    **Postcondition.** Every entry that exists for the whole listing is
    returned exactly once. An entry created or removed during the
    listing may or may not appear, never twice.

    **Bounds.** Each call costs one indexed range scan of at most the
    number of entries that fit the kernel's buffer. Listing a directory
    of `n` entries is O(n) in total.

    **Sources.** ScaleCheck r57, HMFS r131, TableFS r07. The first
    implementation re-ran the full listing on every call and indexed
    into it, which is O(n²) and skips or repeats entries under
    concurrent change.

??? abstract "LooksUpEntries"
    `spec.app.LooksUpEntries`

    A FUSE `lookup` of `(parent ino, name)` returns the child's
    persistent inode number (`ino`), generation (`inodes.gen`,
    `storage.PersistentInodes`) and attributes, and increments the
    kernel lookup count that `forget` later decrements
    (`storage.ForgetsInodes`). A miss replies ENOENT with an entry TTL,
    so the kernel caches the negative result
    (`consistency.KernelCacheContract`).

    **Postcondition.** The same name resolves to the same `(ino, gen)`
    across lookups, restarts and remounts, until it is unlinked or
    renamed.

    **Bounds.** One index probe per layer of the view, in one round
    trip, or none on a daemon cache hit.

??? abstract "ReportsAttributes"
    `spec.app.ReportsAttributes`

    A FUSE `getattr` returns kind, mode, nlink, uid, gid, rdev, size,
    `blocks` (512-byte units of stored data, holes excluded), `blksize`
    (the block size), and atime, mtime, ctime and birth time in
    nanoseconds. Size and mtime include writes still buffered on the
    mount (`consistency.DrainsBuffersBeforeExternalising`). The reply's
    TTL follows `consistency.KernelCacheContract`. Inode 1 is the root
    directory of the mounted view.

    **Postcondition.** The values reflect every operation that replied
    before the request.

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
