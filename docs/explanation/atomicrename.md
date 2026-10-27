# Explanation: Atomic Rename

!!! success "Status: Stable"
    File and directory renames, overwrite semantics and the full POSIX error matrix are implemented and covered by `test_rename.sh` and `test_git_init.sh`. `renameat2` flags (`RENAME_NOREPLACE`, `RENAME_EXCHANGE`) are rejected.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

[Tutorial](../tutorials/atomicrename.md) · [How-to](../how-to/atomicrename.md) · [Reference](../reference/atomicrename.md) · **Explanation**

---

## Why rename has to be atomic

Rename is how Unix programs publish changes safely. An editor
saves by writing a temporary file and renaming it over the
original. Package managers install files the same way. Git
guards every configuration change and every branch update with
a lock file that is renamed into place. All of them rely on one
guarantee: at every instant the target name refers to a
complete file, either the old one or the new one.

A filesystem that implemented rename as "delete the target,
then move the source" would open a window in which the name
refers to nothing. A crash in that window loses the file. So
rename was the first pgfs feature that had to be transactional
rather than merely correct.

## One transaction, every row

In pgfs every entry stores the full path of its parent
directory. Moving a file therefore changes one `entries` row
and its blocks. Moving a directory changes every row beneath
it, because every one of those rows has the old path baked in.

pgfs does all of it, including deleting an overwritten target,
in a single PostgreSQL transaction. PostgreSQL then supplies
the guarantees. Atomicity: after a crash the rename either
happened completely or not at all. Isolation: any other
connection sees the tree only before or after the move. The
daemon's own in-memory inode map is updated only after the
commit succeeds, so a failed rename leaves no trace.

## The price of materialised paths

Rewriting every descendant makes the cost of a directory rename
proportional to the size of its subtree. Renaming a directory
holding a million files rewrites a million rows. Local
filesystems avoid this by naming directories with inode numbers
instead of paths, which makes a move a single pointer update.
The schema already carries `ino` and `parent_ino` columns for
exactly that change, described as `InoBasedRekeyingReq` in
[Performance Optimizations](optimizationroadmap.md).

## Why `git` is the test

`git init` exercises rename-overwrite immediately: it creates
`config`, then rewrites it several times through
`config.lock`. It was also the first real program run against
pgfs, and it failed until directories were implemented.
Git's workflow, made of lock files, renames and readers that
expect complete files, is a compact stand-in for the guarantees
real applications need, so `test_git_init.sh` is part of the
integration suite.

## Why `renameat2` flags are refused

`RENAME_NOREPLACE` and `RENAME_EXCHANGE` need their own atomic
implementations: "fail if the target exists" and "swap two
entries". Rather than approximate them, pgfs rejects them with
`EINVAL`, which well-behaved tools such as GNU `mv` already
handle by falling back to plain rename.

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
