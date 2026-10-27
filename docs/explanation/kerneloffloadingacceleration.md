# Explanation: Kernel Offloading

!!! warning "Status: Partially implemented"
    Server-side `copy_file_range` and `fallocate` are implemented in the storage backend and unit-tested, but the FUSE layer does not call them yet. On a mount, copies stream through the daemon and `fallocate` fails with `EOPNOTSUPP`. Reflink, subtree clone and content search are planned.

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing copy_file_range, fallocate, reflink CoW, and server-side search to eliminate userspace byte streaming and deliver sub-millisecond execution for large-scale file operations.

[Tutorial](../tutorials/kerneloffloadingacceleration.md) · [How-to](../how-to/kerneloffloadingacceleration.md) · [Reference](../reference/kerneloffloadingacceleration.md) · **Explanation**

---

## Moving work to where the data is

In a normal copy, a program reads a file into its own memory
and writes it back out. On pgfs, that means every byte travels
from PostgreSQL to the daemon, up to the program through the
kernel, back down through the kernel to the daemon, and into
PostgreSQL again: four copies to produce data the database
already has.

Linux provides calls that describe intent instead of moving
bytes. `copy_file_range` means "make this range of that file
appear here". `fallocate` means "reserve this space" or "this
range is now empty". A filesystem that understands them can do
the work where the data lives. For pgfs that is the query
engine: a copy becomes one `INSERT … SELECT` over block rows,
and punching a hole becomes one `DELETE`.

## The roadmap

The same idea extends further. With reference-counted blocks,
a reflink copy (`cp --reflink`) would copy only references,
and blocks would be duplicated lazily on the first write.
Cloning a whole directory tree could be one transaction of
set-based inserts. Content search could use PostgreSQL's
trigram indexes instead of reading every file through FUSE.
Each of these is specified as a requirement; none is built
yet.

## Why it is not wired in yet

The offloading code is written against the inode-keyed
storage interface, the future of pgfs storage. The mount still
runs on the path-keyed layout. Exposing the callbacks before
the FUSE layer moves to the new interface would mean
implementing them twice, against two schemas and two block
sizes. The [Dual-Interface Architecture](dualinterfacearchitecture.md)
page describes that migration.

## Correctness before speed

Offloaded operations must be indistinguishable from the slow
path: same bytes, same sizes, same holes. That is why the
backend falls back to block-by-block copying whenever ranges
are not block-aligned. A fast path that changed results would
be worse than no fast path.

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
