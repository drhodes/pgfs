# Explanation: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and nested paths of any depth are implemented and covered by `test_dirs.sh`. Directory timestamps are not tracked.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

[Tutorial](../tutorials/hierarchicaldirectories.md) · [How-to](../how-to/hierarchicaldirectories.md) · [Reference](../reference/hierarchicaldirectories.md) · **Explanation**

---

## Directories are rows too

A directory is an `entries` row with `kind = 'dir'` and no
blocks. What makes the tree a tree is the `parent` column:
every row stores the full path of the directory that contains
it. Listing a directory is a single indexed query,
`WHERE parent = 'project/src'`, and finding everything below a
directory is a prefix match on that column.

This is the materialised-path pattern. Reading is easy:
listings, subtree queries and path lookups are each one
indexed query, and the rows are readable on their own. Moving
things is harder, because a directory's path is copied into
every descendant row, so renaming the directory rewrites them
all. [Atomic Rename](../explanation/atomicrename.md) explains
how pgfs keeps that atomic, and [Performance
Optimizations](../explanation/optimizationroadmap.md) describes
the planned inode-keyed layout that would make it
constant-time.

## The root

The root has no parent, so it is stored as the row with an
empty `parent` and an empty `name`, created together with the
schema and pinned to inode 1, which FUSE requires. Its
attributes are synthesised and never fetched, so the root can
always be listed, even when the database is unreachable.

## What directories do not track

On a local filesystem a directory's link count is two plus its
number of subdirectories, and its modification time changes
whenever an entry is added or removed. pgfs reports a fixed
link count of 2 and the epoch as every timestamp. Tracking
either would mean updating the parent's row on every create,
delete and rename, adding a write to operations that are
currently a single statement. Few programs depend on these
values; tools that do (backup software comparing directory
mtimes, for example) treat every pgfs directory as unchanged.

## Why `rmdir` refuses non-empty directories

POSIX requires it, and it is a safety feature. `rm -r` is the
explicit way to delete a subtree, and it walks the tree
bottom-up, removing each entry through the normal unlink and
rmdir paths.

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
