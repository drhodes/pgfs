# Tutorial: Kernel Offloading

!!! warning "Status: Partially implemented"
    Server-side `copy_file_range` and `fallocate` are implemented in the storage backend and unit-tested, but the FUSE layer does not call them yet. On a mount, copies stream through the daemon and `fallocate` fails with `EOPNOTSUPP`. Reflink, subtree clone and content search are planned.

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing copy_file_range, fallocate, reflink CoW, and server-side search to eliminate userspace byte streaming and deliver sub-millisecond execution for large-scale file operations.

**Tutorial** · [How-to](../how-to/kerneloffloadingacceleration.md) · [Reference](../reference/kerneloffloadingacceleration.md) · [Explanation](../explanation/kerneloffloadingacceleration.md)

---

The offloading callbacks are not connected to the mount yet,
but the idea behind them can be tried today with plain SQL. In
this tutorial we will copy a file without its bytes ever
leaving PostgreSQL, then run the unit test that exercises the
real implementation. We need a running mount (see [Your first
pgfs mount](projectlocalenvironment.md)).

## Step 1: create a file through the mount

```bash
mkdir testdata/mnt/offload
head -c 300000 /dev/urandom > testdata/mnt/offload/big.bin
```

## Step 2: copy it inside the database

One transaction copies the file's metadata row and its five
block rows. No data passes through pgfs or through `psql`:

```bash
psql -h "$PWD/testdata" -d pgfs -v ON_ERROR_STOP=1 <<'SQL'
BEGIN;
INSERT INTO entries (parent, name, kind, size)
  SELECT parent, 'clone.bin', kind, size FROM entries
  WHERE parent = 'offload' AND name = 'big.bin';
INSERT INTO blocks (parent, name, block_no, data)
  SELECT parent, 'clone.bin', block_no, data FROM blocks
  WHERE parent = 'offload' AND name = 'big.bin';
COMMIT;
SQL
```

## Step 3: see the copy appear on the mount

Wait a second for the kernel's attribute cache to expire, then:

```bash
ls -l testdata/mnt/offload
cmp testdata/mnt/offload/big.bin testdata/mnt/offload/clone.bin && echo identical
```

```text
-rw-r--r-- 1 alice alice 300000 Oct 26 17:13 big.bin
-rw-r--r-- 1 alice alice 300000 Oct 26 17:13 clone.bin
identical
```

This is what `copy_file_range` offloading does on the
inode-keyed schema: one set-based `INSERT … SELECT` instead of
reading and rewriting every byte.

## Step 4: run the real implementation's test

```bash
cargo test tests_offload
```

```text
test db::tests_offload::test_backend_offloaded_copy_file_range_and_fallocate ... ok
```

The test copies 128 KiB between two files with
`copy_file_range`, then punches it back out with `fallocate`.

## Step 5: clean up

```bash
rm -r testdata/mnt/offload
```

## What we did

We copied a file entirely inside PostgreSQL and saw it through
the mount, then ran the backend code that will do the same for
`cp` once it is wired to FUSE. The
[explanation](../explanation/kerneloffloadingacceleration.md)
covers the rest of the plan.

---

<small>Generated from `spec.offload.KernelOffloadingAcceleration` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
