# Explanation: Space Management

!!! success "Status: Stable"
    statfs reports capacity and usage from delta-accounted totals, writes past the capacity reply ENOSPC, unshared blobs are freed inline, shared blobs are collected, retired rows and deleted layers are reclaimed after a grace period, orphans are reclaimed at lease acquisition, and space, backlog and amplification are reported. Without `--capacity` the free figure is a nominal 1 TiB. Per-tree quotas remain planned.

> pgfs reports real capacity and usage, refuses writes when full, and reclaims space from deleted files, snapshots and trees in the background, with the backlog and the write amplification always visible.

[Tutorial](../tutorials/spacemanagement.md) · [How-to](../how-to/spacemanagement.md) · [Reference](../reference/spacemanagement.md) · **Explanation**

---

## Why not reference counts

Copy-on-write storage usually counts references to each
block. In a database that counter is a row, and a block
shared by thousands of files makes that row a point of
contention. Counts also drift when there is a bug, and a
drifted count either leaks space or frees live data. pgfs
avoids counting. A block that was never shared is freed
with its only reference. A shared block is freed once an
index lookup shows nothing points to it.

## Why reclaim is lazy

Deleting a snapshot of a large tree may release millions of
rows. Doing that inside the delete would make the delete
slow and its transaction huge. pgfs makes the delete
instant and reclaims in small batches that yield to normal
work, and it always shows how much is left to reclaim.

## Write amplification

Every byte written to pgfs is written at least twice by
Postgres, once to the WAL and once to the table, and more
after checkpoints. pgfs reports the ratio so that it can be
watched and tuned, rather than discovered.

---

<small>Generated from `spec.space.SpaceManagement` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
