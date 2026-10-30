# Explanation: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing with spans per callback (worker and view included) and per database call (statement, inode, backend pid, round trip), the nine 60-second metrics lines with every errno counted where it is replied, the SIGUSR1 dump written by the signal thread from atomics, and crash reports all ship. A test keeps the instrumented callback list in step with the `Filesystem` impl.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

[Tutorial](../tutorials/observabledaemon.md) · [How-to](../how-to/observabledaemon.md) · [Reference](../reference/observabledaemon.md) · **Explanation**

---

## Everything an operator needs is in the log

FUSE can only return an errno to the program; the reason for an EIO
lives in the daemon. pgfs therefore logs every unexpected failure with
its whole error chain, keeps expected errnos out of the log, and counts
both, so a spike in "file not found" can be told apart from a database
failing.

## Signals that predict failure

Counting what happened is not enough: slow failures matter more. The
metrics include the age of the oldest request, the connection state,
a slow-database gauge, replica lag, the reclaim backlog and write
amplification, so trouble shows before it becomes an outage.

---

<small>Generated from `spec.app.ObservableDaemon` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
