# Tutorial: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing with spans per callback (worker and view included) and per database call (statement, inode, backend pid, round trip), the nine 60-second metrics lines with every errno counted where it is replied, the SIGUSR1 dump written by the signal thread from atomics, and crash reports all ship. A test keeps the instrumented callback list in step with the `Filesystem` impl.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

**Tutorial** · [How-to](../how-to/observabledaemon.md) · [Reference](../reference/observabledaemon.md) · [Explanation](../explanation/observabledaemon.md)

---

In this tutorial we will read the daemon's log, metrics and state dump
while it works. We need a mount started with `./scripts/pgfs.sh up`.

## Step 1: watch spans

```bash
pkill -f 'pgfs mount main' ; RUST_LOG=pgfs=debug ./target/debug/pgfs mount main testdata/mnt 2> testdata/obs.log &
sleep 1; ls testdata/mnt > /dev/null
grep 'fuse{op="getattr"' testdata/obs.log | head -2
```

Every callback runs in a `fuse` span with its operation, inode, worker
and view; database calls nest inside it as `db` spans with the
statement, backend pid and round-trip time.

## Step 2: ask for a state dump

```bash
kill -USR1 "$(pgrep -f 'pgfs mount main testdata/mnt')"
grep 'state|' testdata/obs.log | tail -12
```

The signal thread writes the dump itself, so it appears even when
every worker is stuck.

## Step 3: read the metrics

Every 60 s the daemon logs nine `metrics|…` lines: operations, errnos,
replica, FUSE latency, database latency, connection state, in-flight
age, space and integrity.

---

<small>Generated from `spec.app.ObservableDaemon` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
