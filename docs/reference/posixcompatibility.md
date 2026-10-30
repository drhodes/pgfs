# Reference: POSIX Compatibility

!!! success "Status: Stable"
    Modes, owners and nanosecond times stick, set-id bits are cleared on chown and non-owner writes, hard links, symlinks, special files, xattrs, open-after-unlink, renameat2 flags, exclusive create, sparse files with SEEK_DATA and SEEK_HOLE and the unsupported operations' errnos are built and tested (`tests/storage.rs`, `test_posix.sh`, the git workload). pjdfstest and xfstests runs belong to `verify.PosixSuites`.

> pgfs behaves like a local Linux filesystem for the operations that real tools use: permissions, ownership and times that stick, hard links, symlinks, xattrs, files that stay open after deletion, atomic rename variants and sparse files. What it does not support, it refuses with a stated error.

[Tutorial](../tutorials/posixcompatibility.md) · [How-to](../how-to/posixcompatibility.md) · **Reference** · [Explanation](../explanation/posixcompatibility.md)

---

## Supported operations

| Area | Operations |
|---|---|
| names | lookup, create (`O_EXCL`), mkdir, mknod, symlink, link, unlink, rmdir, rename (`RENAME_NOREPLACE`, `RENAME_EXCHANGE`) |
| attributes | getattr, setattr (mode, uid, gid, size, atime, mtime, `UTIME_NOW`, `UTIME_OMIT`) |
| data | open, read, write (`O_APPEND`, `O_SYNC`, `O_DSYNC`, `O_DIRECT`), flush, release, fsync, lseek (`SEEK_DATA`, `SEEK_HOLE`) |
| directories | opendir, readdir, readdirplus, releasedir, fsyncdir |
| space | fallocate (`KEEP_SIZE`, `PUNCH_HOLE`, `ZERO_RANGE`), copy_file_range, statfs |
| xattrs | getxattr, setxattr, listxattr, removexattr (`user.`, `trusted.`, `security.`) |

## Unsupported operations

| Operation | Reply |
|---|---|
| POSIX ACLs | EOPNOTSUPP |
| `O_TMPFILE` | EOPNOTSUPP |
| `RENAME_WHITEOUT` | EINVAL |
| collapse and insert range | EOPNOTSUPP |
| `FICLONE` (`cp --reflink=always`) | EOPNOTSUPP |
| ioctl | ENOTTY |

## Limits

| Limit | Value |
|---|---|
| name | 255 bytes |
| symlink target | 4 095 bytes |
| file size | 2^63 − 1 bytes |
| hard links per inode | 65 000 |
| xattr name / value | 255 / 65 536 bytes |
| timestamps | nanoseconds; atime not updated on read |


## Specification requirements

`PosixCompatibility` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "SetsAttributes"
    `spec.posix.SetsAttributes`

    `setattr` applies every attribute it is given, in one transaction:
    mode, uid, gid, size (`TruncatesFiles`), atime and mtime. It honours
    `UTIME_NOW` and `UTIME_OMIT`, and sets ctime to now whenever anything
    changes. Permission checks are the kernel's (`security.EnforcesPermissions`).
    A chown clears the set-user-id and set-group-id bits of a regular
    file, as Linux does.

    **Postcondition.** A following `getattr` returns exactly the values
    set. No field the call names is ignored.

    **Errors.** EROFS on a snapshot. EPERM only if the kernel let through
    a change pgfs cannot store, which should never happen.

    The current daemon accepts mode, uid, gid and times and replies
    success without storing them. That is the behaviour this
    requirement exists to forbid.

??? abstract "HardLinks"
    `spec.posix.HardLinks`

    `link` adds a dentry for an existing inode and increments its
    `nlink`, in one transaction. `unlink` removes one dentry and
    decrements `nlink`; the inode and its data go only when `nlink`
    reaches 0 and no handle is open (`OpenAfterUnlink`). Hard links to
    directories reply EPERM. Links across trees reply EXDEV.

    **Invariant.** `nlink` of a non-directory equals the number of live
    dentries that point to it in the view (`integrity.PgfsCheck`).

    **Bounds.** At most 65 000 links per inode; more replies EMLINK.

??? abstract "SymbolicLinks"
    `spec.posix.SymbolicLinks`

    `symlink` creates an inode of kind `l` whose `target` holds the
    link text as bytes, up to 4 095 bytes (ENAMETOOLONG beyond). The
    `readlink` callback returns it unchanged. The kernel resolves symlinks;
    pgfs never follows one. Symlink mode is reported as 0777.

??? abstract "SpecialFiles"
    `spec.posix.SpecialFiles`

    `mknod` creates regular files, FIFOs, sockets, and character and
    block devices, storing `rdev` for devices. pgfs stores these inodes
    and the kernel implements their I/O. Device nodes take effect only on
    mounts given `--dev` (`security.SetIdAndDevicesOptIn`), and the kernel
    permits creating them only to privileged callers.

??? abstract "ExtendedAttributes"
    `spec.posix.ExtendedAttributes`

    `getxattr`, `setxattr`, `listxattr` and `removexattr` store
    attributes in `storage.ExtendedAttributeTable`, in the `user.`,
    `trusted.` and `security.` namespaces. `XATTR_CREATE` and
    `XATTR_REPLACE` are honoured atomically. A size-0 `getxattr` or
    `listxattr` returns the required size.

    **Errors.** ENODATA for a missing attribute. EEXIST and ENODATA for
    the create and replace flags. ERANGE when the buffer is too small.
    EOPNOTSUPP for `system.posix_acl_access`,
    `system.posix_acl_default` and unknown namespaces. E2BIG past the
    size limits.

    **Sources.** ExtFUSE r13 (every write asks for `security.capability`;
    answer it cheaply, from the inode cache).

??? abstract "OpenAfterUnlink"
    `spec.posix.OpenAfterUnlink`

    Unlinking the last name of a file that is still open removes the
    name but keeps the inode. In the same transaction the inode is
    recorded in `orphans(layer, ino, since)`. Open handles keep reading
    and writing it. When the last handle is released, one transaction
    removes the `orphans` row and deletes the inode's block map, xattrs
    and inode row — or, when the inode's row in the layer has
    `shadows_base = true`, turns the `inodes` row into an inode whiteout
    (`nlink = 0, size = 0, inherit_limit = 0, inline_data = NULL,
    shadows_base = true`, `branch.EpochVersionedRows`) so the inherited
    inode cannot reappear. If the daemon crashes first, the orphan row
    survives, and the next mount of the tree reclaims it
    (`space.ReclaimsOrphans`).

    **Postcondition.** A program that opens, unlinks and keeps using a
    temporary file sees exactly what it would on ext4.

    **Crash.** An orphan is never visible by name after a crash, and its
    space is reclaimed by the next mount.

    **Sources.** DaisyNFS r68 (split delete). Git, editors and package
    managers rely on this behaviour.

??? abstract "RenameFlags"
    `spec.posix.RenameFlags`

    `rename` honours the renameat2 flags:

    - `RENAME_NOREPLACE`: fail with EEXIST if the target exists, decided
      by the unique index in the same statement;
    - `RENAME_EXCHANGE`: swap two names atomically. Both must exist,
      else ENOENT. They may be of different kinds;
    - `RENAME_WHITEOUT`: EINVAL.

    **Postcondition.** With `RENAME_EXCHANGE`, both names resolve to
    each other's inodes, and nothing else changes except the ctimes and
    the parents' mtimes.

??? abstract "ExclusiveCreate"
    `spec.posix.ExclusiveCreate`

    `create` with `O_CREAT | O_EXCL` fails with EEXIST if the name
    exists. The unique index decides, inside the inserting statement.
    Lockfiles (git's `index.lock`, `.lck` files) depend on exactly one
    of two concurrent exclusive creates succeeding.

    **Concurrency.** Exactly one of any set of concurrent exclusive
    creates of a name succeeds, whatever the interleaving.

    **Sources.** Lockify r41.

??? abstract "AppendWrites"
    `spec.posix.AppendWrites`

    A write on an `O_APPEND` handle lands at the end of the file as
    it is when the write is applied, including pending writes of other
    handles on the same mount. The inode's worker applies appends in
    arrival order (`concurrency.InodeShardedWorkers`).

    **Postcondition.** Concurrent appends from several handles never
    overwrite one another; the file grows by the sum of their lengths.

??? abstract "SparseFiles"
    `spec.posix.SparseFiles`

    Ranges never written are holes: they read as zero and store
    nothing. Extending a file with `truncate`, or writing past its end,
    creates holes rather than zeros. `lseek` with `SEEK_DATA` and
    `SEEK_HOLE` reports holes at block granularity: a block with no map
    row, or a hole row, is a hole. `st_blocks` counts only stored
    blocks, so `du` reports real usage.

    **Postcondition.** For every byte offset, the byte read is the last
    byte written there, or zero if none was.

??? abstract "Timestamps"
    `spec.posix.Timestamps`

    pgfs stores atime, mtime, ctime and birth time with nanosecond
    resolution and maintains them as Linux does:

    - writing or truncating a file sets its mtime and ctime;
    - any attribute change, link, unlink or rename of a file sets its
      ctime;
    - creating, removing or renaming an entry sets the mtime and ctime
      of the directories involved;
    - atime changes only through `setattr`. Reads never update it, as if
      every mount had `noatime`, so that a read is never a write.

    Buffered writes stamp mtime when the write is accepted, not when it
    is committed.

    **Concurrency.** Directory times are updated in the transaction that
    changes the entry. With many concurrent creates in one directory,
    this row is contended. If measurement shows it matters, directory
    times move to appended delta rows (Mantle r127, SingularFS r36).

??? abstract "LinkCounts"
    `spec.posix.LinkCounts`

    A directory's `nlink` is 2 plus its number of subdirectories. A
    non-directory's `nlink` is its number of names. mkdir and rmdir
    update the parent's count in the same transaction that changes the
    entry, and rename does the same when it moves a directory.

    **Invariant.** Checked by `pgfs check` for every inode.

??? abstract "FileSizeLimit"
    `spec.posix.FileSizeLimit`

    The largest file is 2^63 − 1 bytes. A write or truncate past it
    replies EFBIG. statfs reports a maximum name length of 255.

??? abstract "KernelLocalLocks"
    `spec.posix.KernelLocalLocks`

    pgfs does not implement `getlk`, `setlk` or `flock`. The kernel
    handles them, which is exact for a tree with one read-write mount.
    Locks are not visible to read-only mounts or to other machines. The
    multi-machine version must replace this.

??? abstract "WritebackCacheDisabled"
    `spec.posix.WritebackCacheDisabled`

    The daemon does not negotiate `FUSE_WRITEBACK_CACHE`. It will be
    enabled only when these preconditions are specified and covered by
    crash tests (crash consistency of write-back caches, r77):

    - the kernel owns mtime and size while the cache is on;
    - writes can arrive after `release`;
    - `O_APPEND` writes must be positioned by the kernel;
    - `fsync` must cover pages still in the kernel.

    **Sources.** To FUSE or Not r12 (the largest single win on the data
    path, hence worth doing properly).

??? abstract "DeclaresUnsupportedOps"
    `spec.posix.DeclaresUnsupportedOps`

    Each operation pgfs does not support replies a fixed errno, and
    the reference documentation lists them:

    | Operation | Reply |
    |---|---|
    | `system.posix_acl_*` xattrs | EOPNOTSUPP |
    | `O_TMPFILE` | EOPNOTSUPP |
    | `RENAME_WHITEOUT` | EINVAL |
    | `FALLOC_FL_COLLAPSE_RANGE`, `FALLOC_FL_INSERT_RANGE` | EOPNOTSUPP |
    | `FICLONE`, `FICLONERANGE` (`cp --reflink=always`) | EOPNOTSUPP; FUSE cannot pass them through |
    | `ioctl` of any kind | ENOTTY |
    | `bmap` | ENOSYS |
    | `getlk`, `setlk` | handled by the kernel (`KernelLocalLocks`) |

    **Sources.** Metis r48 (silent divergence is the dangerous case),
    ExtFUSE r13.

??? abstract "ErrnoCatalogue"
    `spec.posix.ErrnoCatalogue`

    Every errno pgfs can reply is listed, with its cause:

    | errno | Cause |
    |---|---|
    | ENOENT | the name or inode does not exist |
    | EEXIST | the name exists (create, mkdir, `O_EXCL`, `RENAME_NOREPLACE`, `XATTR_CREATE`) |
    | ENOTEMPTY | rmdir, or rename over a non-empty directory |
    | EISDIR, ENOTDIR | wrong kind for the operation |
    | EINVAL | invalid name or flags; rename into its own subtree |
    | ENAMETOOLONG | a name over 255 bytes or a symlink target over 4 095 bytes |
    | EXDEV | link or rename across trees |
    | EMLINK | too many links or too deep a branch |
    | EFBIG | past the maximum file size |
    | ENOSPC | capacity reached, or Postgres `53100 disk_full` |
    | EDQUOT | per-tree byte or inode quota reached (`space.TreeQuotas`) |
    | EROFS | a write to a snapshot or read-only mount |
    | EPERM, EACCES | a hard link to a directory; permission denied (`security.EnforcesPermissions`) |
    | EBUSY | root of a view in `rmdir` or `rename`; tree, branch or snapshot in use during mount, promote or delete |
    | ESTALE | promote when the tree changed since the branch point; subtree mount whose root directory was deleted; inode generation (`inodes.gen`) mismatch or deleted inode on an open handle (`storage.PersistentInodes`) |
    | EBADF | write or `copy_file_range` on a handle not open for the operation |
    | ENODATA, ERANGE, E2BIG | xattr cases |
    | EOPNOTSUPP, ENOTTY, ENOSYS | unsupported operations (`DeclaresUnsupportedOps`) |
    | EINTR | the request was interrupted (`failure.HonoursInterrupt`) |
    | ETIMEDOUT | a statement exceeded its timeout (`failure.BoundsEveryWait`) |
    | EIO | the database is unreachable, the mount is fenced, a checksum failed, or an invariant was violated |

    Every EIO is logged with its full error chain. No other errno is
    logged above DEBUG.

---

<small>Generated from `spec.posix.PosixCompatibility` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
