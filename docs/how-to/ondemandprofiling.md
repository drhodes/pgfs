# How-To: On-Demand Profiling

!!! success "Status: Stable"
    The SIGUSR2 CPU profiler, the Chrome span trace and `scripts/bench_profile.sh` ship today behind the `profiling` Cargo feature. Default builds contain none of it.

> Operators can profile a running pgfs daemon without restarting it: SIGUSR2 starts and stops a CPU sampler that writes a flamegraph, and a span trace of every FUSE callback and database call is written on shutdown. All of it is compiled in only with the `profiling` feature.

[Tutorial](../tutorials/ondemandprofiling.md) · **How-to** · [Reference](../reference/ondemandprofiling.md) · [Explanation](../explanation/ondemandprofiling.md)

---

## Run the bundled benchmark under the profiler

```bash
./scripts/bench_profile.sh
```

The script builds a release binary with profiling, mounts it at
`testdata/mnt` (unmounting anything already there), and runs
three phases, each wrapped in its own SIGUSR2 start/stop:

| Phase | Workload |
|---|---|
| `write-ampl` | 16 MB appended in 4 KiB writes (`dd oflag=append`) |
| `seq-read` | sequential read of that file in 1 MiB reads |
| `many-files` | 2000 empty file creations |

It lists the profiles it wrote and the relevant log lines.

## Profile a production daemon

The daemon must be built with `--features profiling`. Then:

```bash
kill -USR2 <pid>      # start sampling
# ... let the workload run ...
kill -USR2 <pid>      # stop and write /tmp/pgfs-profile-<ts>.svg
```

On a default build, SIGUSR2 logs
`CPU profiling not compiled in — rebuild with --features profiling`
and does nothing else.

## Separate database time from FUSE time

Use the span trace rather than the flamegraph. A FUSE callback
that waits on PostgreSQL shows a long `db::` span inside it but
little CPU. Stop the daemon cleanly (SIGTERM, SIGINT or SIGHUP)
so the trace file is written; a killed daemon writes no trace.

## Compare two builds

Run the same workload against each build, take one profile per
run, and compare the `metrics|latency` lines in their logs. The
p50/p95/p99 figures are bucketed (see
[Observability](observabledaemon.md)); use the span trace for
exact durations.

## Check that profiling is compiled out

```bash
cargo build --release
nm -C target/release/pgfs | grep -c pprof     # prints 0
```

---

<small>Generated from `spec.perf.OnDemandProfiling` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
