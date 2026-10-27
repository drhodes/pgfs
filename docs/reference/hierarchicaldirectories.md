# Reference: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and nested paths of any depth are implemented and covered by `test_dirs.sh`. Directory timestamps are not tracked.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

[Tutorial](../tutorials/hierarchicaldirectories.md) · [How-to](../how-to/hierarchicaldirectories.md) · **Reference** · [Explanation](../explanation/hierarchicaldirectories.md)

---

## FUSE operations

| Operation | Behaviour | Errors |
|---|---|---|
| `mkdir` | checks the primary for an existing entry, then inserts a `dir` row | `EEXIST`, `ENOENT` (parent unknown), `EIO` |
| `rmdir` | checks the primary that the entry exists, then that no row has it as `parent`, then deletes it | `ENOENT`, `ENOTEMPTY`, `EIO` |
| `readdir` | returns `.`, `..`, then every child ordered by name | `ENOENT`, `EIO` |
| `lookup` | resolves `(parent inode, name)` to attributes | `ENOENT` |

The emptiness check and the delete in `rmdir` are separate
statements, not one transaction.

## `readdir` offsets

Entries are numbered from 1 in the order returned: `.` is 1,
`..` is 2, and children follow sorted by name. The kernel
resumes a partial listing by passing back the last number it
received. Offsets index the current list, so if the directory
changes between two calls of one listing, entries can be
skipped or repeated.

## Directory attributes

| Attribute | Value |
|---|---|
| `perm` | `0755` |
| `nlink` | 2, regardless of the number of subdirectories |
| `size`, `blocks` | 0 |
| `uid`, `gid` | the daemon's own uid and gid |
| `atime`, `mtime`, `ctime`, `crtime` | the Unix epoch (shown as 1970-01-01, or 1969-12-31 west of UTC) |

## Root directory

| Property | Value |
|---|---|
| Inode | 1 |
| Path | `''` (empty string) |
| Row | `parent = ''`, `name = ''`, `kind = 'dir'`, inserted at schema creation |
| Attributes | always synthesised; never read from the database |
| Listing | excludes the root's own row |

## Paths and names

| Rule | Detail |
|---|---|
| Key | `(parent, name)`, unique |
| Full path | `parent || '/' || name`, or `name` when `parent = ''` |
| Valid names | any UTF-8 string without `/` or NUL; non-UTF-8 names are rejected with `ENOENT` |
| Depth | not limited by pgfs. The unique index on `(parent, name)` limits one key to about 2.7 kB, so a longer combined path fails with `EIO`. |
| Index used by listings | `entries(parent)` |


## Specification requirements

`HierarchicalDirectories` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "CreatesDirectories"
    `spec.app.CreatesDirectories`

    A FUSE `mkdir` inserts a row with kind='dir'. If the entry exists
    the kernel receives EEXIST. The reply includes a directory inode
    with 0755 permissions and nlink=2.

??? abstract "RemovesDirectories"
    `spec.app.RemovesDirectories`

    A FUSE `rmdir` deletes the directory row only if the directory is
    empty (no child rows with matching parent path). If children exist
    the kernel receives ENOTEMPTY. If the entry does not exist the
    kernel receives ENOENT.

??? abstract "ListsDirectories"
    `spec.app.ListsDirectories`

    A FUSE `readdir` returns '.' and '..' followed by every child of
    the directory inode, each with the correct inode number and file
    type. Offsets are synthetic (1-based index); the reply stops when
    the kernel's buffer is full and resumes on the next call.

??? abstract "LooksUpEntries"
    `spec.app.LooksUpEntries`

    A FUSE `lookup` on a (parent inode, name) pair returns the
    entry's inode and attributes, or ENOENT if absent. Lookup allocates
    a stable inode number for the entry's full path.

??? abstract "ReportsAttributes"
    `spec.app.ReportsAttributes`

    A FUSE `getattr` returns size, kind, permissions, timestamps,
    and nlink for any inode. The root inode (ino=1) is always a
    directory.

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
