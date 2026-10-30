# Explanation: Sharded Filesystem

!!! danger "Status: Planned, not implemented"
    Superseded, not implemented. These requirements describe the v0.1 design (`design/sharded_filesystem-v0.1.md`): quorum fan-out, client-sequenced versions and hashing by top-level name, all of which the literature review advises against. The v0.2 design in `design/sharded_filesystem.md` replaces it, with one metadata database plus data sets replicated by Postgres. This module will be rewritten to v0.2 once the single-machine filesystem is complete (`design/design-decisions.md`, topic 10).

> A pgfs mount can be backed by N independent Postgres host nodes with RAID-5-like subset replication: each directory subtree is pinned to a replication set of R of the N hosts (never a full mirror on every host). Writes fan out to every member of the set; reads consult a quorum and return the newest version. Any number of clients can mount the same namespace from different machines and add files to one shared tree, and a host failure degrades reads and writes to the remaining healthy members without stale or corrupt data.

[Tutorial](../tutorials/sharded_filesystem.md) · [How-to](../how-to/sharded_filesystem.md) · [Reference](../reference/sharded_filesystem.md) · **Explanation**

---

## From one database to many

Today a pgfs namespace lives in one PostgreSQL database,
optionally with a read-only standby. That caps capacity at one
machine's disk and makes that machine a single point of
failure. The sharded design spreads the namespace across N
independent hosts and lets many clients mount it at once.

## Why not mirror everything

Copying every file to every host is simple, but it wastes
capacity, since N hosts hold one host's worth of data, and every
write costs N writes. Instead each file lives on R of the N
hosts, its replication set. With R = 3 of N = 5, losing any two
hosts still leaves every file at least one copy, and losing any
one keeps every set writable (two of three is a quorum), while
total capacity grows with the number of hosts. This is the sense in
which the design is "RAID-5-like": partial redundancy spread
across members, not a full mirror. Unlike RAID 5, it copies
whole files rather than computing parity.

## Why place by top-level directory

Placement uses the entry's top-level name, so everything under
`/docs` lives on one set. Hashing each file separately would
spread a directory's children across every host, and every
`ls` would become a cluster-wide merge. Subtree-sticky
placement keeps listings and most renames local to one set,
at the cost of hot spots if one top-level directory is much
larger than the rest.

## Why quorums and versions

The hosts in a set are independent databases with no
replication between them, so they can disagree, for example
after one missed a write while it was down. A write therefore
counts as done only when a majority of the set has it, and
every write carries a version number. A read asks a majority
and keeps the highest version. Any two majorities overlap, so a
read always sees the latest acknowledged write, even with a
member down.

Versions combine a client identifier with a per-client
counter, so they can be compared without synchronised clocks.
The cost is last-writer-wins semantics when two clients write
the same file at the same time.

## Why inode numbers encode the set

Each host's database has its own sequence, so two hosts would
hand out the same numbers. Putting the set id in the high bits
and letting one member of each set allocate makes inode numbers
unique across the cluster without coordination between sets.

## What the design leaves out

Cross-client cache coherence is bounded staleness via the
attribute TTL, not a guarantee: two clients can briefly see
different views. Changing the host list requires a migration.
Both are deliberate v1 limits, and the roadmap addresses the
first with leases in Phase 5.

---

<small>Generated from `spec.shard.ShardedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
