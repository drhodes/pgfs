# How-To: Observability & Diagnostics

!!! success "Status: Stable"
    Tracing with spans per callback (worker and view included) and per database call (statement, inode, backend pid, round trip), the nine 60-second metrics lines with every errno counted where it is replied, the SIGUSR1 dump written by the signal thread from atomics, and crash reports all ship. A test keeps the instrumented callback list in step with the `Filesystem` impl.

> Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.

[Tutorial](../tutorials/observabledaemon.md) · **How-to** · [Reference](../reference/observabledaemon.md) · [Explanation](../explanation/observabledaemon.md)

---

## Get JSON logs

```bash
RUST_LOG_FORMAT=json pgfs mount main /srv/pgfs 2> pgfs.jsonl
```

## Turn up one module

```bash
RUST_LOG=pgfs=info,pgfs::pg=debug pgfs mount main /srv/pgfs
```

## Match a slow span to the database

The `db` span records the backend pid; look it up in
`pg_stat_activity`.

---

<small>Generated from `spec.app.ObservableDaemon` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
