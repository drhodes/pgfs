# Reference: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing with spans per callback (worker and view included) and per database call (statement, inode, backend pid, round trip), the nine 60-second metrics lines with every errno counted where it is replied, the SIGUSR1 dump written by the signal thread from atomics, and crash reports all ship. A test keeps the instrumented callback list in step with the `Filesystem` impl.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

[Tutorial](../tutorials/observabledaemon.md) · [How-to](../how-to/observabledaemon.md) · **Reference** · [Explanation](../explanation/observabledaemon.md)

---

## Log levels

| Level | Used for |
|---|---|
| ERROR | unexpected failures with their full chain, fencing, crash reports, checksum mismatches |
| WARN | degraded but working: retries, timeouts with their thresholds, slow database, standby fallbacks |
| INFO | lifecycle, state transitions, metrics, state dumps |
| DEBUG | expected errnos, per-operation detail |

## Metrics lines (every 60 s)

| Line | Fields |
|---|---|
| `metrics\|ops\|…` | one counter per callback |
| `metrics\|errors\|…` | one counter per errno in the catalogue |
| `metrics\|replica\|…` | reads, fallbacks, lag_bytes, lag_ms, ticket_waits |
| `metrics\|latency\|fuse=…` | p50 / p95 / p99 of callbacks |
| `metrics\|latency\|db=…` | p50 / p95 / p99 of database calls |
| `metrics\|db\|…` | state, reconnects, fenced, slow_db |
| `metrics\|inflight\|oldest_ms=…` | age of the oldest in-flight request |
| `metrics\|space\|…` | live, physical, inline and snapshot-only bytes; backlog; dead tuples; WAL per user byte |
| `metrics\|integrity\|checksum_failures=…` | checksum failures |

## State dump (SIGUSR1)

`state|mount=… view=… fence=… layer=… db=… uptime_secs=…`, one
`state|worker=N` line per worker (operation, inode, age, backend pid),
`state|cache=… handles=… buffered_bytes=…`, the replica line, and the
latency summaries. Any lock that cannot be taken at once prints
`locked`.

## Threads

| Thread | Role |
|---|---|
| `worker-N` | dispatch workers, one database connection each |
| `renewer` | lease renewal and self-fencing |
| `reconnector` | reconnect and revalidation |
| `housekeeping` | usage folding, reclaim, GC, change pruning and notifications, auto-flatten |
| `control` | owner-drained snapshots and branches |
| `watchdog` | in-flight ages, stall warnings and cancels, systemd watchdog |
| `signal-waiter` | SIGINT/SIGTERM/SIGHUP shutdown, SIGUSR1 dump, SIGUSR2 profiling |
| `metrics-export` | the 60 s metrics lines |


## Specification requirements

`ObservableDaemon` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "UsesStructuredTracing"
    `spec.app.UsesStructuredTracing`

    The daemon uses the `tracing` crate (not bare `log`/`env_logger`)
    for all diagnostic output. The `tracing-subscriber` subscriber is
    initialized before any other work. `RUST_LOG` controls filter levels
    with the same syntax as `env_logger`; `RUST_LOG_FORMAT=json` switches
    to machine-parseable JSON output.

??? abstract "InstrumentsFuseCallbacks"
    `spec.app.InstrumentsFuseCallbacks`

    Every `Filesystem` callback the daemon implements opens a
    `tracing::info_span!` recording `ino`, `name` where there is one,
    the worker, and the view. The span is entered for the duration of
    the callback, so nested database operations are attributed to it.
    The callbacks are those listed in `posix.PosixCompatibility`, plus
    `init`, `destroy`, `forget`, `batch_forget`, `statfs`, `interrupt`
    and the open and release calls. The list is checked by a test
    against the `Filesystem` impl, so it cannot drift from the code.

??? abstract "InstrumentsDbOperations"
    `spec.app.InstrumentsDbOperations`

    Every database call opens a `tracing::debug_span!` recording the
    `pgfs` function or prepared statement name, the `ino` it concerns,
    the backend pid, and the round-trip time. A slow span can therefore
    be matched to its row in `pg_stat_activity` (Fail-Slow at Scale
    r122).

??? abstract "DistinguishesExpectedErrors"
    `spec.app.DistinguishesExpectedErrors`

    Expected conditions reply their errno to the kernel and are logged
    at DEBUG at most. These are the errnos in `posix.ErrnoCatalogue`
    other than EIO. Unexpected failures are logged in full through
    `log_and_reply!`: database errors that map to EIO, checksum
    failures, invariant violations and fencing. Database errors are
    classified by SQLSTATE in one table (`failure.MapsDatabaseErrors`),
    never by matching message text.

??? abstract "ExportsPeriodicMetrics"
    `spec.app.ExportsPeriodicMetrics`

    A background thread logs nine INFO lines every 60 s:
    `metrics|ops|…` (per-callback counters), `metrics|errors|…` (EIO and
    per-errno counters),
    `metrics|replica|reads=… fallbacks=… lag_bytes=… lag_ms=… ticket_waits=…`
    (`replica.ReplicaObservability`), `metrics|latency|fuse=…` and
    `metrics|latency|db=…`. Counters are
    lifetime totals and never reset. Latency histograms bucket wall-clock
    durations at ≤10µs, ≤100µs, ≤1ms, ≤10ms, ≤100ms, ≤1s and >1s and
    report p50/p95/p99 as the upper bound of the bucket that contains the
    percentile.

    Further lines report:

    - `metrics|db|state=… reconnects=… fenced=… slow_db=…`;
    - `metrics|inflight|oldest_ms=…`;
    - `metrics|space|…` (`space.ReportsSpace`);
    - `metrics|integrity|checksum_failures=…`.

    Every per-errno counter is incremented where its errno is replied,
    or removed. None stays at zero by omission.

??? abstract "DumpsStateOnSigusr1"
    `spec.app.DumpsStateOnSigusr1`

    SIGUSR1 makes the signal thread itself write a state dump at INFO.
    It does not wait for the next callback, so the dump appears even
    when every worker is stuck. The dump reads only atomics and
    `try_lock` snapshots, and prints `locked` for any state it cannot
    take at once. It contains:

    - the mount, view, fence and connection states;
    - the in-flight request of each worker, with its age and backend
      pid;
    - cache, handle and buffer sizes;
    - the replica state;
    - the latency summaries.

    **Sources.** Gray Failure r124. The first design ran the dump on the
    next callback, which on a stuck mount never comes.

??? abstract "WritesHealthFiles"
    `spec.app.WritesHealthFiles`

    After connecting to Postgres the daemon writes its PID to
    `/tmp/pgfs-<hash>.pid`, prints `health pid file: <path>` on stdout and
    logs it. After the FUSE session is created it writes
    `/tmp/pgfs-<hash>.ready` containing `ready`. `<hash>` is 16 hex digits
    derived from the mountpoint argument exactly as given. Both files are
    removed when `run()` returns.

??? abstract "ReportsCrashes"
    `spec.app.ReportsCrashes`

    If the daemon panics, the panic hook emits a crash report to both
    stderr and the log channel. The report includes the panic message,
    file:line location, a symbolized backtrace, and OS/Rust metadata.
    In release builds, `panic = "abort"` prevents a broken daemon from
    lingering.

---

<small>Generated from `spec.app.ObservableDaemon` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
