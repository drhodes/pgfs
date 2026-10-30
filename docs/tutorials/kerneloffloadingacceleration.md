# Tutorial: Kernel Offloading

!!! warning "Status: Partially implemented"
    `copy_file_range` maps whole blocks to the source's blobs (shared, collected by GC) and copies only partial edge blocks, in one bounded transaction; `fallocate` extends, punches holes and zeroes ranges by changing map rows. Content search is planned.

> Copies and hole punching run inside PostgreSQL. `cp` shares blocks instead of copying bytes, and `fallocate` punches holes and zeroes ranges by changing block-map rows, so no file data passes through the daemon. Content search through database indexes is planned.

**Tutorial** · [How-to](../how-to/kerneloffloadingacceleration.md) · [Reference](../reference/kerneloffloadingacceleration.md) · [Explanation](../explanation/kerneloffloadingacceleration.md)

---

In this tutorial we will copy a large file with `cp` and watch pgfs
share its blocks instead of copying them, then punch a hole in a file
and see the space go. We need a mounted filesystem (see
[Your first pgfs mount](projectlocalenvironment.md)).

## Step 1: make a file and copy it

```bash
head -c 2000000 /dev/urandom > testdata/mnt/orig.bin
cp testdata/mnt/orig.bin testdata/mnt/copy.bin
cmp testdata/mnt/orig.bin testdata/mnt/copy.bin && echo identical
```

```text
identical
```

coreutils 9 `cp` calls `copy_file_range(2)`, which reaches pgfs's
`copy_file_range` callback. Whole blocks at the same offset become
references to the source's blobs.

## Step 2: see the sharing

```bash
psql -h "$PWD/testdata" -d pgfs -c "SELECT count(*) AS shared_blobs FROM pgfs.blobs WHERE shared"
```

```text
 shared_blobs
--------------
           30
```

Thirty 64 KiB blocks are referenced by both files; only the partly
covered last block was written again.

## Step 3: punch a hole

```bash
fallocate --punch-hole --offset 65536 --length 655360 testdata/mnt/copy.bin
du -k testdata/mnt/copy.bin
```

The ten blocks in the range read as zero and no longer count in `du`.

## What we did

We saw a copy that moved no data through the daemon, and a hole
punched by removing map rows. The
[explanation](../explanation/kerneloffloadingacceleration.md) says why
these run inside PostgreSQL.

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
