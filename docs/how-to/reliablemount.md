# How-To: Reliable Mount & Shutdown

!!! warning "Status: Partially implemented"
    AutoUnmount, clean shutdown that commits every buffer and runs the synchronous barrier, health files, bounded waits, the connection state machine with reconnect and revalidation (Bug 5), op_log idempotency, serialization retries, the in-flight watchdog, the slow-database signal and the SQLSTATE table are built and tested. The fuser 0.14 crate never delivers FUSE_INTERRUPT to the filesystem, so `failure.HonoursInterrupt` is limited to the watchdog's cancels.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

[Tutorial](../tutorials/reliablemount.md) · **How-to** · [Reference](../reference/reliablemount.md) · [Explanation](../explanation/reliablemount.md)

---

## Take over a tree from a crashed daemon

```bash
pgfs mount --takeover main /srv/pgfs
```

The new mount waits for the old lease to expire (`--lease`, default
30 s), then acquires it and reclaims the old mount's orphans.

## Find a stuck request

The watchdog logs a WARN past `--stall-warn` (5 s) naming the worker,
operation, inode and backend pid, and past `--stall-error` (60 s)
writes a state dump and cancels the query. Match the pid in
`pg_stat_activity`.

## Run under systemd

Install `contrib/pgfs@.service`; the daemon sends `READY=1`,
`STOPPING=1` and `WATCHDOG=1` while no request is stalled.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
