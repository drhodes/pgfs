# Tutorial: Sharded Filesystem

!!! danger "Status: Planned, not implemented"
    Superseded, not implemented. These requirements describe the v0.1 design (`design/sharded_filesystem-v0.1.md`): quorum fan-out, client-sequenced versions and hashing by top-level name, all of which the literature review advises against. The v0.2 design in `design/sharded_filesystem.md` replaces it, with one metadata database plus data sets replicated by Postgres. This module will be rewritten to v0.2 once the single-machine filesystem is complete (`design/design-decisions.md`, topic 10).

> A pgfs mount can be backed by N independent Postgres host nodes with RAID-5-like subset replication: each directory subtree is pinned to a replication set of R of the N hosts (never a full mirror on every host). Writes fan out to every member of the set; reads consult a quorum and return the newest version. Any number of clients can mount the same namespace from different machines and add files to one shared tree, and a host failure degrades reads and writes to the remaining healthy members without stale or corrupt data.

**Tutorial** · [How-to](../how-to/sharded_filesystem.md) · [Reference](../reference/sharded_filesystem.md) · [Explanation](../explanation/sharded_filesystem.md)

---

!!! warning "Preview"
    This tutorial describes the intended experience once
    Phase 2 of the roadmap lands. None of its commands work
    yet.

In this tutorial we will spread one pgfs namespace across five
simulated hosts, write files into different subtrees, stop a
host and keep working, and mount the same namespace a second
time as another client.

## Step 1: start five hosts

```bash
./scripts/shard_db.sh up
./scripts/shard_db.sh conns
```

Each simulated host is its own PostgreSQL cluster
(`testdata/pgdata-1` … `testdata/pgdata-5`). `conns` prints the
comma-separated `--shards` list.

## Step 2: mount the namespace

```bash
./target/debug/pgfs --shards "$(./scripts/shard_db.sh conns)" \
    --replication-factor 3 testdata/mnt &
```

The startup log announces the topology:

```text
INFO pgfs: shard topology: 5 hosts, R=3, sets=…
```

## Step 3: write into two subtrees

```bash
mkdir testdata/mnt/docs testdata/mnt/src
echo hi  > testdata/mnt/docs/a.txt
echo bye > testdata/mnt/src/b.txt
```

`docs` and `src` are top-level names, so each subtree is placed
on its own replication set of three hosts. The SIGUSR1 state
dump shows which.

## Step 4: lose a host

```bash
pg_ctl -D testdata/pgdata-2 stop
cat testdata/mnt/docs/a.txt testdata/mnt/src/b.txt
echo again >> testdata/mnt/docs/a.txt
```

Every command still succeeds. Reads and writes need only two
of each set's three members. The metrics now report one
degraded host.

## Step 5: bring it back, and add a second client

```bash
./scripts/shard_db.sh up        # restarts host 2; it re-syncs
mkdir -p /tmp/client2
./target/debug/pgfs --shards "$(./scripts/shard_db.sh conns)" /tmp/client2 &
cat /tmp/client2/docs/a.txt
```

The second mount sees the same tree, including the line
written while host 2 was down.

## What we will have done

We will have run one namespace on five hosts with each file on
three of them, survived a host failure without errors, and
shared the tree between two clients. The
[explanation](../explanation/sharded_filesystem.md) describes
the design.

---

<small>Generated from `spec.shard.ShardedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
