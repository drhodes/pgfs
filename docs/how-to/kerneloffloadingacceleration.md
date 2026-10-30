# How-To: Kernel Offloading

!!! warning "Status: Partially implemented"
    `copy_file_range` maps whole blocks to the source's blobs (shared, collected by GC) and copies only partial edge blocks, in one bounded transaction; `fallocate` extends, punches holes and zeroes ranges by changing map rows. Content search is planned.

> Copies and hole punching run inside PostgreSQL. `cp` shares blocks instead of copying bytes, and `fallocate` punches holes and zeroes ranges by changing block-map rows, so no file data passes through the daemon. Content search through database indexes is planned.

[Tutorial](../tutorials/kerneloffloadingacceleration.md) · **How-to** · [Reference](../reference/kerneloffloadingacceleration.md) · [Explanation](../explanation/kerneloffloadingacceleration.md)

---

## Copy without moving data

Use any tool that calls `copy_file_range(2)`: `cp` from coreutils 9,
`sendfile`, or Python's `os.copy_file_range`. Whole blocks are shared
when the source and destination offsets are equal modulo the block
size; other bytes are copied by the daemon. A copy larger than 4 096
blocks replies with the bytes copied so far, and the caller continues.

## Punch holes and zero ranges

```bash
fallocate --punch-hole --offset OFFSET --length LENGTH FILE
fallocate --zero-range --offset OFFSET --length LENGTH FILE
fallocate --length LENGTH FILE        # extend the size; no preallocation
```

`FALLOC_FL_COLLAPSE_RANGE` and `FALLOC_FL_INSERT_RANGE` reply
EOPNOTSUPP.

## See how much is shared

```sql
SELECT count(*) FILTER (WHERE shared) AS shared, count(*) AS blobs FROM pgfs.blobs;
SELECT gc_queue FROM pgfs.backlog();
```

Shared blobs are freed by the background collector once no map row
references them (`space.CollectsSharedBlobs`).

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
