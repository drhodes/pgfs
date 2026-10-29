# PGFS Sharded Shared Filesystem Design

**Document Version:** 0.1.0 (Draft)
**Status:** Proposed Architectural Specification
**Author:** PGFS Engineering Team
**Target Systems:** Linux VFS / FUSE, PostgreSQL 15+, N host nodes

---

## Executive Summary

Today `pgfs` mounts a directory tree backed by exactly one Postgres
instance, optionally with a single physical streaming standby that serves
reads (`spec.replica.py`). This design generalizes that model into a
**sharded, subset-replicated, shared** filesystem:

- **N host nodes**, each running a Postgres instance that holds part of the
  namespace. Individual files are **not** copied to every host.
- Each directory subtree is pinned to a **replication set** of `R` of the
  `N` hosts (default `R=3`, `N=5`) — a RAID-5-like partial mirror: `R` of
  `N`, never `N` of `N`.
- Writes fan out to every member of the entry's replication set; reads
  consult a quorum and return the newest version. A stale or crashed host
  degrades gracefully, exactly as the existing `--replica` fallback does.
- Any number of clients can **mount the same namespace from different
  machines**, reading and adding files to one shared tree.

The design reuses the existing seams: the `PgfsBackend` trait
(`src/backend.rs`), the `DbPool` connection machinery (`src/db.rs`), the
replica freshness/fallback logic, and the metrics/observability stack.

---

## Table of Contents

1. [Goals & Non-Goals](#1-goals--non-goals)
2. [Terminology & Topology](#2-terminology--topology)
3. [Placement: Subtree-Sticky Consistent Hashing](#3-placement-subtree-sticky-consistent-hashing)
4. [Global Inode Allocation](#4-global-inode-allocation)
5. [Read Path: Quorum & Versioning](#5-read-path-quorum--versioning)
6. [Write Path: Fan-Out & Acknowledgment](#6-write-path-fan-out--acknowledgment)
7. [Degraded Mode & Re-Sync](#7-degraded-mode--re-sync)
8. [Cross-Set Rename](#8-cross-set-rename)
9. [Shared Mounts & Multi-Machine Access](#9-shared-mounts--multi-machine-access)
10. [Schema & DDL Changes](#10-schema--ddl-changes)
11. [CLI & Configuration](#11-cli--configuration)
12. [Observability & Metrics](#12-observability--metrics)
13. [Operational Scripts](#13-operational-scripts)
14. [Consistency Model](#14-consistency-model)
15. [Phased Implementation Roadmap](#15-phased-implementation-roadmap)
16. [Open Questions](#16-open-questions)

---

## 1. Goals & Non-Goals

### Goals

- **Shared**: many machines mount the same namespace; users can add files
  concurrently from different locations.
- **Subset-replicated (RAID-5-like)**: each file lives on `R` of `N` hosts
  (e.g. 3 of 5), not a full mirror. Losing up to `R − 1` members of a
  file's replication set does not lose the file.
- **Scale-out capacity**: total namespace size is the sum of the hosts'
  storage, not the smallest single host.
- **Fault-tolerant**: a host crash degrades reads/writes to the remaining
  members of affected replication sets and never returns stale or corrupt
  data.
- **Backwards compatible**: `--conn` alone keeps today's single-node
  behavior; the existing `--replica` physical-standby mode keeps working.

### Non-Goals (v1)

- **Strong cross-mount cache coherence.** Leases / revocation
  (DistFUSE-style) are explicitly Phase 5 future work. v1 provides bounded
  staleness (`attr_ttl`) and last-writer-wins, documented in §14.
- **True erasure coding (parity blocks).** Stripe-level XOR/Reed-Solomon
  across hosts is a later optimization; v1 replicates whole files.
- **Dynamic rebalancing / re-sharding on membership change.** Topology is
  fixed at mount time; adding/removing hosts requires a migration run
  (documented in §3.4).
- **Cross-machine atomic multi-file operations.** Atomicity is per
  entry/rename, not across the cluster.

---

## 2. Terminology & Topology

| Term | Meaning |
|---|---|
| **host** / **shard node** | One machine running one Postgres instance holding a partition of the namespace. Identified by a libpq connection string. |
| **shard** | The partition of `entries`/`blocks` rows stored on one host. |
| **replication set** | An ordered list of `R` hosts that hold *the same* data (a placement group). Written as `set_id → [h_a, h_b, h_c]`. |
| **set_id** | Stable small integer identifying a replication set; used to seed inode ranges (§4) and the consistent-hash ring. |
| **quorum** | `⌊R/2⌋ + 1` members of a replication set (e.g. 2 of 3, 3 of 5). |
| **subtree** | The set of entries under one top-level name (`/docs`, `/src`, …), or a single top-level file. |
| **set leader** | The deterministically chosen inode allocator for a replication set (§4.2). |

Topology constants (v1 defaults):

```
N = 5   hosts
R = 3   replication factor  (R ≤ N; recommend N ≥ R + 1)
QUORUM = ⌊R/2⌋ + 1
```

Each host runs an **independent** Postgres instance (own `pgdata`, own
`entries`/`blocks` tables). There is **no** physical WAL streaming between
hosts of a set — divergence is prevented by write fan-out + versioning,
and repaired by re-sync (§7).

---

## 3. Placement: Subtree-Sticky Consistent Hashing

### 3.1 Rule

An entry's replication set is derived from its **top-level ancestor name**:
the first path component under `/`. All descendants of a top-level
directory live on the same set. This is the "subtree-sticky" property the
project chose.

```
/                        → replicated on every host (tiny, authoritative index)
/docs/guide.md           → set(placement("/docs"))
/src/backend/db.rs       → set(placement("/src"))
/notes.txt               → set(placement("/notes.txt"))
```

Why subtree-sticky:

- **`readdir` stays a single-host query.** Listing a directory only needs
  its own set, not a fan-out across hosts for every child. A naive
  per-entry hash would scatter children across hosts and make every `ls`
  a cluster-wide merge.
- **Locality.** Files that are usually read together (a project tree)
  colocate on the same hosts, so warm caches and connections are reused.
- **Simpler cross-host consistency.** Mutations to an entry only ever touch
  one replication set.

### 3.2 Placement Function

Consistent hashing over the top-level name:

```
ring = sorted [ hash(set_id → host_order) for set_id in 0..SETS ]
      where SETS is the number of replication sets and each set's
      members are the R consecutive hosts around the ring starting at
      a seed offset.

placement(name) = first replication set whose anchor hash ≥ hash(name),
                  wrapping around the ring.
```

- Deterministic: every client computes the same placement from the same
  `N` host list, `R`, and seed — no placement table to keep in sync.
- Consistent: when a host is *marked degraded* (not removed), placement is
  unchanged; reads/writes simply avoid that member while keeping the set
  (§7).

### 3.3 Root Directory

`/` and its `entries` row (`ino=1`) exist on **every** host. The set index
is implicit in the hash ring, so a client can list `/` by reading from any
reachable host (quorum not required for root — root is immutable except
for top-level entry creation, which is placed on the child's set).

### 3.4 Membership Change (documented, not automated in v1)

Because placement is computed from the ring, adding/removing a host changes
the ring and re-maps some top-level names to new sets. v1 treats topology
as fixed; a migration tool would re-home subtrees whose placement changed
(the same machinery as cross-set rename, §8). Not built in v1.

---

## 4. Global Inode Allocation

### 4.1 Problem

`entries.ino` is currently `GENERATED ALWAYS AS IDENTITY` per database.
With N independent databases, each generates `1, 2, 3, …` — inode numbers
would collide across hosts, and FUSE inodes must be unique across the
whole namespace (the kernel identifies files by ino).

### 4.2 Encoding

Inodes are globally unique by construction:

```
global_ino = (set_id << 48) | local_seq

  set_id   16 bits — the entry's replication set id
  local_seq 48 bits — unique within the set
```

- **Root** keeps `ino = 1` (special case, replicated everywhere).
- `set_id` identifies the *set the entry was created in*. After a
  cross-set rename (§8) the ino is retained, so its high bits name the
  original set — harmless, inos only need to be *unique*, not
  *descriptive*, and uniqueness is preserved because a row exists in
  exactly one set at a time.

### 4.3 Set-Leader Allocation

Collision within a set is prevented by making **one member of each set the
inode authority**:

- The set leader is deterministic: the set's first healthy member in ring
  order.
- The leader's `entries_ino_seq` is seeded to start at `(set_id << 48)`,
  so `INSERT … RETURNING ino` yields globally unique values.
- The client performs allocation against the leader only, then writes the
  **explicit** ino to the other `R − 1` members (writes already carry
  explicit inos; the DB identity is not used on follower insert).

Fallback: if the leader is unreachable, the next healthy member takes over
leadership for new allocations. Because the follower's sequence was not
seeded, a takeover changes `local_seq` source — the new leader must
allocate from a reserved region (`set_id << 48 | 2^47` boundary) to avoid
overlapping the departed leader's range. Documented; implemented in Phase 2
(§15).

---

## 5. Read Path: Quorum & Versioning

### 5.1 Versioning

Every write stamps the entry with a monotonic logical version:

```
ver = (client_id << 32) | client_local_seq
```

- `client_id` is unique per pgfs client (drawn from the root/authoritative
  space at startup, e.g. `set_id << 32 | random`);
- `client_local_seq` is a per-client monotonic counter.
- `ver` is globally comparable lexicographically; no wall-clock
  coordination between hosts is needed.

### 5.2 Read Algorithm

For `getattr`, `lookup`, `readdir`, and `read`:

1. Compute the entry's replication set from its top-level name (§3).
2. Query members of the set in ring order, skipping hosts currently marked
   degraded (§7).
3. Stop at quorum responses; pick the row with the **highest `ver`**.
   Ties are broken deterministically (highest ino).
4. If fewer than quorum respond, fall back to reading any remaining
   healthy member, then to the last known-good cached row — degrading to
   `EIO` only when no usable copy exists.

This generalizes today's `replica_fresh()` + `FallsBackToPrimary` logic
(`spec/replica.py`) from 2 nodes to R nodes.

---

## 6. Write Path: Fan-Out & Acknowledgment

### 6.1 Algorithm

Every mutation (`create`, `mkdir`, `write`, `unlink`, `rmdir`, `rename`):

1. Resolve the entry's replication set (§3).
2. Generate/allocate `ino` (§4) and `ver` (§5.1) client-side.
3. Execute the mutation as a single transaction on **every** member of the
   set (fan-out). Followers insert with the explicit `ino`/`ver`.
4. **Write quorum**: succeed when `≥ QUORUM` members ack; mark
   unacknowledged members degraded (§7).
5. If fewer than quorum ack, roll back the acknowledged members
   (compensating delete) and return `EIO`.

Consistency reads a mutation depends on (rename's source-existence check,
`rmdir`'s emptiness check) read from the **same set** with the same quorum
rule, never from a possibly-stale degraded member — mirroring
`WritesStayOnPrimary`'s spirit from `spec/replica.py`.

### 6.2 Transactional Fan-Out

Each member applies the mutation in its own `BEGIN … COMMIT` (there is no
distributed transaction in v1). Divergence windows are closed by:

- versioning + quorum reads (§5) — readers never observe a half-written
  mutation because the newer version wins;
- compensating rollback when the write quorum is not reached (§6.1 step 5);
- re-sync repair of lagging members (§7).

---

## 7. Degraded Mode & Re-Sync

### 7.1 Health Tracking

Per-client health table: each host has a state `{healthy, degraded}` and
an error/timestamp pair (the client stamps the last failure). A host is
degraded after `K` consecutive failures (default `K=3`, configurable); a
successful quorum read/write clears it after a grace interval.

The degraded set is **per client**, matching the existing
`--replica` model where each daemon independently decides freshness.

### 7.2 Behavior While Degraded

- **Reads**: skip degraded members, serve from the remaining healthy
  members (quorum still required; the set still has `R − degraded` members
  — when that drops below quorum, reads degrade to best-effort + `EIO`).
- **Writes**: fan out to healthy members only; require `≥ QUORUM` ack.
- **Metrics**: `SHARD_DEGRADED_HOSTS`, `SHARD_FALLBACK_COUNT` (§12) so the
  operator sees a host is unhealthy.

### 7.3 Re-Sync

When a degraded host comes back, the client verifies its freshness for
each set it belongs to (compare max `ver` on the healthy member against
the degraded member, per entry, incrementally) and re-applies missing
mutation rows. v1 re-syncs on demand per top-level subtree and is logged
at INFO. Full background scrubber is future work.

---

## 8. Cross-Set Rename

`rename(2)` where source and destination live in **different** replication
sets is the only operation that must move data between hosts:

1. Verify POSIX preconditions on the source set (source exists, target
   type-compatible) and destination set (target absent / replaceable) with
   quorum reads.
2. Snapshot the source subtree (entry rows + block rows) from any healthy
   source member.
3. Apply the snapshot to the destination set's members (new `parent_ino`,
   new `name`, same `ino`/`ver`, `ver` bumped) with write quorum.
4. Delete the source rows on the source set with write quorum.
5. If step 3 or 4 fails to reach quorum, roll the destination back and
   return `EIO` — a rename either fully lands on the new set or is
   unchanged.

Because inos are retained, open file handles and cached attributes keep
working across the move. Cost is `O(subtree size)` — acceptable because
cross-set renames are rare (top-level moves) while ordinary renames inside
a subtree stay a single-set local update (existing `stmt_rename_ino` path).

---

## 9. Shared Mounts & Multi-Machine Access

Multiple pgfs daemons on different machines mount the **same** namespace by
passing the **same** `--shards` topology:

- Every client computes identical placement (§3.2) and allocates
  collision-free inos (`client_id` + set leader, §4).
- Data races between clients are resolved by versioned quorum reads (§5)
  and fan-out writes (§6) — no client "owns" a file.
- Each client keeps its own kernel page cache and in-memory ino table.
  Bounded staleness: v1 sets `attr_ttl` (already a flag) to a small value
  (default 1s) and uses write-through semantics (no dirty-page write-back
  cache). This yields **read-your-writes on every client** and
  **eventual cross-client visibility** bounded by `attr_ttl`.

The strongly-consistent guarantee (a second client immediately observes
another client's write, with kernel-cache revocation) requires leases —
explicitly Phase 5 (§15), following DistFUSE's lease-offload model.

---

## 10. Schema & DDL Changes

The existing `entries`/`blocks` tables are kept; two columns are added:

```sql
ALTER TABLE entries ADD COLUMN IF NOT EXISTS ver bigint NOT NULL DEFAULT 0;

-- Inode allocation range per host is seeded at provisioning time:
--   leader host of set S:  ALTER SEQUENCE entries_ino_seq START WITH (S::bigint << 48);
```

`ver` stores the logical version (§5.1) used for quorum conflict
resolution. `blocks` requires no change (block rows inherit their entry's
`ino`/`ver` semantics; block data is versioned implicitly by the entry's
`ver` bump on write).

`entries.ino` remains `GENERATED ALWAYS AS IDENTITY` (leader allocation,
§4.3) with the sequence seeded per set. Followers insert explicit inos.

---

## 11. CLI & Configuration

```text
pgfs [OPTIONS] MOUNTPOINT

--conn <CONN>                     Single primary connstring (unchanged; single-node mode)
--shards <CONN[,CONN,…]>          N host connstrings; enables sharded mode (mutually
                                  exclusive with --replica)
--replication-factor <R>          Replication factor; default 3, must be ≤ N
--replica <CONN>                  Existing physical-standby mode (unchanged)
--attr-ttl <secs>                 Entry/attribute cache TTL; default 1.0 (bounded staleness)
--unlogged                        Unlogged tables (existing)
```

- `--shards` present ⇒ sharded topology; `--replica` and `--conn`
  together keep the current behavior.
- A sharded client builds the consistent-hash ring from the host list;
  `R`, `N`, and the ring seed are logged at startup and reported in the
  SIGUSR1 dump.

---

## 12. Observability & Metrics

Extend the existing `metrics.rs` export (and `spec/observe.py`):

- `SHARD_READ_COUNT`, `SHARD_WRITE_COUNT` — operations per shard mode.
- `SHARD_QUORUM_MISSES` — reads/writes that could not reach quorum.
- `SHARD_FALLBACK_COUNT` — reads served from a non-quorum fallback path.
- `SHARD_DEGRADED_HOSTS` — number of hosts currently marked degraded.
- `SHARD_LATENCY_*` — the existing microsecond histogram machinery,
  bucketed per operation.
- SIGUSR1 state dump: ring topology, per-set membership, per-host health,
  degraded set, leader election state.
- Log line on startup: `shard topology: N hosts, R=3, sets=…`.

---

## 13. Operational Scripts

- `scripts/shard_db.sh up|down|status|conns` — provision N project-local
  Postgres clusters (one per simulated host: separate `pgdata-N`,
  `pgport-N`), seed each `entries_ino_seq`, and print the `--shards`
  connstring list. Modeled on `init_db.sh` / `replica_db.sh`.
- `scripts/test_shard.sh` — integration suite: create files across
  subtrees, kill a host, verify reads/writes continue with quorum, bring
  the host back, verify re-sync. Modeled on `run_tests.sh` / `test_replica.sh`.
- `Makefile` targets: `make shard-up`, `make shard-down`, `make test-shard`.

---

## 14. Consistency Model

| Guarantee | v1 |
|---|---|
| Read-your-writes (same client) | Yes — writes are quorum-acked before the syscall returns |
| Cross-client visibility | Bounded by `attr_ttl` (default 1s), write-through caching |
| Concurrent writes to different files | Safe — disjoint sets or quorum per set |
| Concurrent writes to the same file | Last-writer-wins by `ver` |
| Host failure | No stale/corrupt reads; degrade to healthy set members |
| Strong kernel cache coherence (leases) | Phase 5 |

---

## 15. Phased Implementation Roadmap

```mermaid
gantt
    title PGFS Sharded Filesystem Roadmap
    dateFormat  YYYY-MM-DD

    section Phase 0 — Foundation
    CLI: --shards/--replication-factor, ring topology, placement fn  :p0, 2026-08-12, 5d
    Generalize DbPool to Vec<shard pools>                            :p0b, after p0, 4d

    section Phase 1 — Read Path
    Quorum reads + ver selection, per-set fallback                   :p1, after p0b, 5d
    Ino encoding (set_id<<48) + root special-case                    :p1b, after p1, 4d

    section Phase 2 — Write Path
    Fan-out writes + write quorum + compensating rollback            :p2, after p1b, 6d
    Set-leader ino allocation + takeover reserve region              :p2b, after p2, 4d

    section Phase 3 — Degraded Mode & Re-Sync
    Health tracking, degraded set, re-sync on recovery               :p3, after p2b, 5d

    section Phase 4 — Shared Mounts & Cross-Set Rename
    Cross-set subtree migration                                      :p4, after p3, 5d
    Multi-client semantics, tests (test_shard.sh), docs              :p4b, after p4, 5d

    section Phase 5 — Future: Leases
    DistFUSE-style kernel-offloaded lease revocation                 :p5, after p4b, 12d
```

Milestone exit criteria (each phase lands green):

- **P0** `cargo check` + unit tests for the ring/placement function.
- **P1** `make test-shard` read-only: `ls`, `cat` across subtrees with one
  host stopped.
- **P2** `make test-integration` passes in sharded mode: create/write/append/
  rename/truncate/git-init.
- **P3** kill-host and recovery tests (`test_shard.sh`).
- **P4** two concurrent clients (two `--shards` mounts) pass the suite;
  cross-top-level `mv` works.

---

## 16. Open Questions

1. **Degraded-host re-sync granularity**: per-subtree on demand (v1) vs.
   full background scrubber (future). Confirm acceptable lag.
2. **`ver` conflict semantics**: LWW is simplest; do we need read-modify
   expectations for e.g. `O_APPEND` across clients? (v1: no.)
3. **Topology change tooling**: ship the migration script in v1 or defer?
4. **Set-leader takeover reserve region** (§4.3): confirm the `2^47`
   boundary vs. a per-host allocation-range table in a root-replicated
   `meta` table.
5. **Replication factor default**: `R=3` of `N=5` per the RAID-5 analogy —
   confirm `N≥R+1` expectation for deployments.
