# Explanation: Replicated Physical Standby Storage

**Feature**: `spec.replica.ReplicatedFilesystem`

> A pgfs mount can be backed by a primary Postgres and a physical
streaming standby in Docker. Writes go to the primary; reads are
served from whichever node is authoritative at that instant, falling
back to the primary when the standby lags. The filesystem is
eventually consistent with the standby's lag continuously bounded by
the freshness check, and a standby failure never causes data loss or
a kernel-visible error.

---

ReplicatedFilesystem offloads read traffic to physical streaming standby replicas while preserving strict POSIX consistency.
All mutating file operations execute synchronously on the primary database node. Read operations query the standby
only when its WAL replay LSN confirms it is synchronized with the primary. If replication lag exceeds tolerance or the replica fails,
reads immediately and silently fall back to the primary, guaranteeing data consistency and zero application downtime.
