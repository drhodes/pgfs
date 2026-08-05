"""
Observability, debugging, and profiling contexts for a Rust FUSE daemon.

This module defines what "software excellence" means for pgfs in terms of
runtime visibility: the program must be transparent about what it is doing
right now, what it did recently, where time is spent, and why it failed.

These Ctx classes are inherited by Feature and Requirement specs alongside
the error-handling contexts in `err.py`.
"""

from libspec import Ctx


# ── Structured observability ─────────────────────────────────────────

class Tracing(Ctx):
    """
    Every significant operation produces a structured span with timestamps.

    1. **`tracing` over `log`.** The `tracing` crate replaces bare
       `log`/`env_logger`. Spans carry parent-child relationships,
       timings, and key-value fields; the subscriber renders them as
       human-readable text or machine-parseable JSON depending on
       `RUST_LOG_FORMAT`.

    2. **Span per FUSE callback.** Every `Filesystem` method
       (`lookup`, `read`, `write`, `mkdir`, `rename`, ...) opens a
       `tracing::info_span!` recording at minimum:
       - `ino` — the subject inode (or parent for create/lookup)
       - `name` — the entry name (redacted if sensitive; not applicable
         here)
       The span is entered for the duration of the callback so all
       nested DB queries and error paths are automatically attributed.

    3. **Span per DB operation.** `Db::getattr`, `Db::read`, `Db::write`,
       `Db::rename`, etc. each open a `tracing::debug_span!` recording
       the query intent and the (parent, name) key.

    4. **JSON output for machines.** Setting `RUST_LOG_FORMAT=json`
       emits newline-delimited JSON objects with `timestamp`, `level`,
       `target`, `fields`, `spans` — ingestible by `jq`, Elasticsearch,
       or any log aggregator. The `tracing-subscriber` crate provides
       this layer.

    5. **Filtering.** `RUST_LOG=pgfs=info` controls verbosity at the
       crate level. `RUST_LOG=pgfs::fs=trace,pgfs::db=debug` targets
       specific modules. The subscriber respects the same `RUST_LOG`
       syntax as `env_logger`.
    """


class Metrics(Ctx):
    """
    Numeric counters and histograms for operational visibility.

    1. **FUSE operation counters.** A per-callback counter
       (`lookup_count`, `read_count`, `write_count`, `mkdir_count`,
       `rmdir_count`, `rename_count`, ...) incremented atomically so
       operators can monitor workload composition.

    2. **Latency histograms.** Each FUSE callback records its wall-clock
       duration into a histogram with buckets at 10µs, 100µs, 1ms,
       10ms, 100ms, 1s. Export percentiles (p50, p95, p99) on demand.

    3. **DB query latency.** `db::getattr`, `db::read`, `db::write`, and
       `db::rename` record their Postgres round-trip time into the same
       histogram structure.

    4. **Error counters.** Counters for `log_and_reply!` invocations
       (unexpected errors → EIO) and per-errno expected-error counts
       (ENOENT, ENOTEMPTY, EEXIST, ...) so operators can distinguish a
       spike in "file not found" from a spike in "database failure".

    5. **Export.** At minimum, metrics are logged periodically (every
       60s at `info!` level). A future `SIGUSR1` handler can dump them
       on demand. No external metrics endpoint (Prometheus, statsd) is
       required in the first iteration — but the counters and histograms
       must be structured so an exporter can be added without rewriting
       the instrumentation.
    """


# ── Debugging & crash forensics ──────────────────────────────────────

class CrashReports(Ctx):
    """
    When the daemon panics, the operator must receive a self-contained
    forensic report suitable for a bug report.

    1. **`human-panic` or equivalent.** The panic hook emits:
       - The panic message and location (file:line)
       - A symbolized backtrace (requires `RUST_BACKTRACE=full` or
         debug symbols)
       - OS/environment metadata (Rust version, OS, pgfs version)
       - A pointer to the log file for the full error chain

    2. **Double-write.** The crash report goes to stderr AND to the
       `log::error!` channel so it appears in both the terminal and
       any log file.

    3. **Abort on panic in release.** In `--release`, `panic = "abort"`
       in `Cargo.toml` profile so a panicked daemon can never linger
       in a broken state. The crash report is written before abort.
    """


class StateIntrospection(Ctx):
    """
    Operators must be able to inspect live daemon state without a
    debugger.

    1. **SIGUSR1 state dump.** On receiving SIGUSR1, the daemon logs
       at `info!` level:
       - Inode cache size (`ino_by_path.len()`, `path_by_ino.len()`)
       - Next inode number
       - DB connection status (connected, idle, error)
       - Uptime since mount
       - Metrics snapshot (counters + histogram summaries per Metrics
         spec)

    2. **SIGUSR2 profiling toggle.** On receiving SIGUSR2, the daemon
       starts or stops CPU profiling (see `Profiling` Ctx). The toggle
       event is logged at `info!`.

    3. **Non-blocking.** Signal processing must not block the FUSE
       dispatch. SIGUSR1 and SIGUSR2 are added to the blocked signal set
       alongside the existing SIGINT/SIGTERM/SIGHUP; the dedicated
       `sigwait` thread (already in `main.rs`) dispatches them by
       setting `AtomicBool` flags or pushing messages into a lock-free
       queue, serviced on the next event loop iteration.
    """


# ── Profiling ────────────────────────────────────────────────────────

class Profiling(Ctx):
    """
    On-demand CPU and span profiling for identifying bottlenecks.

    1. **CPU profiling via `pprof`.** Behind a `profiling` feature flag,
       the `pprof` crate provides a SIGUSR2-activated CPU profiler.
       Calling `pprof::ProfilerGuardBuilder::default().frequency(997)`
       starts sampling; a second SIGUSR2 stops it and renders the report
       to `/tmp/pgfs-profile-{timestamp}.svg` (inferno flamegraph) plus
       `/tmp/pgfs-profile-{timestamp}.stacks` (per-stack sample dump) —
       the `.pb` protobuf codec is deliberately not enabled. The paths
       are logged at `info!`.

    2. **Span flamegraphs via `tracing-chrome`.** Behind the same
       feature flag, a `tracing_chrome::ChromeLayer` writes a Chrome
       trace file to `/tmp/pgfs-trace-{timestamp}.json` on shutdown
       or signal. This visualizes the span tree (FUSE callback → DB
       query) with nanosecond-precision timestamps. Open in
       `chrome://tracing` or Perfetto.

    3. **Zero overhead when disabled.** The profiling feature flag gates
       all profiling code. When disabled (`--no-default-features` or
       default-off), profiling adds zero instructions to the hot path.
       Metrics and tracing spans remain active regardless (they are
       cheap enough for production).

    4. **Profiling is safe for production.** The profiling signal
       handlers use only signal-safe operations to set a flag; the
       actual profiler guard is created/dropped on a background thread.
       Profiling never allocates in the FUSE dispatch path.
    """


# ── Logging conventions ──────────────────────────────────────────────

class LogLevels(Ctx):
    """
    Every log message must be at the correct level so operators can
    filter without losing signal.

    | Level   | When                                              |
    |---------|---------------------------------------------------|
    | `ERROR` | Unexpected failure requiring human attention     |
    |         | (DB connection lost, invariant violated, panic)  |
    | `WARN`  | Degraded but recoverable (retry succeeded,       |
    |         | slow query above threshold, resource near limit) |
    | `INFO`  | Lifecycle events (mount, unmount, signal          |
    |         | received, periodic metrics dump)                 |
    | `DEBUG` | Per-operation detail (FUSE callback entry/exit,   |
    |         | DB query, inode allocation)                      |
    | `TRACE` | Extremely verbose (byte contents, internal state)|

    - Expected conditions (ENOENT, EEXIST) are NEVER logged — they are
      normal operation.
    - `ERROR` messages must include enough context that the operator
      does not need to reproduce the failure to understand it.
    - `WARN` messages must include the threshold that was exceeded
      (e.g., "query took 1.2s, threshold 200ms").
    """


class HealthEndpoint(Ctx):
    """
    The daemon must expose a lightweight health signal for supervisors.

    1. **PID file.** On successful mount, write the PID to
       `/tmp/pgfs-{mountpoint-hash}.pid`. Remove it on clean unmount.
       The file path is printed to stdout and logged at `info!`.

    2. **Liveness check.** A background thread touches a shared
       `AtomicU64` counter every N seconds. Each FUSE callback
       (`lookup`, `read`, `write`, ...) observes the counter on entry;
       if the counter has not advanced within 3×N seconds, the daemon
       logs an `ERROR` and initiates a clean unmount. This catches
       deadlock in the FUSE session.

    3. **Readiness file.** A flag file
       `/tmp/pgfs-{mountpoint-hash}.ready` is created once the mount
       session is set up, immediately before the FUSE dispatch loop
       begins. Supervisors (systemd, launch scripts) poll for this file
       before declaring the mount ready.
    """
