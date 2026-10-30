# How-To: Atomic Rename

!!! success "Status: Stable"
    A rename moves one dentry in constant time with no name matching (Bug 2), commits the source's buffered writes first (Bug 3), takes the rename lock for directory moves, and honours `RENAME_NOREPLACE` and `RENAME_EXCHANGE`. Covered by `tests/storage.rs`, the model-conformance suite, `test_rename.sh`, `test_posix.sh` and the git workload.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

[Tutorial](../tutorials/atomicrename.md) · **How-to** · [Reference](../reference/atomicrename.md) · [Explanation](../explanation/atomicrename.md)

---

## Replace a file atomically

Write a temporary file in the same directory and `rename` it over the
target. No fsync is needed for atomicity: pgfs commits the temporary
file's writes before the rename.

## Refuse to replace

```bash
mv --no-clobber src dst       # RENAME_NOREPLACE: fails if dst exists
```

## Swap two entries

```bash
mv --exchange a b             # RENAME_EXCHANGE
```

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
