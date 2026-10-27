# How-To: Sharded Filesystem

!!! danger "Status: Planned, not implemented"
    Specified and designed (`design/sharded_filesystem.md`), not implemented. The daemon does not accept `--shards` or `--replication-factor`, and `scripts/shard_db.sh` does not exist yet. The tutorial and how-to describe the intended interface.

> A pgfs mount can be backed by N independent Postgres host nodes with RAID-5-like subset replication: each directory subtree is pinned to a replication set of R of the N hosts (never a full mirror on every host). Writes fan out to every member of the set; reads consult a quorum and return the newest version. Any number of clients can mount the same namespace from different machines and add files to one shared tree, and a host failure degrades reads and writes to the remaining healthy members without stale or corrupt data.

[Tutorial](../tutorials/sharded_filesystem.md) · **How-to** · [Reference](../reference/sharded_filesystem.md) · [Explanation](../explanation/sharded_filesystem.md)

---

!!! warning "Planned interface"
    These instructions describe the designed interface. None
    of it is implemented yet.

## Deploy a sharded namespace

1. Run one PostgreSQL instance per host and create a `pgfs`
   database on each.
2. Start pgfs on every client with the same host list and
   replication factor:

   ```bash
   pgfs --shards "host=h1 dbname=pgfs,host=h2 dbname=pgfs,host=h3 dbname=pgfs,host=h4 dbname=pgfs,host=h5 dbname=pgfs" \
        --replication-factor 3 /srv/pgfs
   ```

Every client must list the hosts in the same order; placement
is computed from the list.

## Choose N and R

Use `R = 3` and `N ≥ R + 1`. A file survives the loss of
`R − 1` members of its set and stays writable while a quorum
(`⌊R/2⌋ + 1`) is up.

## Bound how stale other clients' changes can look

```bash
pgfs --shards "…" --attr-ttl 1 /srv/pgfs
```

Changes from other clients appear within the TTL.

## Detect a failing host

Watch `SHARD_DEGRADED_HOSTS` and `SHARD_QUORUM_MISSES` in the
minute-by-minute metrics, and use the SIGUSR1 state dump for
per-host health.

## Recover a host

Restart its PostgreSQL. On recovery the host is compared with
the highest version of each entry in its sets and missing
mutations are re-applied.

## Add or remove hosts

Not supported online. The topology is fixed at mount time, and
changing it requires a migration run (design §3.4).

---

<small>Generated from `spec.shard.ShardedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
