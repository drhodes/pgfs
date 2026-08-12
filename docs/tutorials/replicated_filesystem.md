# Tutorial: Replicated Physical Standby Storage

**Feature**: `spec.replica.ReplicatedFilesystem`

> A pgfs mount can be backed by a primary Postgres and a physical
streaming standby in Docker. Writes go to the primary; reads are
served from whichever node is authoritative at that instant, falling
back to the primary when the standby lags. The filesystem is
eventually consistent with the standby's lag continuously bounded by
the freshness check, and a standby failure never causes data loss or
a kernel-visible error.

---

In this tutorial, we will set up and verify a replicated pgfs mount with primary/standby PostgreSQL.
1. Start the Docker streaming standby container: `./scripts/replica_db.sh up`.
2. Launch pgfs with primary and standby parameters: `./target/release/pgfs --conn postgres://127.0.0.1:5432/pgfs --replica postgres://127.0.0.1:5433/pgfs`.
3. Perform write operations (`touch ./mnt/data.txt`) and observe writes directed to primary.
4. Perform read operations (`cat ./mnt/data.txt`) and observe reads served from the standby node.
5. Stop the standby container (`./scripts/replica_db.sh down`) and verify reads seamlessly fall back to primary without errors.
