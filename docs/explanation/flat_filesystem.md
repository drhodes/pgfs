# Explanation: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete run on the inode-keyed schema: persistent inode numbers with generations, buffered writes visible to every handle on the mount, read-modify-write of partial blocks (Bug 1), single-transaction unlink and checksummed immutable blocks. Covered by `tests/storage.rs`, the model-conformance suite and `scripts/test_*.sh`.

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

[Tutorial](../tutorials/flat_filesystem.md) · [How-to](../how-to/flat_filesystem.md) · [Reference](../reference/flat_filesystem.md) · **Explanation**

---

## A file is an inode row

Every file has a permanent inode number and a random generation. The
kernel holds `(ino, gen)`, and every operation checks it, so a number
reused after a database restore is caught as ESTALE rather than
silently reading another file.

## Why writes are buffered

Each database round trip costs far more than copying a few kilobytes.
Writes collect in a per-inode buffer and commit as whole blocks at
close, fsync or under memory pressure. A partly written block is read
and overlaid before it is stored, so untouched bytes are never zeroed
(Bug 1), and every read on the mount sees the buffered bytes.

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
