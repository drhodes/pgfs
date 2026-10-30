# Tutorial: Atomic Rename

!!! success "Status: Stable"
    A rename moves one dentry in constant time with no name matching (Bug 2), commits the source's buffered writes first (Bug 3), takes the rename lock for directory moves, and honours `RENAME_NOREPLACE` and `RENAME_EXCHANGE`. Covered by `tests/storage.rs`, the model-conformance suite, `test_rename.sh`, `test_posix.sh` and the git workload.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

**Tutorial** · [How-to](../how-to/atomicrename.md) · [Reference](../reference/atomicrename.md) · [Explanation](../explanation/atomicrename.md)

---

In this tutorial we will replace a file atomically the way editors and
package managers do, swap two names, and move a directory. We need a
mount.

## Step 1: write-then-rename

```bash
cd testdata/mnt
echo "version 1" > config
echo "version 2" > config.tmp
mv config.tmp config
cat config
```

Readers see either version 1 or version 2, never a mix; after a crash
the new name never points at a file missing its writes.

## Step 2: swap two names

```bash
echo a > left; echo b > right
mv --exchange left right
cat left right
```

## Step 3: move a large directory

```bash
mkdir -p big/sub && for i in $(seq 1 1000); do : > big/sub/f$i; done
time mv big moved
```

The move changes one row, so it takes as long as moving an empty
directory.

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
