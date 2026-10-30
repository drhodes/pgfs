# Reference: On-Demand Profiling

!!! success "Status: Stable"
    The SIGUSR2 CPU profiler, the Chrome span trace and `scripts/bench_profile.sh` ship today behind the `profiling` Cargo feature. Default builds contain none of it.

> Operators can profile a running pgfs daemon without restarting it: SIGUSR2 starts and stops a CPU sampler that writes a flamegraph, and a span trace of every FUSE callback and database call is written on shutdown. All of it is compiled in only with the `profiling` feature.

[Tutorial](../tutorials/ondemandprofiling.md) · [How-to](../how-to/ondemandprofiling.md) · **Reference** · [Explanation](../explanation/ondemandprofiling.md)

---

## Build

| Item | Value |
|---|---|
| Cargo feature | `profiling` (enables the `pprof` and `tracing-chrome` crates) |
| Default | off |
| Build command | `cargo build --release --features profiling` |

## SIGUSR2 CPU profiler

| Property | Value |
|---|---|
| Toggle | first SIGUSR2 starts sampling, the next stops it |
| Sampling rate | 997 Hz |
| Excluded frames | `libc`, `libgcc`, `pthread`, `vdso` |
| Runs on | the `signal-waiter` thread, never in a signal handler |
| Output on stop | `/tmp/pgfs-profile-<unix-seconds>.svg` (inferno flamegraph) and `/tmp/pgfs-profile-<unix-seconds>.stacks` (text dump of every sampled stack) |
| Log lines | `CPU profiling started (997 Hz)`; `CPU profiling stopped; wrote <svg> and <stacks>` |
| Not produced | pprof protobuf (`.pb`); the codec is not enabled |
| Default build | logs `CPU profiling not compiled in — rebuild with --features profiling` |

## Span trace

| Property | Value |
|---|---|
| Installed | at startup, as a `tracing-chrome` layer next to the log layer |
| Captures | every span: FUSE callbacks and nested `db::` spans |
| Output | `/tmp/pgfs-trace-<unix-seconds>.json`, Chrome trace event format |
| Written | when the daemon exits cleanly; not written after SIGKILL or a crash |
| Path announced | `span trace will be written to <path> on shutdown` at INFO on startup |
| Viewers | Perfetto (<https://ui.perfetto.dev>), `chrome://tracing` |

## `scripts/bench_profile.sh`

| Item | Value |
|---|---|
| Binary | `target/release/pgfs`, built with `--features profiling` |
| Mount | `testdata/mnt` (runs `pgfs.sh down` first) |
| Daemon log | `testdata/bench-pgfs.log` |
| PID file | `testdata/bench.pid` |
| Phases | idle baseline (2 s), `write-ampl`, `seq-read`, `many-files` |
| Cleanup | SIGTERM to the daemon, then `fusermount3 -u` (lazy if busy) |


## Specification requirements

`OnDemandProfiling` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "CpuProfilingOnDemand"
    `spec.perf.CpuProfilingOnDemand`

    SIGUSR2 toggles a pprof CPU sampler running at 997 Hz. The toggle
    runs entirely on the signal-waiter thread; signal dispositions never
    do the work themselves. A second SIGUSR2 stops sampling and renders
    the report.

    Output contract — stopping writes two files under /tmp:

      - /tmp/pgfs-profile-{ts}.svg      — flamegraph (inferno SVG)
      - /tmp/pgfs-profile-{ts}.stacks   — per-stack sample dump
    The written path is logged at `info!`. The `.pb` protobuf format is
    deliberately not produced (the protobuf codec feature is not enabled);
    the SVG and stack dump carry the same information.

??? abstract "SpanTraceOnShutdown"
    `spec.perf.SpanTraceOnShutdown`

    When built with the `profiling` feature, a tracing-chrome layer is
    installed in the tracing subscriber at startup. It captures every FUSE
    callback span and nested `db::` span. On clean shutdown the trace is
    written to /tmp/pgfs-trace-{ts}.json (Chrome/Perfetto format). The
    target path is logged at `info!` at startup.

??? abstract "ZeroOverheadWhenDisabled"
    `spec.perf.ZeroOverheadWhenDisabled`

    All profiling code — the pprof guard, the chrome layer, and report
    rendering — is gated behind the `profiling` feature. A default build
    contains no profiling instructions; SIGUSR2 merely logs that profiling
    was not compiled in. `cargo check`, `cargo clippy`, and `cargo test`
    must all pass with and without the feature.

??? abstract "ProfileDrivenBenchmark"
    `spec.perf.ProfileDrivenBenchmark`

    scripts/bench_profile.sh drives the mounted filesystem through
    workload phases — sequential small-block writes (whole-blob
    read-modify-write amplification), sequential reads, and many-small-file
    creation (round-trip bound) — each wrapped in a SIGUSR2 start/stop pair.
    The bottleneck shows up as CPU time in the flamegraph rather than as
    wall-clock guesswork, and the span trace attributes FUSE vs DB time.
    This script is for finding where time goes. Comparable measurements
    over time are `BenchmarkSuite`'s job.

---

<small>Generated from `spec.perf.OnDemandProfiling` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
