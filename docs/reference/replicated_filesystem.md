# Reference: Replicated Physical Standby Storage

**Feature**: `spec.replica.ReplicatedFilesystem`

> A pgfs mount can be backed by a primary Postgres and a physical
streaming standby in Docker. Writes go to the primary; reads are
served from whichever node is authoritative at that instant, falling
back to the primary when the standby lags. The filesystem is
eventually consistent with the standby's lag continuously bounded by
the freshness check, and a standby failure never causes data loss or
a kernel-visible error.

---

ReplicatedFilesystem Specifications & Requirements:
- Provisioning: `StartsDockerStandby` (`scripts/replica_db.sh`), `RequiresPrimaryTcpListener`.
- Connection & Routing: `AcceptsReplicaConnection` (`--replica`), `ServesReadsFromReplica` (getattr, readdir, read), `WritesStayOnPrimary` (mutations on primary).
- Consistency: `DetectsReplicationLag` (checks LSN replay lag `pg_last_wal_replay_lsn`), `FallsBackToPrimary` (transparent fallback on lag/error).
- Observability: `ReplicaObservability` (`REPLICA_READ_COUNT`, `REPLICA_FALLBACK_COUNT`, SIGUSR1 status dump).
