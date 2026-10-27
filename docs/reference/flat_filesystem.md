# Reference: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete are implemented and covered by unit and integration tests. Permissions, ownership and timestamps are reported but not settable (see Reference).

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

[Tutorial](../tutorials/flat_filesystem.md) · [How-to](../how-to/flat_filesystem.md) · **Reference** · [Explanation](../explanation/flat_filesystem.md)

---

## FUSE operations

| Operation | Triggered by | Behaviour | Errors |
|---|---|---|---|
| `lookup` | path resolution | reads the entry's attributes; assigns an inode number on first sight | `ENOENT` (absent, or name not valid UTF-8) |
| `getattr` | `stat`, `ls -l` | reads attributes; the root (inode 1) is synthesised | `ENOENT` |
| `create` | `open(O_CREAT)` | checks the primary for an existing entry, inserts a `file` row with size 0, opens a write handle | `EEXIST` |
| `open` | `open` | allocates a handle with an empty dirty-block buffer; flags and permissions are not checked | `ENOENT` |
| `read` | `read`, `pread` | flushes the handle's pending writes, then fetches only the blocks covering the range; the range is clamped to the file size; holes read as zeros | `ENOENT` |
| `write` | `write`, `pwrite` | with an open handle, buffers the bytes in memory and returns immediately; without one, writes the touched blocks in one transaction; extends the size if the write ends past it | `ENOENT` |
| `flush`, `fsync`, `release` | `close`, `fsync` | writes the handle's dirty blocks and new size in one transaction | `EIO` |
| `setattr` | `truncate`, `ftruncate`, `open(O_TRUNC)` | size changes truncate or extend in one transaction; mode, owner and time changes are accepted and ignored | `EISDIR` (size change on a directory), `ENOENT` |
| `unlink` | `rm` | deletes the entry row, then its block rows; forgets the inode mapping | `EIO` |

## Storage schema

```sql
CREATE TABLE entries (
    ino        bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    parent_ino bigint NOT NULL DEFAULT 1,
    parent     text   NOT NULL DEFAULT '',
    name       text   NOT NULL,
    kind       text   NOT NULL DEFAULT 'file',   -- 'file' | 'dir'
    data       bytea  NOT NULL DEFAULT '',       -- legacy, always empty
    size       bigint NOT NULL DEFAULT 0,
    mode       integer NOT NULL DEFAULT 33188,
    uid        integer NOT NULL DEFAULT 1000,
    gid        integer NOT NULL DEFAULT 1000,
    atime      timestamptz NOT NULL DEFAULT now(),
    mtime      timestamptz NOT NULL DEFAULT now(),
    ctime      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT unique_parent_name UNIQUE (parent, name)
);

CREATE TABLE blocks (
    ino      bigint  NOT NULL DEFAULT 1,
    parent   text    NOT NULL DEFAULT '',
    name     text    NOT NULL DEFAULT '',
    block_no integer NOT NULL,
    data     bytea   NOT NULL,
    PRIMARY KEY (parent, name, block_no)
);
```

Indexes: `entries(parent_ino)`, `entries(parent)`,
`blocks(ino, block_no)`. The schema is created on first
connect with `CREATE TABLE IF NOT EXISTS`, inside one
transaction that also inserts the root row
(`ino = 1, parent = '', name = ''`, `kind = 'dir'`).

| Property | Value |
|---|---|
| Block size | 65 536 bytes (`BLOCK_SIZE`) |
| Block number | `offset / 65536` |
| Last block | may be shorter than 65 536 bytes |
| Holes | blocks with no row; read as zeros |
| Source of truth for size | `entries.size` |
| `entries.ino`, `parent_ino`, `mode`, `uid`, `gid`, `atime`, `ctime` | written with defaults; not read by the FUSE path |

## Reported attributes

| Attribute | Regular file |
|---|---|
| `perm` | `0644` |
| `nlink` | 1 |
| `uid`, `gid` | the daemon's own uid and gid |
| `size` | `entries.size` |
| `blocks` | `ceil(size / 512)` |
| `blksize` | 4096 |
| `atime`, `mtime`, `ctime`, `crtime` | all equal to `entries.mtime` |

Attribute and entry replies carry the TTL set by `--attr-ttl`
(default 1 s).

## Inode numbers

Inode numbers are assigned in memory, per mount, the first time
the kernel sees a path. They start at 2, increase by one, are
never reused within a mount, and survive renames. They are not
persisted: a remount assigns new numbers. They are unrelated to
`entries.ino`.

## Durability

Every connection runs `SET synchronous_commit = off`. A
transaction is acknowledged once it is in PostgreSQL's WAL
buffers, before the WAL is flushed to disk. `fsync` on a pgfs
file waits for the commit, not for the disk flush. If
PostgreSQL crashes, the most recently acknowledged commits can
be lost; with default settings that window is under one second.
Committed data is never partially applied. Buffered writes that
have not reached `flush`, `fsync` or `close` are lost if the
daemon dies.

## Error mapping

| errno | Meaning |
|---|---|
| `ENOENT` | the entry, or the inode's path, does not exist |
| `EEXIST` | `O_CREAT|O_EXCL` on an existing name |
| `EISDIR` | size change requested on a directory |
| `EIO` | unexpected failure (database error, invariant violation); the full error chain is logged at ERROR |

## Not supported

| Operation | Result |
|---|---|
| symbolic links (`symlink`) | `EPERM` from `ln -s` (operation not implemented) |
| hard links (`link`) | `EPERM` from `ln` (operation not implemented) |
| FIFOs, sockets, device nodes (`mknod`) | `ENOSYS` |
| extended attributes | not implemented |
| `chmod`, `chown`, `utimes`, `touch -d` | succeed but have no effect |
| `statfs` (`df`) | reports zero size and zero free space |
| `fallocate` | `EOPNOTSUPP` (see [Kernel Offloading](kerneloffloadingacceleration.md)) |
| reflink clone (`cp --reflink=always`) | `EOPNOTSUPP` |


## Specification requirements

`FlatFilesystem` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "ConnectsToPostgres"
    `spec.app.ConnectsToPostgres`

    The daemon connects to a Postgres database via a libpq connection
    string. On first connect it creates the `entries` table (ino, parent_ino,
    parent, name, kind, size, mode, uid, gid, atime, mtime, ctime) and
    `blocks` table (ino, parent, name, block_no, data) if they do not
    already exist. Connection failure produces a full error story
    including the connection string and the root cause.

??? abstract "CreatesFiles"
    `spec.app.CreatesFiles`

    A FUSE `create` (O_CREAT) inserts a row with kind='file', empty
    data, size=0, and the current timestamp. If the entry already exists
    the kernel receives EEXIST. The reply includes a valid inode and
    file attributes with 0644 permissions.

??? abstract "ReadsFiles"
    `spec.app.ReadsFiles`

    A FUSE `read` on a regular-file inode returns the byte range
    from the file's stored block chunks. Offset and size are clamped to
    the stored file length. Missing inodes reply ENOENT.

??? abstract "WritesFiles"
    `spec.app.WritesFiles`

    A FUSE `write` updates touched block chunks or buffers dirty blocks
    per open file handle, persisting them to PostgreSQL. The write extends
    the file if the offset+length exceeds the current size.

??? abstract "TruncatesFiles"
    `spec.app.TruncatesFiles`

    A FUSE `setattr` with a size parameter truncates or extends a
    regular file to the given size (zero-filled extension, deleting
    trailing block chunks in PostgreSQL). Size changes on directories
    reply EISDIR.

??? abstract "UnlinksFiles"
    `spec.app.UnlinksFiles`

    A FUSE `unlink` deletes the row and removes the inode from the
    in-memory path map.

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

??? abstract "DistinguishesExpectedErrors"
    `spec.app.DistinguishesExpectedErrors`

    Expected conditions (ENOENT, EEXIST, ENOTEMPTY, EISDIR, ENOTDIR,
    EINVAL) are replied directly to the kernel without logging.
    Unexpected failures (database errors, invariant violations) are
    logged in full via `log_and_reply!` with the complete error chain
    and replied as EIO.

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
