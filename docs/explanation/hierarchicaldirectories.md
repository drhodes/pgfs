# Explanation: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and `readdirplus` keep link counts and directory times, page by name with keyset cookies, and check emptiness in the same transaction as the delete. Covered by `tests/storage.rs`, the model-conformance suite and `test_dirs.sh`.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

[Tutorial](../tutorials/hierarchicaldirectories.md) · [How-to](../how-to/hierarchicaldirectories.md) · [Reference](../reference/hierarchicaldirectories.md) · **Explanation**

---

## Names point at inodes

A directory entry is a row `(parent_ino, name) -> ino`. A lookup is one
index probe, a listing is one range scan in byte order of name, and a
rename changes one row whatever the size of the subtree. The first pgfs
schema keyed rows by path, which made a directory rename rewrite every
descendant.

## Why emptiness is checked inside the transaction

`rmdir` locks the directory's inode row before checking that it is
empty, and a create in that directory locks the same row, so no create
can slip in between the check and the delete.

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
