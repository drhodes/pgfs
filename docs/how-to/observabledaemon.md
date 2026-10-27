# How-To: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing, the 60-second metrics, the SIGUSR1 state dump, error chains and crash reports all ship today. Of the per-errno counters, only `eio` and `enoent` are incremented yet.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

[Tutorial](../tutorials/observabledaemon.md) · **How-to** · [Reference](../reference/observabledaemon.md) · [Explanation](../explanation/observabledaemon.md)

---

## Change the log level

```bash
RUST_LOG=pgfs=debug ./scripts/pgfs.sh run              # every DB query
RUST_LOG=pgfs::fs=debug,pgfs::db=info ./scripts/pgfs.sh run
```

The default is `pgfs=info`. The level is read at startup, so
changing it requires a restart.

## Get JSON logs for a log pipeline

```bash
RUST_LOG_FORMAT=json ./target/debug/pgfs testdata/mnt 2> pgfs.jsonl
jq -c 'select(.level == "ERROR")' pgfs.jsonl
```

## Find out why an operation returned "Input/output error"

```bash
grep ' ERROR ' testdata/pgfs.log | tail -5
```

Read each line left to right: the FUSE span that failed, then
each layer's context with its `file:line`, then the root cause
after the last `: `.

## Recover from "Input/output error" on every operation

If every access fails and the log shows connection errors, the
daemon's database connection is gone. pgfs does not reconnect.
Make sure PostgreSQL is running, then restart the daemon:

```bash
./scripts/ensure_db.sh
./scripts/pgfs.sh down && ./scripts/pgfs.sh up
```

## Take a snapshot of a running daemon

```bash
kill -USR1 <pid>
ls <mountpoint> > /dev/null     # any access triggers the dump
```

## Extract metrics from the log

```bash
grep -o 'metrics|ops|.*' testdata/pgfs.log | tail -1
grep -o 'metrics|latency|.*' testdata/pgfs.log | tail -2
```

## Check that the daemon is alive from a script

```bash
pid=$(cat /tmp/pgfs-<hash>.pid) && kill -0 "$pid" && test -e /tmp/pgfs-<hash>.ready
```

## Get a useful crash report

Run with `RUST_BACKTRACE=1` (or `full`) so the report includes a
symbolised backtrace. The report is printed to stderr and
logged at ERROR, so it ends up in `testdata/pgfs.log` when the
daemon was started by `pgfs.sh`.

---

<small>Generated from `spec.app.ObservableDaemon` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
