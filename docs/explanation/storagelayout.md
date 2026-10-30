# Explanation: Storage Layout

!!! success "Status: Stable"
    `pgfs mkfs` creates the versioned `pgfs` schema: inode and dentry rows keyed by persistent inode numbers with random generations, byte-exact names, inline small files, fixed-size immutable checksummed blobs behind a block map, epoch-versioned xattrs, every operation as a SQL function, and the read-only views.

> Every byte of a pgfs filesystem is a row in PostgreSQL: inodes, names, attributes and data blocks. Inode numbers are permanent, names are exact bytes, small files take one row, and file data is immutable, checksummed and shareable between files, snapshots and branches.

[Tutorial](../tutorials/storagelayout.md) · [How-to](../how-to/storagelayout.md) · [Reference](../reference/storagelayout.md) · **Explanation**

---

## Why inode numbers, not paths

The first pgfs schema keyed every row by its directory path
and name. That makes a lookup one query, but renaming a
directory has to rewrite the path of every descendant, which
costs time proportional to the subtree, and the
prefix match that finds descendants is easy to get wrong.
Almost every filesystem built on a database, from Inversion
in 1993 to HopsFS, Tectonic and Oracle's file storage
service, keys metadata by a permanent inode number instead
and stores names as `(parent, name)` rows. A rename then
changes one row, and an inode number can be handed to the
kernel and mean the same file forever.

## Why data is immutable

A block is never changed in place. Writing to it inserts a
new blob and moves one pointer. That costs nothing extra,
because Postgres rewrites a changed value in full anyway, and
it gives a lot back: a reader never sees half a block, a
snapshot can keep the old pointer, a branch or a copy can
share blobs, and a checksum computed once stays valid.

## Why names are bytes

Unix names are byte strings, not text. Storing them as
`text` subjects them to the database's encoding and
collation, so a name that is not valid UTF-8 cannot be
stored, and two names may compare equal when the kernel
thinks they differ. `bytea` matches the kernel exactly.

## Why small files live in the inode

Most files in source trees and home directories are small.
Putting their bytes in the inode row means that creating,
writing and reading one touches a single row, which is where
a database-backed filesystem wins or loses on metadata-heavy
work.

---

<small>Generated from `spec.storage.StorageLayout` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
