# Reference: Sharded Filesystem

!!! danger "Status: Planned, not implemented"
    Superseded, not implemented. These requirements describe the v0.1 design (`design/sharded_filesystem-v0.1.md`): quorum fan-out, client-sequenced versions and hashing by top-level name, all of which the literature review advises against. The v0.2 design in `design/sharded_filesystem.md` replaces it, with one metadata database plus data sets replicated by Postgres. This module will be rewritten to v0.2 once the single-machine filesystem is complete (`design/design-decisions.md`, topic 10).

> A pgfs mount can be backed by N independent Postgres host nodes with RAID-5-like subset replication: each directory subtree is pinned to a replication set of R of the N hosts (never a full mirror on every host). Writes fan out to every member of the set; reads consult a quorum and return the newest version. Any number of clients can mount the same namespace from different machines and add files to one shared tree, and a host failure degrades reads and writes to the remaining healthy members without stale or corrupt data.

[Tutorial](../tutorials/sharded_filesystem.md) · [How-to](../how-to/sharded_filesystem.md) · **Reference** · [Explanation](../explanation/sharded_filesystem.md)

---

All items on this page are planned. Values come from
`design/sharded_filesystem.md` (Draft 0.1.0).

## Command line

| Option | Meaning |
|---|---|
| `--shards <CONN[,CONN,…]>` | the N host connection strings; enables sharded mode; mutually exclusive with `--replica` |
| `--replication-factor <R>` | members per replication set; default 3; must be ≤ N |
| `--attr-ttl <secs>` | bounds cross-client staleness; default 1.0 |

## Topology

| Term | Definition |
|---|---|
| host | one PostgreSQL instance, identified by a connection string |
| replication set | an ordered list of R hosts that hold the same entries |
| `set_id` | small stable integer naming a replication set |
| quorum | `⌊R/2⌋ + 1` members of a set |
| subtree | everything under one top-level name |
| set leader | the set member that allocates inode numbers |

## Placement

| Rule | Detail |
|---|---|
| Key | the entry's top-level ancestor name |
| Function | consistent hashing over the ring of replication sets |
| Stickiness | every descendant of a top-level name lives on the same set |
| Root (inode 1) | replicated on every host |

## Inode numbers

| Property | Value |
|---|---|
| Encoding | `(set_id << 48) | local_seq` |
| Allocator | the set leader's `entries_ino_seq`, seeded at `set_id << 48` |
| Leader failure | the next healthy member allocates from a disjoint reserved range |
| Cross-set rename | keeps the inode number |

## Versions

`ver = (client_id << 32) | client_local_seq`, stamped on every
mutation. Reads return the row with the highest `ver`;
concurrent writers to one entry resolve last-writer-wins.

## Operations

| Operation | Behaviour |
|---|---|
| `getattr`, `lookup`, `readdir`, `read` | quorum read from the entry's set, highest `ver` wins, degraded hosts skipped |
| `create`, `mkdir`, `write`, `unlink`, `rmdir`, `rename` | one transaction on every healthy set member; succeeds when a quorum acknowledges; otherwise acknowledged members are rolled back and the call fails with `EIO` |
| rename across sets | snapshot the source subtree, apply it to the destination set, delete the source; quorum on each step; compensating rollback on failure |

## Health

| Parameter | Value |
|---|---|
| Degraded after | K consecutive failures |
| While degraded | skipped by reads; writes go to healthy members only |
| Recovery | compare with the highest `ver` per entry and re-apply missing mutations |

## Metrics

`SHARD_READ_COUNT`, `SHARD_WRITE_COUNT`, `SHARD_QUORUM_MISSES`,
`SHARD_FALLBACK_COUNT`, `SHARD_DEGRADED_HOSTS`; the startup log
line `shard topology: N hosts, R=…, sets=…`; and, in the SIGUSR1
dump, ring, set membership, host health, degraded set and
leader state.

## Consistency

| Guarantee | v1 |
|---|---|
| read-your-writes, same client | yes |
| cross-client visibility | within `attr_ttl` |
| concurrent writes, different files | safe |
| concurrent writes, same file | last writer wins by `ver` |
| host failure | no stale or corrupt reads |
| kernel cache coherence across clients (leases) | future (Phase 5) |

## Roadmap

| Phase | Scope |
|---|---|
| 0 | CLI, ring, placement; one pool per host |
| 1 | quorum reads with `ver`; inode encoding |
| 2 | fan-out writes, write quorum, rollback; leader allocation |
| 3 | health tracking, degraded mode, re-sync |
| 4 | cross-set rename, multi-client tests |
| 5 | leases for cross-client cache coherence |


## Specification requirements

`ShardedFilesystem` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "AcceptsShardTopology"
    `spec.shard.AcceptsShardTopology`

    The daemon accepts `--shards <CONN[,CONN,…]>` listing the N host
    connection strings and `--replication-factor <R>` (default 3, must be
    ≤ N). When `--shards` is present, the daemon runs in sharded mode and
    builds the consistent-hash ring from the host list; the existing
    `--conn` / `--replica` single-node and physical-standby modes are
    unchanged when `--shards` is absent.

??? abstract "ComputesDeterministicPlacement"
    `spec.shard.ComputesDeterministicPlacement`

    Every client computes the same replication set for a given entry
    from its top-level ancestor name using consistent hashing over the
    ring of N hosts with factor R — no shared placement table is needed.
    Placement is subtree-sticky: all descendants of a top-level directory
    live on the same replication set, so `readdir` is a single-set query
    rather than a cluster-wide merge.

??? abstract "ReplicatesToSubsetOfHosts"
    `spec.shard.ReplicatesToSubsetOfHosts`

    Each entry is replicated to exactly R of the N hosts (a replication
    set) — the RAID-5-like partial mirror. The root directory (ino=1) is
    the only exception and is replicated on every host so any client can
    list `/`. Data is never copied to all N hosts.

??? abstract "AllocatesGloballyUniqueInodes"
    `spec.shard.AllocatesGloballyUniqueInodes`

    Inodes are unique across the whole namespace even though each host
    runs an independent Postgres instance with its own identity sequence.
    The global inode encodes the entry's replication set id in the high
    bits: `ino = (set_id << 48) | local_seq`. The root keeps ino=1. A
    cross-set rename retains the inode, so uniqueness is preserved.

??? abstract "AllocatesInodesViaSetLeader"
    `spec.shard.AllocatesInodesViaSetLeader`

    Each replication set designates one healthy member (the set leader,
    chosen deterministically in ring order) as the inode authority. The
    leader's `entries_ino_seq` is seeded to start at `(set_id << 48)`, so
    `INSERT … RETURNING ino` yields globally unique values. The client
    allocates against the leader, then fans the explicit ino out to the
    other R−1 members. If the leader is unreachable, the next healthy
    member takes over from a disjoint reserved region so allocations never
    overlap the departed leader's range.

??? abstract "ServesReadsFromReplicationSet"
    `spec.shard.ServesReadsFromReplicationSet`

    Read-only operations — `getattr`, `lookup`, `readdir`, and `read` —
    are resolved to the entry's replication set and served from its
    members. A versioned quorum read picks the row with the highest `ver`;
    ties are broken deterministically. Reads skip hosts currently marked
    degraded.

??? abstract "VersionsMutations"
    `spec.shard.VersionsMutations`

    Every mutation stamps the entry with a monotonically increasing
    logical version `ver = (client_id << 32) | client_local_seq`. `ver` is
    globally comparable without wall-clock coordination between hosts, so
    concurrent writers to the same entry resolve to last-writer-wins by
    version.

??? abstract "FansOutWritesToSetMembers"
    `spec.shard.FansOutWritesToSetMembers`

    Every mutation — `create`, `mkdir`, `write`, `unlink`, `rmdir`,
    `rename` — executes as a single transaction on every member of the
    entry's replication set (fan-out), with followers inserting the
    explicit `ino`/`ver` supplied by the client. Consistency reads that a
    mutation depends on (rename's source-existence check, rmdir's
    emptiness check) read from the same set with the same quorum rule,
    never from a possibly-stale degraded member.

??? abstract "RequiresWriteQuorum"
    `spec.shard.RequiresWriteQuorum`

    A mutation succeeds only when at least `QUORUM = ⌊R/2⌋ + 1` members
    acknowledge it; unacknowledged members are marked degraded. If fewer
    than quorum acknowledge, the acknowledged members are rolled back with
    a compensating delete and the operation replies EIO — a write either
    lands on a quorum or is not reported as committed.

??? abstract "DegradesGracefullyOnHostFailure"
    `spec.shard.DegradesGracefullyOnHostFailure`

    A host is marked degraded after K consecutive failures. While
    degraded, reads skip it and writes fan out only to healthy members,
    both still requiring quorum. Reads never return stale or corrupt data
    from a degraded host, and the degraded set is reported in metrics and
    the SIGUSR1 state dump. A recovering host is verified against the
    highest `ver` per entry and missing mutations are re-applied.

??? abstract "SupportsCrossSetRename"
    `spec.shard.SupportsCrossSetRename`

    `rename` whose source and destination live in different replication
    sets snapshots the source subtree from a healthy member, applies it to
    the destination set (new parent/name, retained ino, bumped ver), then
    deletes the source rows — all quorum-acked, with compensating rollback
    on the destination if either half fails. Re-lands atomically or is
    unchanged. Ordinary renames inside one subtree stay single-set local
    updates.

??? abstract "SupportsSharedMounts"
    `spec.shard.SupportsSharedMounts`

    Multiple pgfs daemons on different machines can mount the same
    namespace by passing the same `--shards` topology. Identical placement
    computation and collision-free inode allocation let any client read
    and add files to one shared tree. Cross-client visibility is bounded
    by `attr_ttl` with write-through caching; strongly consistent kernel
    cache coherence via leases is future work.

??? abstract "ShardObservability"
    `spec.shard.ShardObservability`

    Sharded-mode operation is observable: the 60-second metrics export
    includes `SHARD_READ_COUNT`, `SHARD_WRITE_COUNT`, `SHARD_QUORUM_MISSES`,
    `SHARD_FALLBACK_COUNT`, and `SHARD_DEGRADED_HOSTS`; the startup log
    announces the topology (`N hosts, R, ring seed`); and the SIGUSR1 state
    dump reports the ring, per-set membership, per-host health, degraded
    set, and leader state.

---

<small>Generated from `spec.shard.ShardedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
