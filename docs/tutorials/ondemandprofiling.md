# Tutorial: On-Demand Profiling

!!! success "Status: Stable"
    The SIGUSR2 CPU profiler, the Chrome span trace and `scripts/bench_profile.sh` ship today behind the `profiling` Cargo feature. Default builds contain none of it.

> Operators can profile a running pgfs daemon without restarting it: SIGUSR2 starts and stops a CPU sampler that writes a flamegraph, and a span trace of every FUSE callback and database call is written on shutdown. All of it is compiled in only with the `profiling` feature.

**Tutorial** · [How-to](../how-to/ondemandprofiling.md) · [Reference](../reference/ondemandprofiling.md) · [Explanation](../explanation/ondemandprofiling.md)

---

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
./target/release/pgfs mount main testdata/mnt 2> testdata/profile-demo.log &
sleep 1
grep 'span trace will be written' testdata/profile-demo.log
```

```text
pgfs: span trace will be written to /tmp/pgfs-trace-1793059200.json on shutdown
```

## Step 2: profile a workload

Start the sampler, run the workload, stop the sampler:

```bash
PID=$(pgrep -f 'target/release/pgfs mount main testdata/mnt')
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
(`create`, `write`, `flush`) and which `tokio-postgres` client calls
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

---

<small>Generated from `spec.perf.OnDemandProfiling` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
