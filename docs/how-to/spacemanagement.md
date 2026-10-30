# How-To: Space Management

!!! success "Status: Stable"
    statfs reports capacity and usage from delta-accounted totals, writes past the capacity reply ENOSPC, unshared blobs are freed inline, shared blobs are collected, retired rows and deleted layers are reclaimed after a grace period, orphans are reclaimed at lease acquisition, and space, backlog and amplification are reported. Without `--capacity` the free figure is a nominal 1 TiB. Per-tree quotas remain planned.

> pgfs reports real capacity and usage, refuses writes when full, and reclaims space from deleted files, snapshots and trees in the background, with the backlog and the write amplification always visible.

[Tutorial](../tutorials/spacemanagement.md) · **How-to** · [Reference](../reference/spacemanagement.md) · [Explanation](../explanation/spacemanagement.md)

---

## Set a capacity

```bash
pgfs tune --capacity 500G
```

## Find what is holding space

```bash
pgfs stats main            # live, snapshot-only, backlog
pgfs snapshot list main    # unique bytes per snapshot
```

## Check write amplification

```bash
pgfs stats main | grep amplification
```

Values far above 3 on a write-heavy workload usually mean
many small partial-block writes. A larger write buffer, or a
smaller block size at mkfs, helps.

---

<small>Generated from `spec.space.SpaceManagement` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
