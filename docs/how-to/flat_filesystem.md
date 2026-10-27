# How-To: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete are implemented and covered by unit and integration tests. Permissions, ownership and timestamps are reported but not settable (see Reference).

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

[Tutorial](../tutorials/flat_filesystem.md) · **How-to** · [Reference](../reference/flat_filesystem.md) · [Explanation](../explanation/flat_filesystem.md)

---

All paths below are inside a mounted pgfs filesystem.

## Create an empty file

```bash
touch notes.txt
```

## Replace a file's contents

```bash
printf 'new contents\n' > notes.txt
```

To replace contents so that readers see either the old or the
new version and never a partial one, write a temporary file and
rename it over the target (see [Atomic
Rename](atomicrename.md)):

```bash
printf 'new contents\n' > .notes.txt.tmp && mv .notes.txt.tmp notes.txt
```

## Append

```bash
echo 'another line' >> notes.txt
```

## Overwrite bytes in the middle of a file

```bash
printf 'XY' | dd of=notes.txt bs=1 seek=4 conv=notrunc
```

## Make written data visible outside the mount

Writes are held in memory per open file and committed to
PostgreSQL when the file is closed, flushed or fsynced.
Processes reading through the same mount always see their own
and each other's writes, because a read flushes pending writes
first. A SQL client, or a second pgfs daemon on the same
database, sees the data once the writer closes the file or
calls `fsync`.

## Truncate or extend a file

```bash
truncate -s 0 notes.txt      # empty it
truncate -s 1M notes.txt     # extend with zeros
```

## Create a sparse file

```bash
truncate -s 1G sparse.img
```

Ranges that were never written have no block rows and read as
zeros, so this costs one row in `entries` and nothing in
`blocks`.

## Delete a file

```bash
rm notes.txt
```

## Find a file's rows in PostgreSQL

A file at `docs/sub/notes.txt` has `parent = 'docs/sub'` and
`name = 'notes.txt'`:

```sql
SELECT kind, size, mtime FROM entries WHERE parent = 'docs/sub' AND name = 'notes.txt';
SELECT block_no, length(data) FROM blocks WHERE parent = 'docs/sub' AND name = 'notes.txt' ORDER BY block_no;
```

Files at the top of the mount have `parent = ''`.

## Diagnose an `Input/output error`

EIO means something unexpected failed, usually the database.
The full error chain is in the daemon log:

```bash
grep ERROR testdata/pgfs.log | tail -5
```

Each entry names the operation, the source location and the
root cause; see [Observability](observabledaemon.md).

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
