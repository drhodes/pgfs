# How-To: Kernel Offloading

!!! warning "Status: Partially implemented"
    Server-side `copy_file_range` and `fallocate` are implemented in the storage backend and unit-tested, but the FUSE layer does not call them yet. On a mount, copies stream through the daemon and `fallocate` fails with `EOPNOTSUPP`. Reflink, subtree clone and content search are planned.

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing copy_file_range, fallocate, reflink CoW, and server-side search to eliminate userspace byte streaming and deliver sub-millisecond execution for large-scale file operations.

[Tutorial](../tutorials/kerneloffloadingacceleration.md) · **How-to** · [Reference](../reference/kerneloffloadingacceleration.md) · [Explanation](../explanation/kerneloffloadingacceleration.md)

---

## Copy a file

`cp` works and the copy is exact, but today every byte passes
through the daemon. `cp` calls `copy_file_range(2)`; pgfs does
not implement the FUSE callback, so the kernel falls back to
reading and writing.

## Copy a large file or tree faster today

Copy inside the database, as in the tutorial, while nothing is
writing to the source:

```sql
BEGIN;
INSERT INTO entries (parent, name, kind, size)
  SELECT parent, 'copy.bin', kind, size FROM entries WHERE parent = 'dir' AND name = 'orig.bin';
INSERT INTO blocks (parent, name, block_no, data)
  SELECT parent, 'copy.bin', block_no, data FROM blocks WHERE parent = 'dir' AND name = 'orig.bin';
COMMIT;
```

The new file appears on the mount once the attribute cache
expires (`--attr-ttl`, default 1 s).

## Preallocate or punch holes

Not available on a mount: `fallocate` fails with
`Operation not supported`. To make a file sparse, truncate it
and rewrite the parts you need, or extend it with
`truncate -s`, which never stores the new bytes.

## Clone with `cp --reflink`

Not available: `cp --reflink=always` fails with
`Operation not supported`. Plain `cp` (or
`cp --reflink=auto`) works.

## Search file contents in the database

Not available as a filesystem interface. For text files you
can query blocks directly:

```sql
SELECT DISTINCT parent || '/' || name FROM blocks
WHERE position(convert_to('needle', 'UTF8') in data) > 0;
```

This scans every block, and it misses matches that span a
block boundary.

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
