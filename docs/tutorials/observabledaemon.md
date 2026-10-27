# Tutorial: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing, the 60-second metrics, the SIGUSR1 state dump, error chains and crash reports all ship today. Of the per-errno counters, only `eio` and `enoent` are incremented yet.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

**Tutorial** · [How-to](../how-to/observabledaemon.md) · [Reference](../reference/observabledaemon.md) · [Explanation](../explanation/observabledaemon.md)

---

In this tutorial we will watch pgfs work: follow its log, ask a
running daemon for a state dump, read a metrics report, and
then break the database on purpose to see what a real failure
looks like. We need a terminal for the daemon and a second one
for commands.

## Step 1: run the daemon in the foreground

In the first terminal:

```bash
./scripts/pgfs.sh run
```

The startup lines show the replica mode, the health file and
the negotiated FUSE capability:

```text
INFO pgfs: replica mode: off (single primary)
health pid file: /tmp/pgfs-5049eced64c2821d.pid
INFO init: pgfs::fs: FUSE init: negotiated FUSE_BIG_WRITES
```

## Step 2: see that normal misses are quiet

In the second terminal:

```bash
cat testdata/mnt/does-not-exist
```

`cat` reports "No such file or directory", and the daemon's log
stays silent. A missing file is an expected answer, not a
failure, so pgfs does not log it.

## Step 3: ask for a state dump

```bash
kill -USR1 "$(pgrep -f 'target/debug/pgfs')"
ls testdata/mnt > /dev/null
```

The dump is written by the next filesystem operation, here the
`ls`:

```text
INFO pgfs: SIGUSR1 received; state dump will appear on next FUSE callback
INFO getattr{ino=1}: pgfs::fs: state|ino_cache=3 next_ino=4 db=connected (replica none) uptime_secs=41
state|fuse_latency=fuse total=12 p50=<=1kµs p95=<=10kµs p99=<=10kµs
state|db_latency=db total=15 p50=<=1kµs p95=<=1kµs p99=<=10kµs
```

## Step 4: read a metrics report

Do some work, then wait for the minute to turn over:

```bash
for i in $(seq 1 50); do echo $i > testdata/mnt/m$i; done
rm testdata/mnt/m*
```

Within 60 seconds, five `metrics|` lines appear. The
`metrics|ops` line counts the 50 creates and 50 unlinks among
the other operations.

## Step 5: break the database

Stop PostgreSQL underneath the running mount, then touch a name
the kernel has not cached:

```bash
make db-stop
ls testdata/mnt/after-stop
```

```text
ls: cannot access 'testdata/mnt/after-stop': Input/output error
```

This time the first terminal shows an ERROR. Read it left to
right: the FUSE operation and its arguments (the span), what
pgfs was doing and where (`src/db.rs:453`), and the root cause
reported by PostgreSQL:

```text
ERROR lookup{parent=1 name=after-stop}: pgfs::fs: get attributes of entry "after-stop" in "" (at src/db.rs:453): db error: FATAL: terminating connection due to administrator command
```

## Step 6: recover

Start PostgreSQL again, then restart the daemon. pgfs holds a
single database connection for its whole lifetime and does not
reconnect, so it needs the restart:

```bash
./scripts/init_db.sh
```

Press Ctrl+C in the first terminal. The daemon logs
`received signal 2; unmounting cleanly` and exits. Run
`./scripts/pgfs.sh run` again, and `ls testdata/mnt` works.

## What we did

We saw that expected misses stay out of the log, pulled live
state out of a running daemon, read its periodic metrics, and
followed a real failure from the kernel's bare `EIO` to its
root cause. The [reference](../reference/observabledaemon.md)
lists every field; the [explanation](../explanation/observabledaemon.md)
covers the design.

---

<small>Generated from `spec.app.ObservableDaemon` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
