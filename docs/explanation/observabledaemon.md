# Explanation: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing, the 60-second metrics, the SIGUSR1 state dump, error chains and crash reports all ship today. Of the per-errno counters, only `eio` and `enoent` are incremented yet.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

[Tutorial](../tutorials/observabledaemon.md) · [How-to](../how-to/observabledaemon.md) · [Reference](../reference/observabledaemon.md) · **Explanation**

---

## The kernel only hears an errno

When something fails inside a FUSE callback, the kernel can be
told exactly one thing: an error number. `EIO` carries no
message, no location and no cause. The process that called
`read` learns that something went wrong and nothing else.
Everything an operator needs to fix the problem therefore has
to be in the daemon's own log, and pgfs is built around that.

## Errors that tell a story

Every fallible step in pgfs wraps its error with what it was
trying to do and where, using `#[track_caller]` so the location
is the caller's line, not the helper's. As an error travels up
from the database driver through the storage layer to the FUSE
callback, it accumulates a chain: "lookup during readdir, at
this line, because fetching attributes, at that line, failed
because the connection closed". The rule is that an operator
should never need a debugger or the source code to understand a
failure.

The other half of the rule is to not log what is normal. A
shell probing for a file that does not exist is not an error,
and logging every `ENOENT` would bury real failures. Expected
conditions are answered with their errno and nothing else.
Only unexpected failures are logged, always at ERROR and always
with the full chain.

## Spans instead of log lines

pgfs uses the `tracing` crate rather than plain logging. Each
FUSE callback opens a span, and each database call opens a span
inside it. Any message emitted underneath carries the full
context, and timing tools can attribute time to "FUSE"
versus "database". JSON output keeps the same structure for
machines.

## Metrics that are cheap enough to keep on

The counters are atomic integers and the histograms are seven
atomic buckets, so recording costs a few nanoseconds and is
always on. Fixed buckets and percentiles approximated to a
bucket bound are coarse, but they answer the questions that
matter in operation: is it a 100 µs filesystem or a 10 ms one,
and is it getting worse. Separating `eio` from expected
errnos lets an operator tell a spike in "file not found" from a
spike in "database down". Exporting through the log keeps the
first iteration free of extra endpoints; the counters are plain
statics, so an exporter can be added without changing the
instrumentation.

## Asking a running daemon

SIGUSR1 requests a snapshot without restarting anything. Signal
handlers are severely restricted in what they may do, so the
signal only sets a flag. The next FUSE callback, which runs in
a normal context, writes the dump. On an idle mount that means
the dump waits for the next access, a deliberate trade for
never doing real work inside signal handling.

## Crash reports

A panic report is written to both stderr and the log, because
either may be the one an operator actually has. It includes the
version, platform and compiler, so it can be pasted into a bug
report as is.

---

<small>Generated from `spec.app.ObservableDaemon` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
