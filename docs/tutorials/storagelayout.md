# Tutorial: Storage Layout

!!! success "Status: Stable"
    `pgfs mkfs` creates the versioned `pgfs` schema: inode and dentry rows keyed by persistent inode numbers with random generations, byte-exact names, inline small files, fixed-size immutable checksummed blobs behind a block map, epoch-versioned xattrs, every operation as a SQL function, and the read-only views.

> Every byte of a pgfs filesystem is a row in PostgreSQL: inodes, names, attributes and data blocks. Inode numbers are permanent, names are exact bytes, small files take one row, and file data is immutable, checksummed and shareable between files, snapshots and branches.

**Tutorial** · [How-to](../how-to/storagelayout.md) · [Reference](../reference/storagelayout.md) · [Explanation](../explanation/storagelayout.md)

---

In this tutorial we will look at one small file and one large
file as rows, and see how a name, an inode and data blocks
relate. We need a mounted, freshly made filesystem (see
[Administering pgfs](filesystemlifecycle.md)).

## Step 1: create two files

```bash
echo "hello" > /mnt/pgfs/small.txt
head -c 200000 /dev/urandom > /mnt/pgfs/big.bin
ls -i /mnt/pgfs
```

```text
1001 big.bin  1000 small.txt
```

The numbers are inode numbers. They are the `ino` column in
the database and never change.

## Step 2: find the names

```sql
SELECT path, ino, kind, size FROM pgfs.files WHERE tree = 'main';
```

```text
   path     | ino  | kind |  size
------------+------+------+--------
 /          |    1 | d    |      3
 /big.bin   | 1001 | f    | 200000
 /small.txt | 1000 | f    |      6
```

## Step 3: see where the data lives

The small file is stored inside its inode row:

```sql
SELECT ino, size, inline_data FROM pgfs.inodes
WHERE ino = 1000 AND died = 9223372036854775807;
```

The large file has a block map with four 64 KiB blocks, each
pointing to an immutable blob:

```sql
SELECT block_no, blob_id FROM pgfs.file_blocks
WHERE ino = 1001 AND died = 9223372036854775807 ORDER BY block_no;
```

## What we did

We saw that a name is a dentry row, a file is an inode row,
small files live in the inode, and large files are a map of
blocks to blobs. The
[explanation](../explanation/storagelayout.md) says why.

---

<small>Generated from `spec.storage.StorageLayout` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
