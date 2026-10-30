# Explanation: Consistency and Mount Ownership

!!! success "Status: Stable"
    One read-write lease per tree with fencing, read-only live mounts, the kernel cache lifetimes, one transaction per operation, bounded transactions with the rest handed to the reclaimer, and housekeeping in its own transactions are built and tested (`tests/storage.rs`, `test_lease.sh`). A commit of a partly written block reads that block first, because only the daemon can checksum the new content.

> Within a mount, every operation sees every earlier one. Each tree has at most one read-write mount, which holds a lease that a stalled daemon cannot outlive. Every operation is one bounded, atomic transaction.

[Tutorial](../tutorials/consistentmounts.md) · [How-to](../how-to/consistentmounts.md) · [Reference](../reference/consistentmounts.md) · **Explanation**

---

## Why one writer per tree

Two daemons writing one tree would each cache names and
attributes that the other could change. Keeping them coherent
needs callbacks or leases on both sides, sequence numbers,
and direct I/O for files that both write. That is the work
planned for the multi-machine version. Until then, one
writer per tree keeps every cache exact. Branches give each
machine a tree of its own at no cost.

## Why a lease and a fence

A lease alone is not enough. A daemon can pause, for a long
garbage collection, a stopped VM or a network partition,
and wake up believing it still holds the tree. The fence
number makes the database itself refuse that daemon's
writes, whatever it believes.

## Why one operation is one transaction

Postgres makes a transaction atomic and orders transactions
in its log. If every operation is exactly one transaction,
those guarantees become the filesystem's guarantees for
free: an operation is applied completely or not at all, and
a crash cuts history at a point.

---

<small>Generated from `spec.consistency.ConsistentMounts` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
