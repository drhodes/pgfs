# How-To: Replicated Physical Standby Storage

**Feature**: `spec.replica.ReplicatedFilesystem`

> A pgfs mount can be backed by a primary Postgres and a physical
streaming standby in Docker. Writes go to the primary; reads are
served from whichever node is authoritative at that instant, falling
back to the primary when the standby lags. The filesystem is
eventually consistent with the standby's lag continuously bounded by
the freshness check, and a standby failure never causes data loss or
a kernel-visible error.

---

To deploy and operate replicated pgfs:
- Pass `--replica <connstring>` to specify the standby PostgreSQL database connection.
- Ensure the primary PostgreSQL daemon listens on TCP loopback (`-h 127.0.0.1`) and accepts replication connections in `pg_hba.conf`.
- Inspect daemon log lines or `REPLICA_READ_COUNT` and `REPLICA_FALLBACK_COUNT` metrics to verify read-routing health.
