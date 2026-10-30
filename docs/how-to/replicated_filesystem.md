# How-To: Replicated Storage

!!! warning "Status: Partially implemented"
    Reads of names, attributes, listings and file contents go to the standby under per-inode LSN tickets, the standby's identity is verified, a lagging or unreachable standby falls back to the primary with no visible error, multi-host connection strings follow failover, and replica metrics are exported. Not yet: demoting a standby that is consistently slower, reading the primary's flush LSN other than from the mount's own commits, and spreading snapshot reads over several `--replica` standbys (only the first is used).

> A pgfs mount can be backed by a primary Postgres and physical streaming standbys. Writes go to the primary. A read goes to a standby only when the standby has replayed the mount's own latest commit, so the mount always reads its own writes. Snapshots can be read from any standby that has them. A standby failure never causes a kernel-visible error. A standby is not protection against corruption: physical replication copies a damaged page faithfully, which is what checksums and scrub are for.

[Tutorial](../tutorials/replicated_filesystem.md) · **How-to** · [Reference](../reference/replicated_filesystem.md) · [Explanation](../explanation/replicated_filesystem.md)

---

## Provision the Docker standby

```bash
./scripts/replica_db.sh up
./scripts/replica_db.sh conn      # prints the --replica string
```

## Mount with a replica

```bash
pgfs mount main /srv/pgfs --replica "host=standby.internal dbname=pgfs sslmode=verify-full"
```

The standby must be a physical copy of the primary; pgfs never writes
to it. `--replica` is refused on a scratch filesystem, whose unlogged
tables do not replicate.

## Check what the mount thinks of the standby

```bash
kill -USR1 <pid>; grep 'state|replica' <daemon log> | tail -1
grep 'metrics|replica' <daemon log> | tail -1
```

`reads` counts standby reads, `fallbacks` reads that went to the
primary, and `lag_bytes` / `lag_ms` the standby's distance.

## Follow a failover

Give `--conn` every host and `target_session_attrs=read-write`:

```bash
pgfs mount main /srv/pgfs --conn "host=pg1,pg2 dbname=pgfs target_session_attrs=read-write"
```

After a failover the daemon reconnects to the new primary and
revalidates (system identifier, timeline, the op_log continuity check)
before serving.

## Fix a standby that never catches up

If the container log says `requested WAL segment ... has already been
removed`, the primary recycled WAL the standby still needed. Rebuild it:

```bash
./scripts/replica_db.sh wipe && ./scripts/replica_db.sh up
```

---

<small>Generated from `spec.replica.ReplicatedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
