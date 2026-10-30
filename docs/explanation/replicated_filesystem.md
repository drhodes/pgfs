# Explanation: Replicated Storage

!!! warning "Status: Partially implemented"
    Reads of names, attributes, listings and file contents go to the standby under per-inode LSN tickets, the standby's identity is verified, a lagging or unreachable standby falls back to the primary with no visible error, multi-host connection strings follow failover, and replica metrics are exported. Not yet: demoting a standby that is consistently slower, reading the primary's flush LSN other than from the mount's own commits, and spreading snapshot reads over several `--replica` standbys (only the first is used).

> A pgfs mount can be backed by a primary Postgres and physical streaming standbys. Writes go to the primary. A read goes to a standby only when the standby has replayed the mount's own latest commit, so the mount always reads its own writes. Snapshots can be read from any standby that has them. A standby failure never causes a kernel-visible error. A standby is not protection against corruption: physical replication copies a damaged page faithfully, which is what checksums and scrub are for.

[Tutorial](../tutorials/replicated_filesystem.md) · [How-to](../how-to/replicated_filesystem.md) · [Reference](../reference/replicated_filesystem.md) · **Explanation**

---

## Why a read ticket

A standby replays the primary's WAL a little behind it. A mount that
reads from a standby must never miss its own writes, so each mutation
records the LSN of its commit, and a read of that inode may use the
standby only once the standby has replayed that far. Comparing the
standby with the primary's *current* position instead is almost never
true on a busy primary, and two positions read by separate queries on
two nodes are not comparable at one instant.

Asynchronous commits complicate this: the WAL sender streams only up to
the primary's flush position, so a commit not yet flushed cannot be on
the standby however long one waits. The daemon therefore sends such
reads straight to the primary instead of waiting.

## Why identity checks

After a failover, an LSN alone cannot tell a standby of this cluster
from a node of another one. Checking the system identifier, the
timeline and `fs_uuid` makes sure the standby holds this filesystem's
history.

## What a standby is not

A standby is a copy, not protection against corruption: physical
replication copies a damaged page faithfully. Checksums and
`pgfs scrub` are for that. A standby failure never surfaces as an error
in the mount; it only turns standby reads into primary reads.

---

<small>Generated from `spec.replica.ReplicatedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
