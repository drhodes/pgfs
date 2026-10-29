# pgfs design decisions

Decisions from the step-by-step design discussion that follows the
spec-versus-literature review (`spec-literature-review.md`). The spec is
edited only after the discussion covers every topic.

| # | Topic | Decision |
|---|-------|----------|
| 1 | Purpose | Build A first: a general-purpose POSIX filesystem with database-grade durability, with small-file and metadata-heavy work as the sweet spot. B, a shared multi-machine filesystem (v0.2), is the goal and builds on A. |
| 1b | Killer feature | Branchable trees: instant read-only snapshots and writable copy-on-write clones of any subtree, built on shared, reference-counted blocks. A change feed is the supporting capability (B needs it for cache coherence anyway). |

## Consequences to carry into later topics

- Data model (4): blocks are shareable and copy-on-write; a tree or
  snapshot identity exists; persistent inode numbers are mandatory.
- Space and GC (11): deleting a file decrements block references, so
  space comes back only when the last reference goes.
- Verification (14): a reference-count invariant, and crash tests for
  snapshot and branch.
- Snapshot and branch are durability points: they return only after a
  synchronous commit.
