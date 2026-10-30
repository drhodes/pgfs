# Tutorial: Reliable Mount & Shutdown

!!! warning "Status: Partially implemented"
    AutoUnmount, clean shutdown that commits every buffer and runs the synchronous barrier, health files, bounded waits, the connection state machine with reconnect and revalidation (Bug 5), op_log idempotency, serialization retries, the in-flight watchdog, the slow-database signal and the SQLSTATE table are built and tested. The fuser 0.14 crate never delivers FUSE_INTERRUPT to the filesystem, so `failure.HonoursInterrupt` is limited to the watchdog's cancels.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

**Tutorial** · [How-to](../how-to/reliablemount.md) · [Reference](../reference/reliablemount.md) · [Explanation](../explanation/reliablemount.md)

---

In this tutorial we will stop the database under a mounted filesystem
and watch the mount recover, then shut it down cleanly. We need a mount
started with `./scripts/pgfs.sh up`.

## Step 1: write something

```bash
echo before > testdata/mnt/f
```

## Step 2: restart PostgreSQL

```bash
pg_ctl -D testdata/pgdata -m fast restart -l testdata/postgres.log -o "-k $PWD/testdata -h 127.0.0.1"
echo after >> testdata/mnt/f
cat testdata/mnt/f
grep 'database state' testdata/pgfs.log | tail -4
```

```text
INFO pgfs::health: database state Connected -> Broken: ...
INFO pgfs::health: database state Broken -> Reconnecting
INFO pgfs::health: database state Reconnecting -> Revalidating
INFO pgfs::health: database state Revalidating -> Connected: revalidation passed
```

The append that found the connection broken waited for the mount to
reconnect and revalidate, then committed.

## Step 3: shut down cleanly

```bash
./scripts/pgfs.sh down
grep -E 'committing buffers|flushed to WAL|session released' testdata/pgfs.log | tail -3
```

SIGTERM commits every buffer, runs the synchronous barrier, unmounts,
and releases the lease.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
