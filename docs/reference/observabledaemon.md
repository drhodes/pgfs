# Reference: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing, the 60-second metrics, the SIGUSR1 state dump, error chains and crash reports all ship today. Of the per-errno counters, only `eio` and `enoent` are incremented yet.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

[Tutorial](../tutorials/observabledaemon.md) · [How-to](../how-to/observabledaemon.md) · **Reference** · [Explanation](../explanation/observabledaemon.md)

---

## Logging

| Setting | Values | Default |
|---|---|---|
| `RUST_LOG` | `tracing-subscriber` `EnvFilter` syntax | `pgfs=info` |
| `RUST_LOG_FORMAT` | `json` for newline-delimited JSON; anything else for text | text |
| Destination | stderr | |

## Log levels

| Level | Used for |
|---|---|
| ERROR | unexpected failures (with the full error chain), crash reports, deadlock detection |
| WARN | degraded but working: replica fallback, AutoUnmount unavailable, health file write failures |
| INFO | lifecycle (startup, mount, signals, shutdown), metrics, state dumps, profiling |
| DEBUG | FUSE callback and database spans, reads served from the primary |

Expected conditions (`ENOENT`, `EEXIST`, `ENOTEMPTY`, `EISDIR`,
`ENOTDIR`, `EINVAL`) are replied without logging.

## Spans

| Span | Level | Fields |
|---|---|---|
| `lookup`, `create`, `mkdir`, `unlink`, `rmdir` | INFO | `parent`, `name` |
| `getattr`, `open`, `readdir` | INFO | `ino` (`readdir`: + `offset`) |
| `read`, `write` | INFO | `ino`, `fh`, `offset`, `size`/`len` |
| `setattr` | INFO | `ino`, `size` |
| `fsync` | INFO | `fh` |
| `rename` | INFO | `parent`, `name`, `newparent`, `newname`, `flags` |
| `db::getattr`, `db::list`, `db::create`, `db::mkdir`, `db::unlink`, `db::rmdir` | DEBUG | `parent`, `name` |
| `db::read_range`, `db::write_range` | DEBUG | `parent`, `name`, `offset`, `len` |
| `db::truncate` | DEBUG | `parent`, `name`, `new_size` |
| `db::write_blocks_batch` | DEBUG | `parent`, `name`, `count`, `new_size` |
| `db::rename` | DEBUG | `old_parent`, `old_name`, `new_parent`, `new_name` |

## Periodic metrics

Logged at INFO every 60 s by the `metrics-export` thread:

```text
metrics|ops|lookup=N getattr=N setattr=N read=N write=N create=N mkdir=N unlink=N rmdir=N rename=N readdir=N open=N fsync=N
metrics|errors|eio=N enoent=N eexist=N enotempty=N eisdir=N enotdir=N einval=N
metrics|replica|reads=N fallbacks=N
metrics|latency|fuse=fuse total=N p50=<=Xµs p95=<=Xµs p99=<=Xµs
metrics|latency|db=db total=N p50=<=Xµs p95=<=Xµs p99=<=Xµs
```

| Counter | Incremented when |
|---|---|
| `ops` fields | the corresponding FUSE callback runs (`flush` and `release` are not counted) |
| `eio` | an unexpected failure is logged and replied as `EIO` |
| `enoent` | `lookup` cannot resolve its parent inode or name |
| `eexist`, `enotempty`, `eisdir`, `enotdir`, `einval` | declared; not yet incremented |
| `replica reads` | a `getattr`, `lookup` or `readdir` is served by the standby |
| `replica fallbacks` | a read goes to the primary while a standby is configured |

All counters are totals since startup.

## Latency histograms

| Histogram | Records |
|---|---|
| `fuse` | wall-clock time of every instrumented FUSE callback |
| `db` | wall-clock time of every `Db` method |

Buckets: `<=10`, `<=100`, `<=1k`, `<=10k`, `<=100k`, `<=1M`,
`>1M` microseconds. Each percentile is reported as the upper
bound of the bucket that contains it.

## State dump (SIGUSR1)

```text
state|ino_cache=N next_ino=N db=connected (replica none|fresh|stale or unreachable) uptime_secs=N
state|fuse_latency=fuse total=N p50=… p95=… p99=…
state|db_latency=db total=N p50=… p95=… p99=…
```

| Field | Meaning |
|---|---|
| `ino_cache` | paths with an assigned inode number |
| `next_ino` | next inode number to be assigned |
| `replica` | `none` without `--replica`; otherwise the cached freshness verdict |
| `uptime_secs` | seconds since the mount was created |

A final dump is logged on clean shutdown.

## Error chain format

Unexpected failures are logged at ERROR as one line, the
`anyhow` alternate (`{:#}`) rendering of the chain, inside the
span of the FUSE callback that failed:

```text
ERROR <callback>{<span fields>}: pgfs::fs: <what was attempted> (at src/<file>.rs:<line>): <deeper context> (at …): <root cause>
```

Example:

```text
ERROR lookup{parent=1 name=after-stop}: pgfs::fs: get attributes of entry "after-stop" in "" (at src/db.rs:453): db error: FATAL: terminating connection due to administrator command
```

Startup failures are printed to stderr in the same form,
prefixed `pgfs: `, and the process exits with status 1.

## Crash report

Written to stderr and logged at ERROR before the process
unwinds or aborts:

```text
panic at <file>:<line>: <message>
pgfs v<version> on <os>/<arch>, built with <rustc version>, RUST_BACKTRACE=<value>
Backtrace:
<backtrace>
```

## Background threads

| Thread | Role |
|---|---|
| `signal-waiter` | receives SIGINT, SIGTERM, SIGHUP, SIGUSR1, SIGUSR2 |
| `metrics-export` | logs metrics every 60 s |
| `liveness` | heartbeat every 10 s |
| `deadlock-watchdog` | unmounts when a stall is detected |


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

    Every `Filesystem` callback (`lookup`, `getattr`, `read`, `write`,
    `create`, `mkdir`, `unlink`, `rmdir`, `rename`, `readdir`, `setattr`,
    `open`, `flush`, `fsync`, `copy_file_range`, `fallocate`) opens a
    `tracing::info_span!` recording `ino` and `name` (where applicable).
    The span is entered for the duration of the callback so nested DB
    operations are automatically attributed.

??? abstract "InstrumentsDbOperations"
    `spec.app.InstrumentsDbOperations`

    Every `Db` method (`getattr`, `read`, `write`, `create`, `mkdir`,
    `unlink`, `rmdir`, `rename`, `list`) opens a `tracing::debug_span!`
    recording the (parent, name) key. The span is entered for the
    duration of the database round-trip.

??? abstract "DistinguishesExpectedErrors"
    `spec.app.DistinguishesExpectedErrors`

    Expected conditions (ENOENT, EEXIST, ENOTEMPTY, EISDIR, ENOTDIR,
    EINVAL) are replied directly to the kernel without logging.
    Unexpected failures (database errors, invariant violations) are
    logged in full via `log_and_reply!` with the complete error chain
    and replied as EIO.

??? abstract "ExportsPeriodicMetrics"
    `spec.app.ExportsPeriodicMetrics`

    A background thread logs five INFO lines every 60 s:
    `metrics|ops|…` (per-callback counters), `metrics|errors|…` (EIO and
    per-errno counters), `metrics|replica|reads=… fallbacks=…`,
    `metrics|latency|fuse=…` and `metrics|latency|db=…`. Counters are
    lifetime totals and never reset. Latency histograms bucket wall-clock
    durations at ≤10µs, ≤100µs, ≤1ms, ≤10ms, ≤100ms, ≤1s and >1s and
    report p50/p95/p99 as the upper bound of the bucket that contains the
    percentile.

??? abstract "DumpsStateOnSigusr1"
    `spec.app.DumpsStateOnSigusr1`

    SIGUSR1 sets a flag on the signal-waiter thread. The next FUSE
    callback clears it and logs, at INFO, `state|ino_cache=… next_ino=…
    db=connected (replica none|fresh|stale or unreachable) uptime_secs=…`
    followed by the FUSE and DB latency summaries. The dump never runs on
    the signal thread and never blocks dispatch; on an idle mount it
    appears with the next filesystem access.

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
