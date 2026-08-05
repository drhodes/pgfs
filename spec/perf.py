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
from .err import Req


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
