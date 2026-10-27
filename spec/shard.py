"""
Sharded-mode requirements for pgfs.

A pgfs mount can be backed by N independent Postgres host nodes with
RAID-5-like subset replication: each directory subtree is pinned to a
replication set of R of the N hosts (never a full mirror on every host).
Writes fan out to every member of the entry's replication set; reads
consult a quorum and return the newest version. Any number of clients can
mount the same namespace from different machines and add files to one
shared tree.

These Requirement classes are granular and single-responsibility,
following the project's convention: each one is independently
footprintable, testable, and implementable. The architectural rationale
lives in design/sharded_filesystem.md.
"""

from .err import PLANNED, Feat, Req, md


# ── Topology & placement ─────────────────────────────────────────────

class AcceptsShardTopology(Req):
    """The daemon accepts `--shards <CONN[,CONN,…]>` listing the N host
    connection strings and `--replication-factor <R>` (default 3, must be
    ≤ N). When `--shards` is present, the daemon runs in sharded mode and
    builds the consistent-hash ring from the host list; the existing
    `--conn` / `--replica` single-node and physical-standby modes are
    unchanged when `--shards` is absent."""


class ComputesDeterministicPlacement(Req):
    """Every client computes the same replication set for a given entry
    from its top-level ancestor name using consistent hashing over the
    ring of N hosts with factor R — no shared placement table is needed.
    Placement is subtree-sticky: all descendants of a top-level directory
    live on the same replication set, so `readdir` is a single-set query
    rather than a cluster-wide merge."""


class ReplicatesToSubsetOfHosts(Req):
    """Each entry is replicated to exactly R of the N hosts (a replication
    set) — the RAID-5-like partial mirror. The root directory (ino=1) is
    the only exception and is replicated on every host so any client can
    list `/`. Data is never copied to all N hosts."""


# ── Inode allocation ─────────────────────────────────────────────────

class AllocatesGloballyUniqueInodes(Req):
    """Inodes are unique across the whole namespace even though each host
    runs an independent Postgres instance with its own identity sequence.
    The global inode encodes the entry's replication set id in the high
    bits: `ino = (set_id << 48) | local_seq`. The root keeps ino=1. A
    cross-set rename retains the inode, so uniqueness is preserved."""


class AllocatesInodesViaSetLeader(Req):
    """Each replication set designates one healthy member (the set leader,
    chosen deterministically in ring order) as the inode authority. The
    leader's `entries_ino_seq` is seeded to start at `(set_id << 48)`, so
    `INSERT … RETURNING ino` yields globally unique values. The client
    allocates against the leader, then fans the explicit ino out to the
    other R−1 members. If the leader is unreachable, the next healthy
    member takes over from a disjoint reserved region so allocations never
    overlap the departed leader's range."""


# ── Read path ────────────────────────────────────────────────────────

class ServesReadsFromReplicationSet(Req):
    """Read-only operations — `getattr`, `lookup`, `readdir`, and `read` —
    are resolved to the entry's replication set and served from its
    members. A versioned quorum read picks the row with the highest `ver`;
    ties are broken deterministically. Reads skip hosts currently marked
    degraded."""


class VersionsMutations(Req):
    """Every mutation stamps the entry with a monotonically increasing
    logical version `ver = (client_id << 32) | client_local_seq`. `ver` is
    globally comparable without wall-clock coordination between hosts, so
    concurrent writers to the same entry resolve to last-writer-wins by
    version."""


# ── Write path ───────────────────────────────────────────────────────

class FansOutWritesToSetMembers(Req):
    """Every mutation — `create`, `mkdir`, `write`, `unlink`, `rmdir`,
    `rename` — executes as a single transaction on every member of the
    entry's replication set (fan-out), with followers inserting the
    explicit `ino`/`ver` supplied by the client. Consistency reads that a
    mutation depends on (rename's source-existence check, rmdir's
    emptiness check) read from the same set with the same quorum rule,
    never from a possibly-stale degraded member."""


class RequiresWriteQuorum(Req):
    """A mutation succeeds only when at least `QUORUM = ⌊R/2⌋ + 1` members
    acknowledge it; unacknowledged members are marked degraded. If fewer
    than quorum acknowledge, the acknowledged members are rolled back with
    a compensating delete and the operation replies EIO — a write either
    lands on a quorum or is not reported as committed."""


class DegradesGracefullyOnHostFailure(Req):
    """A host is marked degraded after K consecutive failures. While
    degraded, reads skip it and writes fan out only to healthy members,
    both still requiring quorum. Reads never return stale or corrupt data
    from a degraded host, and the degraded set is reported in metrics and
    the SIGUSR1 state dump. A recovering host is verified against the
    highest `ver` per entry and missing mutations are re-applied."""


# ── Cross-set rename ─────────────────────────────────────────────────

class SupportsCrossSetRename(Req):
    """`rename` whose source and destination live in different replication
    sets snapshots the source subtree from a healthy member, applies it to
    the destination set (new parent/name, retained ino, bumped ver), then
    deletes the source rows — all quorum-acked, with compensating rollback
    on the destination if either half fails. Re-lands atomically or is
    unchanged. Ordinary renames inside one subtree stay single-set local
    updates."""


# ── Shared mounts ────────────────────────────────────────────────────

class SupportsSharedMounts(Req):
    """Multiple pgfs daemons on different machines can mount the same
    namespace by passing the same `--shards` topology. Identical placement
    computation and collision-free inode allocation let any client read
    and add files to one shared tree. Cross-client visibility is bounded
    by `attr_ttl` with write-through caching; strongly consistent kernel
    cache coherence via leases is future work."""


# ── Observability ────────────────────────────────────────────────────

class ShardObservability(Req):
    """Sharded-mode operation is observable: the 60-second metrics export
    includes `SHARD_READ_COUNT`, `SHARD_WRITE_COUNT`, `SHARD_QUORUM_MISSES`,
    `SHARD_FALLBACK_COUNT`, and `SHARD_DEGRADED_HOSTS`; the startup log
    announces the topology (`N hosts, R, ring seed`); and the SIGUSR1 state
    dump reports the ring, per-set membership, per-host health, degraded
    set, and leader state."""


# ── User-visible feature ─────────────────────────────────────────────

class ShardedFilesystem(Feat):
    """A pgfs mount can be backed by N independent Postgres host nodes
    with RAID-5-like subset replication: each directory subtree is pinned
    to a replication set of R of the N hosts (never a full mirror on every
    host). Writes fan out to every member of the set; reads consult a
    quorum and return the newest version. Any number of clients can mount
    the same namespace from different machines and add files to one shared
    tree, and a host failure degrades reads and writes to the remaining
    healthy members without stale or corrupt data."""

    deps = [
        AcceptsShardTopology,
        ComputesDeterministicPlacement,
        ReplicatesToSubsetOfHosts,
        AllocatesGloballyUniqueInodes,
        AllocatesInodesViaSetLeader,
        ServesReadsFromReplicationSet,
        VersionsMutations,
        FansOutWritesToSetMembers,
        RequiresWriteQuorum,
        DegradesGracefullyOnHostFailure,
        SupportsCrossSetRename,
        SupportsSharedMounts,
        ShardObservability,
    ]

    def title(self):
        return "Sharded Filesystem"

    def status(self):
        return PLANNED

    def status_note(self):
        return (
            "Specified and designed (`design/sharded_filesystem.md`), not "
            "implemented. The daemon does not accept `--shards` or "
            "`--replication-factor`, and `scripts/shard_db.sh` does not exist "
            "yet. The tutorial and how-to describe the intended interface."
        )

    def tutorial(self):
        return md(r"""
            !!! warning "Preview"
                This tutorial describes the intended experience once
                Phase 2 of the roadmap lands. None of its commands work
                yet.

            In this tutorial we will spread one pgfs namespace across five
            simulated hosts, write files into different subtrees, stop a
            host and keep working, and mount the same namespace a second
            time as another client.

            ## Step 1: start five hosts

            ```bash
            ./scripts/shard_db.sh up
            ./scripts/shard_db.sh conns
            ```

            Each simulated host is its own PostgreSQL cluster
            (`testdata/pgdata-1` … `testdata/pgdata-5`). `conns` prints the
            comma-separated `--shards` list.

            ## Step 2: mount the namespace

            ```bash
            ./target/debug/pgfs --shards "$(./scripts/shard_db.sh conns)" \
                --replication-factor 3 testdata/mnt &
            ```

            The startup log announces the topology:

            ```text
            INFO pgfs: shard topology: 5 hosts, R=3, sets=…
            ```

            ## Step 3: write into two subtrees

            ```bash
            mkdir testdata/mnt/docs testdata/mnt/src
            echo hi  > testdata/mnt/docs/a.txt
            echo bye > testdata/mnt/src/b.txt
            ```

            `docs` and `src` are top-level names, so each subtree is placed
            on its own replication set of three hosts. The SIGUSR1 state
            dump shows which.

            ## Step 4: lose a host

            ```bash
            pg_ctl -D testdata/pgdata-2 stop
            cat testdata/mnt/docs/a.txt testdata/mnt/src/b.txt
            echo again >> testdata/mnt/docs/a.txt
            ```

            Every command still succeeds. Reads and writes need only two
            of each set's three members. The metrics now report one
            degraded host.

            ## Step 5: bring it back, and add a second client

            ```bash
            ./scripts/shard_db.sh up        # restarts host 2; it re-syncs
            mkdir -p /tmp/client2
            ./target/debug/pgfs --shards "$(./scripts/shard_db.sh conns)" /tmp/client2 &
            cat /tmp/client2/docs/a.txt
            ```

            The second mount sees the same tree, including the line
            written while host 2 was down.

            ## What we will have done

            We will have run one namespace on five hosts with each file on
            three of them, survived a host failure without errors, and
            shared the tree between two clients. The
            [explanation](../explanation/sharded_filesystem.md) describes
            the design.
        """)

    def how_to(self):
        return md(r"""
            !!! warning "Planned interface"
                These instructions describe the designed interface. None
                of it is implemented yet.

            ## Deploy a sharded namespace

            1. Run one PostgreSQL instance per host and create a `pgfs`
               database on each.
            2. Start pgfs on every client with the same host list and
               replication factor:

               ```bash
               pgfs --shards "host=h1 dbname=pgfs,host=h2 dbname=pgfs,host=h3 dbname=pgfs,host=h4 dbname=pgfs,host=h5 dbname=pgfs" \
                    --replication-factor 3 /srv/pgfs
               ```

            Every client must list the hosts in the same order; placement
            is computed from the list.

            ## Choose N and R

            Use `R = 3` and `N ≥ R + 1`. A file survives the loss of
            `R − 1` members of its set and stays writable while a quorum
            (`⌊R/2⌋ + 1`) is up.

            ## Bound how stale other clients' changes can look

            ```bash
            pgfs --shards "…" --attr-ttl 1 /srv/pgfs
            ```

            Changes from other clients appear within the TTL.

            ## Detect a failing host

            Watch `SHARD_DEGRADED_HOSTS` and `SHARD_QUORUM_MISSES` in the
            minute-by-minute metrics, and use the SIGUSR1 state dump for
            per-host health.

            ## Recover a host

            Restart its PostgreSQL. On recovery the host is compared with
            the highest version of each entry in its sets and missing
            mutations are re-applied.

            ## Add or remove hosts

            Not supported online. The topology is fixed at mount time, and
            changing it requires a migration run (design §3.4).
        """)

    def reference(self):
        return md(r"""
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
        """)

    def explanation(self):
        return md(r"""
            ## From one database to many

            Today a pgfs namespace lives in one PostgreSQL database,
            optionally with a read-only standby. That caps capacity at one
            machine's disk and makes that machine a single point of
            failure. The sharded design spreads the namespace across N
            independent hosts and lets many clients mount it at once.

            ## Why not mirror everything

            Copying every file to every host is simple, but it wastes
            capacity, since N hosts hold one host's worth of data, and every
            write costs N writes. Instead each file lives on R of the N
            hosts, its replication set. With R = 3 of N = 5, losing any two
            hosts still leaves every file at least one copy, and losing any
            one keeps every set writable (two of three is a quorum), while
            total capacity grows with the number of hosts. This is the sense in
            which the design is "RAID-5-like": partial redundancy spread
            across members, not a full mirror. Unlike RAID 5, it copies
            whole files rather than computing parity.

            ## Why place by top-level directory

            Placement uses the entry's top-level name, so everything under
            `/docs` lives on one set. Hashing each file separately would
            spread a directory's children across every host, and every
            `ls` would become a cluster-wide merge. Subtree-sticky
            placement keeps listings and most renames local to one set,
            at the cost of hot spots if one top-level directory is much
            larger than the rest.

            ## Why quorums and versions

            The hosts in a set are independent databases with no
            replication between them, so they can disagree, for example
            after one missed a write while it was down. A write therefore
            counts as done only when a majority of the set has it, and
            every write carries a version number. A read asks a majority
            and keeps the highest version. Any two majorities overlap, so a
            read always sees the latest acknowledged write, even with a
            member down.

            Versions combine a client identifier with a per-client
            counter, so they can be compared without synchronised clocks.
            The cost is last-writer-wins semantics when two clients write
            the same file at the same time.

            ## Why inode numbers encode the set

            Each host's database has its own sequence, so two hosts would
            hand out the same numbers. Putting the set id in the high bits
            and letting one member of each set allocate makes inode numbers
            unique across the cluster without coordination between sets.

            ## What the design leaves out

            Cross-client cache coherence is bounded staleness via the
            attribute TTL, not a guarantee: two clients can briefly see
            different views. Changing the host list requires a migration.
            Both are deliberate v1 limits, and the roadmap addresses the
            first with leases in Phase 5.
        """)
