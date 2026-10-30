# Explanation: Branchable Trees

!!! success "Status: Stable"
    Snapshots, branches (also of a subdirectory), copy-on-write through layered lookup, owner-drained and crash-consistent snapshots, read-only snapshot mounts, the depth cap with background auto-flatten, online flatten, normal and forced promote, deletion with a grace period, retained epochs and `pgfs diff` are built and covered by `tests/storage.rs`, the model-conformance suite and `test_snapshots.sh`.

> Freeze any pgfs tree in constant time, branch a writable copy of it in constant time, and atomically publish a branch as the new contents of a tree. Branches share every unchanged byte with their base and store only what they change.

[Tutorial](../tutorials/branchabletrees.md) · [How-to](../how-to/branchabletrees.md) · [Reference](../reference/branchabletrees.md) · **Explanation**

---

## Snapshots are a number, not a copy

Every row that describes a file records the epoch it was
born in and the epoch it died in. A snapshot writes down the
current epoch and starts a new one. From then on, a change to
an old row does not overwrite it; it marks the row dead and
writes a new one. The snapshot sees the rows that were alive
at its epoch, so it never changes, and taking it touched two
rows. Petal used epoch numbers in its block maps the same
way, and Oracle's cloud file service versions its keys.

## Branches are layers

A branch is an empty layer on top of a snapshot. Looking up a
name checks the branch first and then the snapshot below it.
Changing a file copies only what changes into the branch: an
attribute, a name, the blocks that were written. Deleting an
inherited name leaves a whiteout, as overlay filesystems do.
So a branch costs nothing to create and grows only with what
it changes.

## Why depth is limited

Every layer is one more index probe on every lookup. BetrFS
found that sharing without limits slowly destroys locality.
pgfs caps the chain at 8 and flattens long chains in the
background, copying metadata but never data.

## Why promote is a compare-and-swap

Promote replaces the tree only if nothing changed it since
the branch was taken. pgfs does not try to merge two lines of
change; it refuses, and the caller decides. Because the tree
has not changed since the branch point, a normal promote folds
the branch's delta directly into the tree's layer so layer
depth never grows, while keeping the previous state as a
snapshot: prepare in private, publish in one atomic step, keep
the old version.

## Why a snapshot asks the daemon first

A daemon may hold writes it has acknowledged but not yet
committed. Asking it to drain first makes the snapshot
include everything a program has written. When that is not
possible, a crash-consistent snapshot is still well defined:
it is a state the filesystem could have recovered to after a
crash.

---

<small>Generated from `spec.branch.BranchableTrees` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
