# Tutorial: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete run on the inode-keyed schema: persistent inode numbers with generations, buffered writes visible to every handle on the mount, read-modify-write of partial blocks (Bug 1), single-transaction unlink and checksummed immutable blocks. Covered by `tests/storage.rs`, the model-conformance suite and `scripts/test_*.sh`.

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

**Tutorial** · [How-to](../how-to/flat_filesystem.md) · [Reference](../reference/flat_filesystem.md) · [Explanation](../explanation/flat_filesystem.md)

---

In this tutorial we will create, write, append to, truncate and delete
a file, and look at its inode in PostgreSQL. We need a mount (see
[Your first pgfs mount](projectlocalenvironment.md)).

## Step 1: create and write

```bash
cd testdata/mnt
echo "first line" > notes.txt
echo "second line" >> notes.txt
cat notes.txt
ls -i notes.txt
```

The inode number `ls -i` prints is the file's `ino` in the database,
and stays the same across remounts.

## Step 2: look at the inode

```bash
psql -h "$OLDPWD/testdata" -d pgfs -c \
  "SELECT ino, size, convert_from(inline_data, 'UTF8') FROM pgfs.inodes
   WHERE ino = $(stat -c %i notes.txt) AND died = 9223372036854775807"
```

The 23 bytes are inside the inode row.

## Step 3: truncate and delete

```bash
truncate -s 5 notes.txt && cat notes.txt; echo
rm notes.txt
```

## What we did

We used ordinary tools on a pgfs file, and saw that a file is an inode
row with a permanent number.

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
