# Tutorial: Space Management

!!! success "Status: Stable"
    statfs reports capacity and usage from delta-accounted totals, writes past the capacity reply ENOSPC, unshared blobs are freed inline, shared blobs are collected, retired rows and deleted layers are reclaimed after a grace period, orphans are reclaimed at lease acquisition, and space, backlog and amplification are reported. Without `--capacity` the free figure is a nominal 1 TiB. Per-tree quotas remain planned.

> pgfs reports real capacity and usage, refuses writes when full, and reclaims space from deleted files, snapshots and trees in the background, with the backlog and the write amplification always visible.

**Tutorial** · [How-to](../how-to/spacemanagement.md) · [Reference](../reference/spacemanagement.md) · [Explanation](../explanation/spacemanagement.md)

---

In this tutorial we will watch space being used, shared and
reclaimed. We need `main` mounted at `/mnt/main`.

## Step 1: see usage

```bash
df -h /mnt/main
pgfs stats main
```

## Step 2: share data

```bash
head -c 100M /dev/urandom > /mnt/main/a.bin
cp /mnt/main/a.bin /mnt/main/b.bin
df -h /mnt/main
```

Usage grows by 100 MB, not 200 MB: the copy shares blocks.

## Step 3: hold space with a snapshot

```bash
pgfs snapshot main@keep
rm /mnt/main/a.bin /mnt/main/b.bin
pgfs stats main | grep snapshot
```

The bytes now belong to the snapshot only.

## Step 4: release it

```bash
pgfs snapshot delete main@keep
pgfs stats main | grep backlog
```

The backlog drains within seconds and `df` shows the space
free again.

---

<small>Generated from `spec.space.SpaceManagement` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
