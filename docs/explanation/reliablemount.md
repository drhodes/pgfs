# Explanation: Reliable Mount & Shutdown

!!! warning "Status: Partially implemented"
    AutoUnmount, clean shutdown that commits every buffer and runs the synchronous barrier, health files, bounded waits, the connection state machine with reconnect and revalidation (Bug 5), op_log idempotency, serialization retries, the in-flight watchdog, the slow-database signal and the SQLSTATE table are built and tested. The fuser 0.14 crate never delivers FUSE_INTERRUPT to the filesystem, so `failure.HonoursInterrupt` is limited to the watchdog's cancels.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

[Tutorial](../tutorials/reliablemount.md) · [How-to](../how-to/reliablemount.md) · [Reference](../reference/reliablemount.md) · **Explanation**

---

## Fail fast, recover by revalidating

When the database goes away, a mount that queues requests builds a
backlog that keeps it overloaded after the database returns. pgfs
fails new requests at once with EIO, lets only the request in flight
wait, and reconnects with jittered backoff. Before serving again it
checks that the database is the same one with the same history: the
system identifier, the timeline, the schema and `fs_uuid`, its own
lease row, and that every worker's op_log slot holds at least the last
mutation it acknowledged. If an acknowledged commit was lost, the
mount fences itself rather than serve a rewound filesystem.

## A watchdog that can see a stuck worker

A heartbeat thread can only notice that the heartbeat thread is stuck.
pgfs records each worker's in-flight request in an atomic slot, and a
separate thread watches the slots' ages, so a worker stuck in a query
is reported, dumped and cancelled, never turned into a lost mount.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
