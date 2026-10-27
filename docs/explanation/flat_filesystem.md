# Explanation: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete are implemented and covered by unit and integration tests. Permissions, ownership and timestamps are reported but not settable (see Reference).

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

[Tutorial](../tutorials/flat_filesystem.md) · [How-to](../how-to/flat_filesystem.md) · [Reference](../reference/flat_filesystem.md) · **Explanation**

---

## A file is a handful of rows

pgfs stores each file as one row in `entries`, which holds its
name, kind, size and modification time, plus zero or more rows
in `blocks`, each holding up to 64 KiB of its contents. Both
are keyed by the file's location: the path of the directory
that contains it (`parent`) and its own `name`. A file at
`docs/sub/notes.txt` is the row `('docs/sub', 'notes.txt')`.

Path-based keys make the tables readable by a person with
`psql`. Ordinary SQL answers
questions like "which files under `docs/` changed today". The
cost of path keys appears when a directory is renamed, which is
discussed in [Atomic Rename](atomicrename.md).

## Why blocks

The first version of pgfs stored a whole file in one `bytea`
column; `entries.data` is the remnant. Every write then meant
reading the whole file, changing a few bytes, and writing the
whole file back. Appending a line to a 100 MB log rewrote
100 MB.

Fixed-size blocks bound that work. A write touches only the
blocks it overlaps, and a read fetches only the blocks it
covers. The block size is a trade-off. Smaller blocks mean less
rewriting for small writes but more rows and more per-row
overhead for large files. 64 KiB keeps the row count low while
keeping a partial-block rewrite cheap. PostgreSQL compresses
and moves values of that size out of line through TOAST on its
own.

Blocks that were never written simply do not exist, so sparse
files are free. A read fills the gaps with zeros, which is
exactly what POSIX requires of a hole.

## Why writes are buffered per open file

The kernel delivers a large `write` to FUSE in pieces. Turning
each piece into its own PostgreSQL transaction would make the
commit round-trip, not the data, the bottleneck. pgfs instead
collects writes in memory, per open file, as a set of dirty
blocks, and commits them all in one transaction when the file
is flushed, fsynced or closed.

This is close-to-open consistency, the model NFS uses. A writer
and anything that opens the file after the writer closes it see
the same bytes. Readers using the same mount see the data even
sooner, because a read first flushes that file's pending
writes.

## Durability is a deliberate trade

pgfs turns off PostgreSQL's `synchronous_commit`. Commits
return without waiting for the WAL to reach disk, which removes
the largest fixed cost from every metadata operation. In
exchange, a PostgreSQL crash can lose the last fraction of a
second of acknowledged work. This is similar to how a local
filesystem's page cache behaves without `fsync`, except that
here even `fsync` does not close the gap. Transactions remain
atomic: a lost commit disappears entirely and never leaves a
half-written file. [Performance
Optimizations](optimizationroadmap.md) covers this and the
other trade-offs in more depth.

## What the prototype does not model

pgfs reports fixed permissions (`0644` for files) and the
daemon's own uid and gid, and ignores `chmod`, `chown` and
explicit timestamps. It has no symbolic or hard links.
Inode numbers live only in memory. These are scoping
decisions, not accidents. The schema already carries `mode`,
`uid`, `gid` and a persistent `ino` column for the planned
inode-keyed storage engine described in [Dual-Interface
Architecture](dualinterfacearchitecture.md).

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
