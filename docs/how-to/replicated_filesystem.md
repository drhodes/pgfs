# How-To: Replicated Storage

!!! warning "Status: Partially implemented"
    Write routing, lag detection, transparent fallback and the Docker standby work today. File contents (`read`) are still always served by the primary, and a standby that is unreachable when the daemon starts is dropped for the daemon's lifetime without a warning.

> A pgfs mount can be backed by a primary Postgres and a physical streaming standby in Docker. Writes go to the primary; reads are served from whichever node is authoritative at that instant, falling back to the primary when the standby lags. The filesystem is eventually consistent with the standby's lag continuously bounded by the freshness check, and a standby failure never causes data loss or a kernel-visible error.

[Tutorial](../tutorials/replicated_filesystem.md) · **How-to** · [Reference](../reference/replicated_filesystem.md) · [Explanation](../explanation/replicated_filesystem.md)

---

## Provision the Docker standby

```bash
./scripts/replica_db.sh up
./scripts/replica_db.sh conn      # prints the --replica string
```

`up` restarts the primary with a loopback TCP listener if it
does not already have one.

## Mount with a replica

```bash
./target/debug/pgfs --replica "$(./scripts/replica_db.sh conn)" testdata/mnt
```

`scripts/pgfs.sh` does not pass `--replica`; start the binary
directly.

## Use a standby you manage yourself

Any physical streaming standby of the primary's database
works:

```bash
pgfs --conn "host=primary.internal dbname=pgfs" \
     --replica "host=standby.internal dbname=pgfs" /srv/pgfs
```

pgfs never creates or alters the schema on the standby. It must
be a physical copy of the primary.

## Check that the standby is healthy

```bash
./scripts/replica_db.sh status
```

Look for `in recovery: t` and a small `replay_lag`. Then ask
the daemon what it thinks:

```bash
kill -USR1 <pid>; ls <mountpoint> > /dev/null
grep 'state|ino_cache' <daemon log> | tail -1     # replica fresh | stale or unreachable | none
```

## Tell whether reads are actually reaching the standby

Compare the counters in the minute-by-minute metrics line:

```bash
grep 'metrics|replica' <daemon log> | tail -3
```

`reads` growing means the standby is serving. `fallbacks`
growing much faster means it is lagging or unreachable.

## Fix a standby that never catches up

Symptom: `status` shows `in recovery: t`, the daemon reports
`stale or unreachable`, and the container log contains
`requested WAL segment ... has already been removed`:

```bash
docker logs --tail 5 pgfs-replica
```

The primary has recycled WAL the standby still needs, usually
because the standby was down for a long time. Rebuild it from a
fresh base backup:

```bash
./scripts/replica_db.sh wipe
docker rm pgfs-replica 2>/dev/null    # if a stopped container remains
./scripts/replica_db.sh up
```

## Fix "container name /pgfs-replica is already in use"

A stopped container from an earlier run remains, and `down` only
removes running containers:

```bash
docker rm pgfs-replica
./scripts/replica_db.sh up
```

## Attach a standby to a daemon that is already running

Restart the daemon with `--replica`. The standby connection is
made only at startup, and a standby that was unreachable then
stays detached until the next restart.

## Remove the standby

```bash
./scripts/replica_db.sh down     # stop, keep data
./scripts/replica_db.sh wipe     # stop and delete the data volume
```

---

<small>Generated from `spec.replica.ReplicatedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
