# Tutorial: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and `readdirplus` keep link counts and directory times, page by name with keyset cookies, and check emptiness in the same transaction as the delete. Covered by `tests/storage.rs`, the model-conformance suite and `test_dirs.sh`.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

**Tutorial** · [How-to](../how-to/hierarchicaldirectories.md) · [Reference](../reference/hierarchicaldirectories.md) · [Explanation](../explanation/hierarchicaldirectories.md)

---

In this tutorial we will build a small project tree, list it, and see
how directories are stored. We need a mount.

## Step 1: build a tree

```bash
cd testdata/mnt
mkdir -p project/src/util project/docs
touch project/src/main.rs project/src/util/strings.rs project/docs/README.md
ls -la project/src
```

## Step 2: see the link counts

```bash
stat -c '%n %h' project project/src
```

A directory's link count is 2 plus its subdirectories.

## Step 3: see how the tree is stored

```bash
psql -h "$OLDPWD/testdata" -d pgfs -c \
  "SELECT path, kind FROM pgfs.files WHERE tree = 'main' AND path LIKE '/project%' ORDER BY path"
```

Each name is a `dentries` row `(parent_ino, name) -> ino`; no row
stores a path.

## Step 4: remove it

```bash
rm -r project
```

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
