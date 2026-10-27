# Explanation: On-Demand Profiling

!!! success "Status: Stable"
    The SIGUSR2 CPU profiler, the Chrome span trace and `scripts/bench_profile.sh` ship today behind the `profiling` Cargo feature. Default builds contain none of it.

> Operators can profile a running pgfs daemon without restarting it: SIGUSR2 starts and stops a CPU sampler that writes a flamegraph, and a span trace of every FUSE callback and database call is written on shutdown. All of it is compiled in only with the `profiling` feature.

[Tutorial](../tutorials/ondemandprofiling.md) · [How-to](../how-to/ondemandprofiling.md) · [Reference](../reference/ondemandprofiling.md) · **Explanation**

---

## Measuring instead of guessing

pgfs sits between two systems that each have their own costs:
the kernel's FUSE transport and PostgreSQL. When an operation
is slow, the cause could be in either, or in the daemon in
between, and intuition about which is unreliable. That is why
profiling is a first-class, specified capability rather than an
ad hoc `perf record`.

## Two views of the same run

A sampling CPU profiler answers "where does the processor
spend its time". In a database-backed filesystem much of the
time is spent waiting on a socket for PostgreSQL, which barely
shows up in a CPU profile. The span trace answers the other
question, "where does the wall-clock time go", because every
FUSE callback and every database call is a timed span with its
parent recorded. Together they distinguish "the daemon is
busy", "the daemon is waiting on the database" and "the
kernel is sending too many small requests".

## On demand, on a running daemon

Restarting a daemon to profile it changes the thing being
measured: caches are cold and the workload is interrupted.
SIGUSR2 lets an operator bracket exactly the interval of
interest on a live process. The signal only flips state on the
dedicated signal thread, and the profiler is started and
stopped there, so profiling never runs inside a signal
handler and never allocates on the FUSE dispatch path.

## Zero cost when off

Sampling profilers install timers and signal handlers, and
span recorders buffer every event. Neither belongs in a default
build. Gating both behind a Cargo feature means a normal build
contains no profiling instructions at all, not merely disabled
ones. Metrics and tracing spans stay on in every build because
they are cheap enough to leave running; see
[Observability](../explanation/observabledaemon.md).

## What the benchmark phases are for

Each phase of `bench_profile.sh` isolates one cost. Appending in
4 KiB writes exposes per-write overhead. A sequential read
exposes per-block fetch cost. Creating thousands of empty
files exposes per-operation round-trips with no data at all.
A regression shows up in the phase whose cost it affects, and
the matching flamegraph shows why.

---

<small>Generated from `spec.perf.OnDemandProfiling` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
