# Explanation: Kernel Offloading

!!! warning "Status: Partially implemented"
    `copy_file_range` maps whole blocks to the source's blobs (shared, collected by GC) and copies only partial edge blocks, in one bounded transaction; `fallocate` extends, punches holes and zeroes ranges by changing map rows. Content search is planned.

> Copies and hole punching run inside PostgreSQL. `cp` shares blocks instead of copying bytes, and `fallocate` punches holes and zeroes ranges by changing block-map rows, so no file data passes through the daemon. Content search through database indexes is planned.

[Tutorial](../tutorials/kerneloffloadingacceleration.md) · [How-to](../how-to/kerneloffloadingacceleration.md) · [Reference](../reference/kerneloffloadingacceleration.md) · **Explanation**

---

## Why copies and holes run in the database

A FUSE copy normally reads every byte into the kernel, then into the
daemon, then into the database, and back out to write it. pgfs stores
file data as immutable blobs behind a block map, so a copy of whole
blocks is a set of new map rows pointing at the same blobs: no data
moves, and the copy costs a few rows per megabyte. This is a reflink
in all but name. `cp --reflink=always` itself cannot reach pgfs
because FUSE cannot pass `FICLONE` through, but `copy_file_range`
gets the same result.

Sharing has a cost: a blob with two references can no longer be freed
when one of them goes. Shared blobs are marked once and collected in
the background after a reference check, so the common path (files
never copied) still frees space at once.

A hole is the absence of a map row, so punching one is deleting rows,
and zeroing a range is the same plus rewriting the partly covered
edges.

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
