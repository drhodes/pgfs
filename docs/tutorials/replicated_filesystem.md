# Tutorial: Replicated Storage

!!! warning "Status: Partially implemented"
    Write routing, lag detection, transparent fallback and the Docker standby work today. File contents (`read`) are still always served by the primary, and a standby that is unreachable when the daemon starts is dropped for the daemon's lifetime without a warning.

> A pgfs mount can be backed by a primary Postgres and a physical streaming standby in Docker. Writes go to the primary; reads are served from whichever node is authoritative at that instant, falling back to the primary when the standby lags. The filesystem is eventually consistent with the standby's lag continuously bounded by the freshness check, and a standby failure never causes data loss or a kernel-visible error.

**Tutorial** · [How-to](../how-to/replicated_filesystem.md) · [Reference](../reference/replicated_filesystem.md) · [Explanation](../explanation/replicated_filesystem.md)

---

In this tutorial we will give pgfs a read replica: a second
PostgreSQL that follows the first by streaming its write-ahead
log. We will mount pgfs with both, see metadata reads go to the
replica, then take the replica away and see pgfs carry on
without it.

We need Docker, and the project-local cluster from [Your first
pgfs mount](projectlocalenvironment.md).

## Step 1: start the standby

```bash
./scripts/pgfs.sh down
./scripts/replica_db.sh up
```

The script copies the primary with `pg_basebackup`, starts a
`postgres:17` container on port 5433, and waits until it is
streaming:

```text
==> standby is in recovery (streaming)

ready. replica connection string (for --replica):
  host=127.0.0.1 port=5433 dbname=pgfs user=alice
```

If it reports that the container name `/pgfs-replica` is
already in use, run `docker rm pgfs-replica` and repeat.

## Step 2: mount with the replica

```bash
mkdir -p testdata/mnt
./target/debug/pgfs --replica "$(./scripts/replica_db.sh conn)" testdata/mnt \
    2> testdata/replica-demo.log &
sleep 1
grep 'replica mode' testdata/replica-demo.log
```

```text
INFO pgfs: replica mode: reads served from standby (host=127.0.0.1 port=5433 dbname=pgfs user=alice) when fresh
```

## Step 3: write, wait, read

```bash
mkdir testdata/mnt/replicated
echo "on the primary" > testdata/mnt/replicated/a.txt
sleep 1
ls -l testdata/mnt/replicated
```

Writes went to the primary. After a moment the standby has
replayed them, and the `ls` (a directory listing plus
attribute lookups) is answered by the standby. Ask the daemon:

```bash
kill -USR1 "$(pgrep -f 'pgfs --replica')"; ls testdata/mnt > /dev/null
grep 'state|ino_cache' testdata/replica-demo.log | tail -1
```

```text
... db=connected (replica fresh) ...
```

## Step 4: take the standby away

```bash
./scripts/replica_db.sh down
ls -l testdata/mnt/replicated
cat testdata/mnt/replicated/a.txt
```

Both commands still work. The log records the switch once:

```text
WARN pgfs::db: reads falling back to primary (standby stale or unreachable)
```

## Step 5: clean up

```bash
kill -TERM "$(pgrep -f 'pgfs --replica')"
rm -f testdata/replica-demo.log
```

The standby's data stays in the Docker volume
`pgfs-replica-data`; `./scripts/replica_db.sh wipe` deletes it.

## What we did

We built a primary/standby pair, mounted pgfs across both, and
saw reads move to the standby when it was current and back to
the primary when it disappeared, with no error reaching the
programs using the mount. The
[explanation](../explanation/replicated_filesystem.md) covers
the consistency rules behind this.

---

<small>Generated from `spec.replica.ReplicatedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
