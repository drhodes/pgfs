# Tutorial: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and nested paths of any depth are implemented and covered by `test_dirs.sh`. Directory timestamps are not tracked.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

**Tutorial** · [How-to](../how-to/hierarchicaldirectories.md) · [Reference](../reference/hierarchicaldirectories.md) · [Explanation](../explanation/hierarchicaldirectories.md)

---

In this tutorial we will build a small directory tree, walk it
with standard tools, see how it is stored, and take it apart
again. We will also meet the one rule directories enforce:
they must be empty before they can be removed.

We need a running mount; see [Your first pgfs
mount](projectlocalenvironment.md) if `./scripts/pgfs.sh
status` does not say `mount: active`.

## Step 1: build a tree

```bash
cd testdata/mnt
mkdir -p project/src/util project/docs
touch project/src/main.rs project/src/util/strings.rs project/docs/README.md
```

## Step 2: walk it

```bash
find project | sort
```

```text
project
project/docs
project/docs/README.md
project/src
project/src/main.rs
project/src/util
project/src/util/strings.rs
```

```bash
ls -la project/src
```

The listing starts with the `.` and `..` entries, followed by
`main.rs` and `util`.

## Step 3: see how the tree is stored

From the repository root, in a second terminal:

```bash
psql -h "$PWD/testdata" -d pgfs -c \
  "SELECT parent, name, kind FROM entries
   WHERE parent = 'project' OR starts_with(parent, 'project/')
      OR (parent = '' AND name = 'project')
   ORDER BY parent, name"
```

```text
      parent       |    name    | kind
-------------------+------------+------
                   | project    | dir
 project           | docs       | dir
 project           | src        | dir
 project/docs      | README.md  | file
 project/src       | main.rs    | file
 project/src       | util       | dir
 project/src/util  | strings.rs | file
```

Every row names the directory it lives in. `project` itself
lives at the top of the mount, where `parent` is empty.

## Step 4: try to remove a directory that is not empty

```bash
rmdir project/src
```

```text
rmdir: failed to remove 'project/src': Directory not empty
```

## Step 5: take the tree apart

Empty the directory from the bottom up, and `rmdir` succeeds:

```bash
rm project/src/util/strings.rs
rmdir project/src/util
rm project/src/main.rs
rmdir project/src
```

Remove the rest in one go:

```bash
rm -r project
ls
```

`project` no longer appears.

## What we did

We created nested directories, listed them, saw that each
entry stores the path of its containing directory, and learned
that `rmdir` only removes empty directories. Next, [Atomic
Rename](atomicrename.md) moves whole trees in one step.

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
