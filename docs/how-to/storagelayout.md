# How-To: Storage Layout

!!! success "Status: Stable"
    `pgfs mkfs` creates the versioned `pgfs` schema: inode and dentry rows keyed by persistent inode numbers with random generations, byte-exact names, inline small files, fixed-size immutable checksummed blobs behind a block map, epoch-versioned xattrs, every operation as a SQL function, and the read-only views.

> Every byte of a pgfs filesystem is a row in PostgreSQL: inodes, names, attributes and data blocks. Inode numbers are permanent, names are exact bytes, small files take one row, and file data is immutable, checksummed and shareable between files, snapshots and branches.

[Tutorial](../tutorials/storagelayout.md) · **How-to** · [Reference](../reference/storagelayout.md) · [Explanation](../explanation/storagelayout.md)

---

## Query the filesystem with SQL

Use the views, which stay stable across schema versions:

```sql
-- the 20 largest files in the main tree
SELECT path, size FROM pgfs.files
WHERE tree = 'main' AND kind = 'f'
ORDER BY size DESC LIMIT 20;
```

## Find a file by inode number

```sql
SELECT path FROM pgfs.files WHERE tree = 'main' AND ino = 1001;
```

## Read names that are not UTF-8

`pgfs.files.path` shows such bytes as `\x` escapes. The raw
bytes are in `pgfs.dentries.name`.

## Change the filesystem

Use the mount or the `pgfs` CLI. Writing to the tables
directly is not supported, and `pgfs check` reports the
damage it causes.

---

<small>Generated from `spec.storage.StorageLayout` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
