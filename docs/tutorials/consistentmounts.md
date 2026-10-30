# Tutorial: Consistency and Mount Ownership

!!! success "Status: Stable"
    One read-write lease per tree with fencing, read-only live mounts, the kernel cache lifetimes, one transaction per operation, bounded transactions with the rest handed to the reclaimer, and housekeeping in its own transactions are built and tested (`tests/storage.rs`, `test_lease.sh`). A commit of a partly written block reads that block first, because only the daemon can checksum the new content.

> Within a mount, every operation sees every earlier one. Each tree has at most one read-write mount, which holds a lease that a stalled daemon cannot outlive. Every operation is one bounded, atomic transaction.

**Tutorial** · [How-to](../how-to/consistentmounts.md) · [Reference](../reference/consistentmounts.md) · [Explanation](../explanation/consistentmounts.md)

---

In this tutorial we will see that a tree has one writer, and
what happens when a second machine tries to mount it.

## Step 1: mount the tree

```bash
pgfs mount main /mnt/main
```

## Step 2: try to mount it again

From another terminal, or another machine using the same
database:

```bash
pgfs mount main /mnt/main2
```

```text
error: tree main is mounted read-write by host build-7 pid 41822 (EBUSY)
```

## Step 3: mount it read-only instead

```bash
pgfs mount --read-only main /mnt/main-ro
echo hi > /mnt/main/greeting
sleep 1; cat /mnt/main-ro/greeting
```

```text
hi
```

The read-only mount sees the change within its one-second
attribute TTL.

## What we did

We saw that a tree has one writer, and that readers
elsewhere follow it with a stated delay. For several
writers, give each its own branch (see [Branchable
Trees](branchabletrees.md)).

---

<small>Generated from `spec.consistency.ConsistentMounts` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
