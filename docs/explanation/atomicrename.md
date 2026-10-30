# Explanation: Atomic Rename

!!! success "Status: Stable"
    A rename moves one dentry in constant time with no name matching (Bug 2), commits the source's buffered writes first (Bug 3), takes the rename lock for directory moves, and honours `RENAME_NOREPLACE` and `RENAME_EXCHANGE`. Covered by `tests/storage.rs`, the model-conformance suite, `test_rename.sh`, `test_posix.sh` and the git workload.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

[Tutorial](../tutorials/atomicrename.md) · [How-to](../how-to/atomicrename.md) · [Reference](../reference/atomicrename.md) · **Explanation**

---

## One row, no patterns

A rename moves one dentry row and, at most, removes one replaced
target. No name matching is involved, which is what makes Bug 2
impossible: the first schema found descendants with `LIKE`, so `_` and
`%` in names acted as wildcards.

## Why buffered writes go first

A program that writes a file and renames it expects the new name to
show the writes. pgfs commits the file's pending writes before the
rename's own transaction, and PostgreSQL's WAL keeps that order through
a crash (Bug 3 was a rename that lost them).

## Why directory moves take a lock

Two concurrent moves of directories into each other could form a
cycle. A move to a different parent takes a per-layer lock and checks
that the destination is not inside the source, in the same
transaction.

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
