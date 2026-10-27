"""
Performance-profiling requirements for pgfs.

These Requirement classes make "benchmark with a profiler" a first-class
workflow: the daemon must be able to sample its own CPU usage on demand,
render the sample into a readable flamegraph, and record a span trace of
FUSE callbacks → DB queries. They inherit the `Profiling` context from
observe.py, whose docstring is preserved as a reference; the docstrings
below pin down the concrete output contracts this codebase implements.
"""

from .observe import Profiling
from .err import STABLE, Feat, Req, md


class CpuProfilingOnDemand(Profiling, Req):
    """SIGUSR2 toggles a pprof CPU sampler running at 997 Hz. The toggle
    runs entirely on the signal-waiter thread; signal dispositions never
    do the work themselves. A second SIGUSR2 stops sampling and renders
    the report.

    Output contract — stopping writes two files under /tmp:
      - /tmp/pgfs-profile-{ts}.svg      — flamegraph (inferno SVG)
      - /tmp/pgfs-profile-{ts}.stacks   — per-stack sample dump
    The written path is logged at `info!`. The `.pb` protobuf format is
    deliberately not produced (the protobuf codec feature is not enabled);
    the SVG and stack dump carry the same information.
    """


class SpanTraceOnShutdown(Profiling, Req):
    """When built with the `profiling` feature, a tracing-chrome layer is
    installed in the tracing subscriber at startup. It captures every FUSE
    callback span and nested `db::` span. On clean shutdown the trace is
    written to /tmp/pgfs-trace-{ts}.json (Chrome/Perfetto format). The
    target path is logged at `info!` at startup."""


class ZeroOverheadWhenDisabled(Profiling, Req):
    """All profiling code — the pprof guard, the chrome layer, and report
    rendering — is gated behind the `profiling` feature. A default build
    contains no profiling instructions; SIGUSR2 merely logs that profiling
    was not compiled in. `cargo check`, `cargo clippy`, and `cargo test`
    must all pass with and without the feature."""


class ProfileDrivenBenchmark(Profiling, Req):
    """scripts/bench_profile.sh drives the mounted filesystem through
    workload phases — sequential small-block writes (whole-blob
    read-modify-write amplification), sequential reads, and many-small-file
    creation (round-trip bound) — each wrapped in a SIGUSR2 start/stop pair.
    The bottleneck shows up as CPU time in the flamegraph rather than as
    wall-clock guesswork, and the span trace attributes FUSE vs DB time."""


# ── User-visible feature ──────────────────────────────────────────────

class OnDemandProfiling(Feat):
    """Operators can profile a running pgfs daemon without restarting it:
    SIGUSR2 starts and stops a CPU sampler that writes a flamegraph, and a
    span trace of every FUSE callback and database call is written on
    shutdown. All of it is compiled in only with the `profiling` feature."""

    deps = [
        CpuProfilingOnDemand,
        SpanTraceOnShutdown,
        ZeroOverheadWhenDisabled,
        ProfileDrivenBenchmark,
    ]

    def title(self):
        return "On-Demand Profiling"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "The SIGUSR2 CPU profiler, the Chrome span trace and "
            "`scripts/bench_profile.sh` ship today behind the `profiling` "
            "Cargo feature. Default builds contain none of it."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will build pgfs with profiling compiled
            in, profile one workload, and open the resulting flamegraph and
            span trace. We will see where a burst of small writes spends
            its time.

            ## Step 1: build and mount a profiling binary

            ```bash
            ./scripts/pgfs.sh down
            cargo build --release --features profiling
            ./scripts/ensure_db.sh
            mkdir -p testdata/mnt
            ./target/release/pgfs testdata/mnt 2> testdata/profile-demo.log &
            sleep 1
            grep 'span trace will be written' testdata/profile-demo.log
            ```

            ```text
            INFO pgfs: span trace will be written to /tmp/pgfs-trace-1793059200.json on shutdown
            ```

            ## Step 2: profile a workload

            Start the sampler, run the workload, stop the sampler:

            ```bash
            PID=$(pgrep -f 'target/release/pgfs testdata/mnt')
            kill -USR2 $PID
            for i in $(seq 1 500); do echo "line $i" > testdata/mnt/p$i.txt; done
            kill -USR2 $PID
            sleep 1
            grep 'CPU profiling' testdata/profile-demo.log
            ```

            ```text
            INFO pgfs::profiling: CPU profiling started (997 Hz)
            INFO pgfs::profiling: CPU profiling stopped; wrote /tmp/pgfs-profile-1793059231.svg and /tmp/pgfs-profile-1793059231.stacks
            ```

            ## Step 3: open the flamegraph

            Open the `.svg` file in a web browser. Each box is a function;
            its width is the share of samples in which it was on the stack.
            Hover over the widest towers to find which FUSE callbacks
            (`create`, `write`, `flush`) and which `postgres` client calls
            they lead to. Click a box to zoom into it.

            ## Step 4: stop and open the span trace

            ```bash
            rm testdata/mnt/p*.txt
            kill -TERM $PID
            ls -l /tmp/pgfs-trace-*.json | tail -1
            ```

            Load the JSON file at <https://ui.perfetto.dev> (or
            `chrome://tracing`). Each FUSE callback is a bar, with the
            `db::` spans it caused nested underneath.

            ## What we did

            We profiled a live daemon on demand, without restarting it,
            and got two complementary views: where CPU time goes (the
            flamegraph) and where wall-clock time goes (the span trace).
            For a full scripted run, see the [how-to](../how-to/ondemandprofiling.md).
        """)

    def how_to(self):
        return md(r"""
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
        """)

    def reference(self):
        return md(r"""
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
        """)

    def explanation(self):
        return md(r"""
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
        """)
