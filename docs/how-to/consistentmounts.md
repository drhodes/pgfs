# How-To: Consistency and Mount Ownership

!!! success "Status: Stable"
    One read-write lease per tree with fencing, read-only live mounts, the kernel cache lifetimes, one transaction per operation, bounded transactions with the rest handed to the reclaimer, and housekeeping in its own transactions are built and tested (`tests/storage.rs`, `test_lease.sh`). A commit of a partly written block reads that block first, because only the daemon can checksum the new content.

> Within a mount, every operation sees every earlier one. Each tree has at most one read-write mount, which holds a lease that a stalled daemon cannot outlive. Every operation is one bounded, atomic transaction.

[Tutorial](../tutorials/consistentmounts.md) · **How-to** · [Reference](../reference/consistentmounts.md) · [Explanation](../explanation/consistentmounts.md)

---

## Move a tree's mount to another machine

Unmount it cleanly on the first machine, then mount it on the
second. If the first machine is dead:

```bash
pgfs mount --takeover main /mnt/main
```

This waits for the dead mount's lease to expire, 30 s by
default.

## Let several machines write

Give each machine its own branch of the tree, then promote
the one you want to keep.

## Make read-only mounts fresher

```bash
pgfs mount --read-only --attr-ttl 0.1 main /mnt/main-ro
```

Lower TTLs cost more lookups.

---

<small>Generated from `spec.consistency.ConsistentMounts` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
