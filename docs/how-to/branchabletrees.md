# How-To: Branchable Trees

!!! success "Status: Stable"
    Snapshots, branches (also of a subdirectory), copy-on-write through layered lookup, owner-drained and crash-consistent snapshots, read-only snapshot mounts, the depth cap with background auto-flatten, online flatten, normal and forced promote, deletion with a grace period, retained epochs and `pgfs diff` are built and covered by `tests/storage.rs`, the model-conformance suite and `test_snapshots.sh`.

> Freeze any pgfs tree in constant time, branch a writable copy of it in constant time, and atomically publish a branch as the new contents of a tree. Branches share every unchanged byte with their base and store only what they change.

[Tutorial](../tutorials/branchabletrees.md) · **How-to** · [Reference](../reference/branchabletrees.md) · [Explanation](../explanation/branchabletrees.md)

---

## Give every CI job its own copy of a large tree

```bash
pgfs snapshot main@ci-base            # once, or nightly
pgfs branch main@ci-base job-$CI_JOB_ID
pgfs mount job-$CI_JOB_ID /work/$CI_JOB_ID
# ... run the job ...
fusermount3 -u /work/$CI_JOB_ID
pgfs tree delete job-$CI_JOB_ID
```

Branching and deleting both take constant time. Space used by
the job is reclaimed in the background.

## Branch only one directory

```bash
pgfs branch main@ci-base:/services/api job-api
pgfs mount job-api /work/api
```

## Browse an old version

```bash
pgfs mount main@v1 /mnt/v1          # read-only
```

Any number of machines can mount the same snapshot.

## Roll back a promote

```bash
pgfs branch main@pre-promote-20261029T100312Z rollback
pgfs promote --force rollback main
```

## Snapshot when the owning daemon is unreachable

```bash
pgfs snapshot --crash-consistent main@emergency
```

The snapshot holds every committed operation but not writes
still buffered in the daemon, exactly what a crash would
keep.

## Keep lookups fast on deep branch chains

```bash
pgfs tree flatten job-api
```

Flattening runs online and copies metadata only.

---

<small>Generated from `spec.branch.BranchableTrees` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
