# How-To: Durability and Crash Safety

!!! success "Status: Stable"
    fsync, fdatasync and `O_SYNC` writes commit synchronously and confirm replication with `pgfs.sync_barrier`; other operations commit asynchronously, renames commit pending writes first, close reports errors through `errseq_t`, shutdown flushes and barriers, the server's durability settings are checked, and scratch filesystems reset after a crash. `test_crash.sh` kills the daemon and Postgres; the LazyFS crash matrix is part of `verify.CrashConformance`.

> pgfs keeps the POSIX promise: data is durable when fsync returns. Everything else is fast and becomes durable in order, so after any crash the filesystem is an earlier moment of its history, and write-then-rename is atomic even without fsync.

[Tutorial](../tutorials/durablestorage.md) · **How-to** · [Reference](../reference/durablestorage.md) · [Explanation](../explanation/durablestorage.md)

---

## Make a write durable

Call `fsync` or `fdatasync` on the file, or open it with
`O_SYNC` or `O_DSYNC`. After a directory change you need to
survive, call `fsync` on the directory.

## Survive the loss of the database server

Configure a synchronous standby on the Postgres side
(`synchronous_standby_names`). pgfs's fsync then waits for
the standby too. Without one, an fsync survives a crash but
not the loss of the primary's disk.

## Run a disposable filesystem for caches

```bash
pgfs mkfs --scratch --conn "$CONN"
```

Everything is lost on a database crash.

## Mount against a server with fsync turned off

Don't. If you must, for a throwaway benchmark:

```bash
pgfs mount --i-accept-data-loss main /mnt/pgfs
```

---

<small>Generated from `spec.durability.DurableStorage` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
