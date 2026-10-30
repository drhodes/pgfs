# Tutorial: Replicated Storage

!!! warning "Status: Partially implemented"
    Reads of names, attributes, listings and file contents go to the standby under per-inode LSN tickets, the standby's identity is verified, a lagging or unreachable standby falls back to the primary with no visible error, multi-host connection strings follow failover, and replica metrics are exported. Not yet: demoting a standby that is consistently slower, reading the primary's flush LSN other than from the mount's own commits, and spreading snapshot reads over several `--replica` standbys (only the first is used).

> A pgfs mount can be backed by a primary Postgres and physical streaming standbys. Writes go to the primary. A read goes to a standby only when the standby has replayed the mount's own latest commit, so the mount always reads its own writes. Snapshots can be read from any standby that has them. A standby failure never causes a kernel-visible error. A standby is not protection against corruption: physical replication copies a damaged page faithfully, which is what checksums and scrub are for.

**Tutorial** · [How-to](../how-to/replicated_filesystem.md) · [Reference](../reference/replicated_filesystem.md) · [Explanation](../explanation/replicated_filesystem.md)

---

In this tutorial we will give pgfs a read replica: a second PostgreSQL
that follows the first by streaming its write-ahead log. We will mount
pgfs with both, see reads served by the standby once it has replayed
our writes, then take the standby away and see pgfs carry on.

We need Docker, and the project-local cluster from [Your first pgfs
mount](projectlocalenvironment.md).

## Step 1: start the standby

```bash
./scripts/pgfs.sh down
./scripts/replica_db.sh up
```

The script copies the primary with `pg_basebackup`, starts a
`postgres:17` container on port 5433, and waits until it streams.

## Step 2: mount with the replica

```bash
mkdir -p testdata/mnt
RUST_LOG=pgfs=debug ./target/debug/pgfs mount main testdata/mnt \
    --replica "$(./scripts/replica_db.sh conn)" 2> testdata/replica-demo.log &
sleep 1
grep 'replica mode' testdata/replica-demo.log
```

```text
INFO pgfs::replica: replica mode: standby host=127.0.0.1 port=5433 dbname=pgfs user=alice verified
```

The daemon checked that the standby has the primary's system
identifier and `fs_uuid`, and that it is in recovery.

## Step 3: write, then read

```bash
echo "on the primary" > testdata/mnt/a.txt
sleep 1
cat testdata/mnt/a.txt
grep 'served by standby' testdata/replica-demo.log | tail -2
```

The write went to the primary. The read went to the standby only after
the standby had replayed past the write's LSN ticket.

## Step 4: take the standby away

```bash
./scripts/replica_db.sh down
cat testdata/mnt/a.txt
grep 'fall back' testdata/replica-demo.log | head -1
```

The read still works; the first fallback is logged once.

## Step 5: clean up

```bash
kill -TERM "$(pgrep -f 'pgfs mount main testdata/mnt')"
./scripts/replica_db.sh wipe
```

## What we did

We mounted pgfs across a primary and a standby, saw reads move to the
standby only when it had our writes, and back to the primary when it
went away, with no error reaching programs. The
[explanation](../explanation/replicated_filesystem.md) covers why.

---

<small>Generated from `spec.replica.ReplicatedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
