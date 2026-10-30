# Tutorial: Branchable Trees

!!! success "Status: Stable"
    Snapshots, branches (also of a subdirectory), copy-on-write through layered lookup, owner-drained and crash-consistent snapshots, read-only snapshot mounts, the depth cap with background auto-flatten, online flatten, normal and forced promote, deletion with a grace period, retained epochs and `pgfs diff` are built and covered by `tests/storage.rs`, the model-conformance suite and `test_snapshots.sh`.

> Freeze any pgfs tree in constant time, branch a writable copy of it in constant time, and atomically publish a branch as the new contents of a tree. Branches share every unchanged byte with their base and store only what they change.

**Tutorial** · [How-to](../how-to/branchabletrees.md) · [Reference](../reference/branchabletrees.md) · [Explanation](../explanation/branchabletrees.md)

---

In this tutorial we will snapshot a tree, branch it, change
the branch without touching the original, and then publish
the branch atomically. We need a formatted filesystem with
`main` mounted at `/mnt/main` (see [Administering
pgfs](filesystemlifecycle.md)).

## Step 1: put something in the tree

```bash
mkdir /mnt/main/site
echo "version 1" > /mnt/main/site/index.html
```

## Step 2: take a snapshot

```bash
pgfs snapshot main@v1
```

```text
snapshot main@v1 created (epoch 1, 0.004 s)
```

The snapshot took a few milliseconds and stores no data. It
would take the same time for a tree of a million files.

## Step 3: branch it and mount the branch

```bash
pgfs branch main@v1 staging
mkdir /mnt/staging
pgfs mount staging /mnt/staging
cat /mnt/staging/site/index.html
```

```text
version 1
```

## Step 4: change the branch

```bash
echo "version 2" > /mnt/staging/site/index.html
cat /mnt/main/site/index.html
```

```text
version 1
```

`main` is untouched. The branch stored one new block.

## Step 5: publish the branch

Unmount both trees, then promote:

```bash
fusermount3 -u /mnt/staging
fusermount3 -u /mnt/main
pgfs promote staging main
pgfs mount main /mnt/main
cat /mnt/main/site/index.html
```

```text
version 2
```

The old contents are kept as a snapshot:

```bash
pgfs snapshot list main
```

```text
main@v1                          2026-10-29T10:00:01Z
main@pre-promote-20261029T100312Z 2026-10-29T10:03:12Z
```

## What we did

We froze a tree, made a writable copy without copying data,
changed the copy, and swapped it in as one atomic step, with
the previous version kept for rollback.

---

<small>Generated from `spec.branch.BranchableTrees` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
