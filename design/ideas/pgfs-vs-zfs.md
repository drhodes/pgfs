# pgfs and ZFS

Status: comparison, not a plan. Recorded 2026-10-30.

Both have checksums, scrub, constant-time snapshots, writable clones and
boot environments. ZFS is far ahead as a local filesystem. pgfs does a
handful of things ZFS does not, because its storage is a database.

---

## Tutorial

Not applicable: a comparison has no lesson of its own. To see pgfs's side
of the table, run [Can it git?](can-it-git.md), which uses snapshots,
branches, diff and promote.

---

## How-to guides

### Choose between them

- **A fast, reliable local filesystem**, or one to host databases: ZFS.
- **One filesystem mounted by many machines**, queried with SQL,
  versioned with branches that refuse to clobber concurrent changes, or
  watched by consumers: pgfs.

### Use both: PostgreSQL on ZFS under pgfs

```bash
zfs create -o recordsize=8k -o compression=lz4 tank/pg     # PostgreSQL's page size
initdb -D /tank/pg/data --data-checksums
# keep full_page_writes on: pgfs refuses to mount without it
pgfs mkfs --conn "host=/run/postgresql dbname=pgfs"
```

ZFS gives the database fast, checksummed, redundant storage. pgfs gives
SQL, network mounts, branches and the change feed on top.

### Query the filesystem instead of walking it

```sql
SELECT path, size FROM pgfs.files WHERE tree = 'main' AND kind = 'f'
ORDER BY size DESC LIMIT 20;
```

This also works on a read standby, without mounting anything.

---

## Reference

### What pgfs does that ZFS does not

| | pgfs | ZFS |
|---|---|---|
| query the filesystem | it is tables: `pgfs.files`, joins with other data, on a standby | walk the tree, or `zfs diff` |
| mount from many machines | any host with libpq; one writer per tree by lease and fence; readers anywhere | one host imports the pool; others need NFS or SMB |
| branch any directory | `pgfs branch main:/projects/web job-17` | clones only at dataset boundaries planned in advance |
| publish with a check | `promote` folds back only if the target is unchanged (ESTALE otherwise) | `zfs promote` reverses clone/origin; publishing is rename or rollback |
| change feed | exactly once, causal order, resumable cursors, from any machine | none persistent; `zfs diff` between snapshots; inotify is local and lossy |
| restore to any moment | PostgreSQL point-in-time recovery | only to a snapshot taken |
| backups and replicas | any PostgreSQL backup or standby; standby reads that see your own writes | `zfs send`/`recv` |
| where it runs | userspace FUSE on any PostgreSQL 17, including managed ones | a kernel module on block devices |

### What ZFS does that pgfs does not

| | ZFS | pgfs |
|---|---|---|
| speed | about disk speed, ARC cache | a database round trip per operation; about 5 ms per create in `make bench` |
| maturity | about two decades in production | new |
| redundancy | vdevs, RAID-Z, self-healing | none of its own (use the database's storage) |
| dedup, encryption, zvols | yes | no |
| ACLs | yes | no |
| writeback cache, `mmap` performance | yes | writeback cache off by design |
| independence | needs nothing else running | stalls when PostgreSQL is unreachable |

---

## Explanation

### Why pgfs has features ZFS lacks

Storing a filesystem as rows makes the database's features into
filesystem features. Rows are queryable, so the filesystem is. A database
is a network service, so every tree can be mounted from anywhere, with
leases and fences to keep one writer. Every change is a transaction, so
recording it in a feed costs one more insert in the same transaction.
Point-in-time recovery, streaming replicas and standard backup tools come
for free.

### Why ZFS wins on everything a local filesystem is judged by

ZFS talks to disks from the kernel, caches in memory, and batches writes.
pgfs crosses the kernel, a daemon and a database connection for every
operation, and stores data as rows. It cannot be as fast, and it depends
on a database being reachable.

### Why they combine well

They sit at different levels. ZFS is a good place for PostgreSQL's data
directory; pgfs is a good way to expose that database as a shared,
versioned, queryable filesystem.
