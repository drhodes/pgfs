# Tutorial: Performance Optimizations

!!! warning "Status: Partially implemented"
    Chunked blocks, prepared statements, asynchronous commit, unlogged tables, per-handle write buffering, transaction batching and the attribute-cache TTL are implemented. Inode-keyed storage, the multi-threaded connection pool and the kernel writeback cache are not. The Reference page has the per-requirement status.

> pgfs incorporates a structured performance optimization pipeline targeting database access latency, connection concurrency, userspace buffering, and FUSE kernel-transport tuning.

**Tutorial** · [How-to](../how-to/optimizationroadmap.md) · [Reference](../reference/optimizationroadmap.md) · [Explanation](../explanation/optimizationroadmap.md)

---

In this tutorial we will mount pgfs on a scratch database with
two tuning options, `--unlogged` and `--attr-ttl`, and measure
how many requests the kernel sends with and without attribute
caching. We need the project-local cluster running
(`./scripts/ensure_db.sh`).

## Step 1: create a scratch database and mount it

```bash
createdb -h "$PWD/testdata" pgfs_tuning
mkdir -p /tmp/pgfs-tuning
./target/debug/pgfs --conn "host=$PWD/testdata dbname=pgfs_tuning" \
    --unlogged --attr-ttl 0 /tmp/pgfs-tuning 2> /tmp/pgfs-tuning.log &
sleep 1
```

Because the database is new, `--unlogged` creates both tables
without write-ahead logging:

```bash
psql -h "$PWD/testdata" -d pgfs_tuning -Atc \
  "SELECT relname, relpersistence FROM pg_class WHERE relname IN ('entries', 'blocks')"
```

```text
entries|u
blocks|u
```

## Step 2: create some files

```bash
mkdir -p /tmp/pgfs-tuning/tree/{a,b,c,d,e}
for d in a b c d e; do for i in $(seq 1 20); do echo x > /tmp/pgfs-tuning/tree/$d/f$i; done; done
```

## Step 3: count requests with no attribute cache

We use the state dump's `fuse total`, the number of FUSE
requests served so far. Take one reading, walk the tree three
times, and take another:

```bash
PID=$(pgrep -f 'pgfs --conn.*pgfs_tuning')
dump() { kill -USR1 $PID; ls /tmp/pgfs-tuning > /dev/null; sleep 0.3;
         grep -o 'fuse_latency=fuse total=[0-9]*' /tmp/pgfs-tuning.log | tail -1; }
dump
for k in 1 2 3; do ls -lR /tmp/pgfs-tuning/tree > /dev/null; done
dump
```

With `--attr-ttl 0` the second total is about 4,200 higher: the
kernel asked pgfs about every entry, every time.

## Step 4: repeat with a 30-second cache

```bash
kill -TERM $PID; sleep 1
./target/debug/pgfs --conn "host=$PWD/testdata dbname=pgfs_tuning" \
    --attr-ttl 30 /tmp/pgfs-tuning 2> /tmp/pgfs-tuning.log &
sleep 1
PID=$(pgrep -f 'pgfs --conn.*pgfs_tuning')
dump
for k in 1 2 3; do ls -lR /tmp/pgfs-tuning/tree > /dev/null; done
dump
```

This time the difference is about 160. The first walk asked
pgfs; the next two were answered from the kernel's cache.

## Step 5: clean up

```bash
kill -TERM $PID; sleep 1
dropdb -h "$PWD/testdata" pgfs_tuning
rm -f /tmp/pgfs-tuning.log
```

## What we did

We created unlogged storage for a throwaway mount and cut the
requests for a metadata-heavy workload by more than 95% with
one flag. The [explanation](../explanation/optimizationroadmap.md)
covers what each option costs.

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
