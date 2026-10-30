# Tutorial: Durability and Crash Safety

!!! success "Status: Stable"
    fsync, fdatasync and `O_SYNC` writes commit synchronously and confirm replication with `pgfs.sync_barrier`; other operations commit asynchronously, renames commit pending writes first, close reports errors through `errseq_t`, shutdown flushes and barriers, the server's durability settings are checked, and scratch filesystems reset after a crash. `test_crash.sh` kills the daemon and Postgres; the LazyFS crash matrix is part of `verify.CrashConformance`.

> pgfs keeps the POSIX promise: data is durable when fsync returns. Everything else is fast and becomes durable in order, so after any crash the filesystem is an earlier moment of its history, and write-then-rename is atomic even without fsync.

**Tutorial** · [How-to](../how-to/durablestorage.md) · [Reference](../reference/durablestorage.md) · [Explanation](../explanation/durablestorage.md)

---

In this tutorial we will see the durability promise at work:
an fsync that waits for the disk, and a rename that a crash
cannot reorder. We need a mounted filesystem at `/mnt/pgfs`
and the project-local cluster.

## Step 1: see the commit window

```bash
pgfs stats main | grep durability
```

```text
durability: fsync=synchronous  others=async within 600 ms
```

## Step 2: write a file and sync it

```bash
python3 - <<'PY'
import os, time
fd = os.open("/mnt/pgfs/ledger", os.O_CREAT | os.O_WRONLY, 0o644)
os.write(fd, b"entry 1\n")
t = time.perf_counter(); os.fsync(fd)
print(f"fsync took {1000 * (time.perf_counter() - t):.2f} ms")
os.close(fd)
PY
```

The fsync takes as long as one disk flush. When it returns,
the entry survives a power cut.

## Step 3: replace a file safely without fsync

```bash
echo "new config" > /mnt/pgfs/app.conf.tmp
mv /mnt/pgfs/app.conf.tmp /mnt/pgfs/app.conf
```

If the machine crashes now, `app.conf` holds either the old
content or `new config`, never an empty file.

## What we did

We saw that fsync waits for the disk, and that pgfs keeps
operations in order across a crash, so the common
write-then-rename pattern is safe.

---

<small>Generated from `spec.durability.DurableStorage` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
