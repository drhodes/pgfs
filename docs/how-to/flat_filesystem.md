# How-To: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete run on the inode-keyed schema: persistent inode numbers with generations, buffered writes visible to every handle on the mount, read-modify-write of partial blocks (Bug 1), single-transaction unlink and checksummed immutable blocks. Covered by `tests/storage.rs`, the model-conformance suite and `scripts/test_*.sh`.

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

[Tutorial](../tutorials/flat_filesystem.md) · **How-to** · [Reference](../reference/flat_filesystem.md) · [Explanation](../explanation/flat_filesystem.md)

---

## Make sure data is durable

`write` returns once the data is buffered in the daemon. `close`
commits it (visible to everyone, durable within the asynchronous commit
window); `fsync` makes it durable before returning
([Durability and Crash Safety](durablestorage.md)).

## Find a file's rows

```sql
SELECT path, ino, size FROM pgfs.files WHERE tree = 'main' AND path = '/docs/notes.txt';
SELECT block_no, blob_id FROM pgfs.file_blocks
WHERE ino = 1001 AND died = 9223372036854775807 ORDER BY block_no;
```

## Diagnose an `Input/output error`

EIO means something unexpected failed. The daemon log has the full
error chain; `grep ERROR` it. A `checksum mismatch` names the inode,
block and blob ([Integrity and Backup](dataintegrity.md)).

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
