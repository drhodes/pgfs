# PGFS Sharded Shared Filesystem Design

**Document Version:** 0.2.0 (Draft)
**Status:** Proposed Architectural Specification
**Author:** PGFS Engineering Team
**Target Systems:** Linux VFS / FUSE, PostgreSQL 15+, one metadata cluster + N data hosts
**Supersedes:** 0.1.0 (kept as `sharded_filesystem-v0.1.md`)

---

## Executive Summary

Today `pgfs` mounts a directory tree backed by exactly one Postgres
instance, optionally with a physical streaming standby that serves
metadata reads (`spec/replica.py`). This design scales it out to many hosts
and many clients.

Version 0.1 sharded the whole namespace across independent databases and
had every client fan writes out to `R` of them, resolving conflicts with
client-side version numbers. Reviewing it against GFS and the twelve
systems GFS cites (`design/papers/reading-notes.md`) showed that this is
the one architecture none of those systems use, and that its ordering,
rollback and placement rules lose data under ordinary concurrency (§0).

Version 0.2 adopts the architecture the literature converges on:

- **Metadata stays in one Postgres** (the *metadata database*, MDB), with
  its own synchronous standby. Every namespace operation — lookup, create,
  `readdir`, rename, `rmdir` — remains a single local transaction, exactly
  as today. (GFS master, NASD file manager, Lustre MDS, Swift mediator.)
- **File contents are spread across data sets.** A *data set* is one
  Postgres primary plus `R − 1` synchronous standbys. Each file's blocks
  live on one data set, recorded in the file's metadata row. Clients read
  and write blocks directly on the set's primary. (GFS chunkservers,
  Lustre OSTs.)
- **Replication is primary-copy, done by Postgres.** The set primary
  orders all writes; streaming replication with a synchronous standby
  makes every acknowledged write survive the loss of the primary. Clients
  never fan out writes. (Harp, the model GFS §8 recommends.)
- **Placement is a table, not a hash.** The MDB records which data set
  holds each file and which host is each set's current primary, so data
  can be moved and hosts replaced without recomputing a ring. (xFS manager
  map, AFS volume location database.)
- **Shared mounts are coordinated through the MDB**: close-to-open
  consistency from a per-file data version, then pushed invalidations,
  then write leases. (AFS, Coda, Frangipani, GPFS.)

The single-node configuration (`--conn` alone) is the degenerate case: an
MDB with no data sets keeps blocks in its own `blocks` table, as now.

---

## Table of Contents

0. [Changes from v0.1](#0-changes-from-v01)
1. [Goals & Non-Goals](#1-goals--non-goals)
2. [Terminology & Topology](#2-terminology--topology)
3. [Schema](#3-schema)
4. [Placement](#4-placement)
5. [Inode Allocation](#5-inode-allocation)
6. [Read Path](#6-read-path)
7. [Write Path: Ordered Writes](#7-write-path-ordered-writes)
8. [Delete, Truncate & Garbage Collection](#8-delete-truncate--garbage-collection)
9. [Failure Handling](#9-failure-handling)
10. [Moving Data Between Sets](#10-moving-data-between-sets)
11. [Shared Mounts & Cache Coherence](#11-shared-mounts--cache-coherence)
12. [CLI & Configuration](#12-cli--configuration)
13. [Observability & Metrics](#13-observability--metrics)
14. [Operational Scripts](#14-operational-scripts)
15. [Consistency Model](#15-consistency-model)
16. [Phased Implementation Roadmap](#16-phased-implementation-roadmap)
17. [Open Questions](#17-open-questions)

---

## 0. Changes from v0.1

| v0.1 decision | Problem | v0.2 decision | Source |
|---|---|---|---|
| Shard the namespace (`entries` and `blocks`) by top-level name | `readdir`, rename and inode allocation become multi-host problems; one large top-level directory lands on `R` hosts | Shard only file contents; namespace stays in one MDB | GFS, NASD, Lustre, Swift |
| Client fans every write out to all `R` members, quorum ack | No single order of writes per replica: two clients' writes to different blocks can land on different quorums, and a quorum read then misses one | One primary per data set orders all writes; Postgres replicates | Harp, GFS leases |
| `ver = (client_id << 32) \| seq`, last-writer-wins | Compares by client id first, so one client always wins regardless of time | Single primary per set; per-file `data_ver` bumped by the MDB | GFS chunk versions |
| Compensating delete when quorum fails | Cannot undo an overwrite; old bytes are gone | No rollback needed: a data-set transaction commits or not | — |
| Consistent-hash ring over top-level names | Membership change remaps names; no way to move one subtree | Placement table in the MDB; per-file `set_id` | xFS, AFS |
| `global_ino = set_id << 48 \| seq`, set-leader allocation with takeover region | Complex, and takeover can collide | MDB identity sequence (already unique) | — |
| Cross-set rename copies the subtree | `O(subtree)` data movement inside `rename(2)` | Rename is metadata-only (blocks keyed by `ino`) | AFS (forbids it), GFS |
| Health by `K` consecutive failures | Slow-but-alive hosts are the common fault and are never detected | Latency tracked per set; slow sets avoided for new files | River, GPFS |
| "RAID-5-like" | RAID-5 means parity; this is mirroring | "`R`-way mirroring on a subset of hosts" | RAID |
| Leases deferred to Phase 5 | Cache validation dominates traffic without them | Close-to-open in Phase 3, pushed invalidation in Phase 3, write leases Phase 5 | AFS, Coda |

---

## 1. Goals & Non-Goals

### Goals

- **Shared**: many machines mount the same namespace and add files
  concurrently.
- **Subset-replicated**: each file's contents live on one data set of `R`
  hosts (e.g. 2 or 3), never on every host. Losing `R − 1` hosts of a set
  loses no acknowledged write.
- **Scale-out capacity**: total data capacity is the sum of the data sets.
- **Fault-tolerant**: a failed set primary is replaced by its standby;
  files on other sets are unaffected; nothing stale or torn is returned.
- **Backwards compatible**: `--conn` alone is today's single-node pgfs.
- **Every phase is useful on its own** (§16): the Phase 0 prerequisites
  improve single-node pgfs before any sharding exists.

### Non-Goals (v1)

- **Metadata scale-out.** One MDB holds the namespace. GFS reports roughly
  100 bytes of master metadata per file; a Postgres row per entry is
  larger but the same order, so tens of millions of entries fit on one
  host. Partitioned metadata is future work (§17).
- **Erasure coding.** Parity across hosts turns every small write into a
  cross-host read-modify-write. v1 mirrors whole blocks.
- **Striping one file across sets.** A file's blocks live on one set.
  Large-file striping (GFS chunks, Lustre stripes) is Phase 5.
- **Byte-range concurrency control across clients.** v1 gives
  close-to-open consistency; concurrent writers to one file get
  per-block last-commit-wins (§15).
- **Writing failover orchestration.** Promotion of a set standby is
  delegated to existing Postgres tooling (§9.2).

---

## 2. Terminology & Topology

| Term | Meaning |
|---|---|
| **MDB** | The metadata database: one Postgres primary (plus synchronous standby) holding `entries` and the topology tables. Today's `--conn` database. |
| **data host** | A machine running one Postgres instance that stores `blocks` rows. |
| **data set** | A placement group: one data-host primary and `R − 1` synchronous standbys holding identical `blocks`. Identified by `set_id`. |
| **epoch** | Per-set counter in the MDB, incremented on every primary change. Used to detect stale routing (§9.2). |
| **`data_ver`** | Per-file counter in `entries`, incremented on every committed write. Drives cache revalidation (§11) and safe moves (§10). |
| **`data_lsn`** | WAL position on the set primary after a file's last write commit. Lets a standby serve a read once it has replayed that far (§6.2). |
| **client** | A pgfs daemon (one mount). Identified by `client_id`, registered in the MDB with a lease (§11.3). |

Topology for the local test environment: one MDB, `S = 2` data sets, `R = 2`
(primary + one standby) — six Postgres instances. Production defaults are
an open question (§17).

Physical streaming replication is used **within** a data set and within
the MDB. Data sets are independent of each other; there is no cross-set
replication.

---

## 3. Schema

### 3.1 Prerequisite: key blocks by inode

Today `blocks` is keyed by path: `PRIMARY KEY (parent, name, block_no)`.
Renaming a directory rewrites the `parent` column of every block row under
it (with the `LIKE` wildcard bug), and renaming a file with buffered
writes loses data. Moving blocks to another host makes path keys
untenable, so the first step — independently valuable on one node — is:

```sql
-- blocks: one row per 64 KiB block of a file
PRIMARY KEY (ino, block_no)          -- replaces (parent, name, block_no)
-- parent/name columns dropped from blocks after migration
```

After this, rename touches only `entries`.

### 3.2 MDB

```sql
ALTER TABLE entries
  ADD COLUMN set_id   integer,                 -- NULL: blocks in the MDB's own blocks table
  ADD COLUMN data_ver bigint  NOT NULL DEFAULT 0,
  ADD COLUMN data_lsn pg_lsn;                  -- on the set primary, after last write

CREATE TABLE data_sets (
  set_id      integer PRIMARY KEY,
  primary_dsn text    NOT NULL,                -- current primary
  standby_dsn text[]  NOT NULL DEFAULT '{}',
  epoch       bigint  NOT NULL DEFAULT 1,
  state       text    NOT NULL DEFAULT 'active' -- active | degraded | draining | retired
);

CREATE TABLE clients (
  client_id   bigint  GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  hostname    text    NOT NULL,
  mountpoint  text    NOT NULL,
  lease_until timestamptz NOT NULL
);
```

### 3.3 Data host

```sql
CREATE TABLE blocks (
  ino      bigint  NOT NULL,
  block_no integer NOT NULL,
  data     bytea   NOT NULL,
  PRIMARY KEY (ino, block_no)
);
CREATE TABLE set_identity (set_id integer NOT NULL, epoch bigint NOT NULL); -- one row
```

`set_identity` lets a client verify it is talking to the primary of the
set and epoch it expects (§9.2).

---

## 4. Placement

- A new regular file is assigned a `set_id` at its first block write, not
  at `create` (empty files and directories never touch a data set).
- The client chooses among sets in state `active`, preferring the set with
  the most free space and lowest recent latency (§13). Degraded and
  draining sets receive no new files (Lustre skips failed OSTs on create;
  xFS writes only to current stripe groups).
- The choice is written to `entries.set_id` in the same MDB transaction
  that records the file's first size update (§7).
- Placement is per file, not per subtree. v0.1 needed subtree-stickiness
  so that `readdir` stayed on one host; with metadata in the MDB,
  `readdir` never touches a data set, so per-file placement balances load
  without that cost.
- Directories, symlinks (when supported) and files below an inline
  threshold may keep their data in the MDB (`set_id IS NULL`). The
  threshold is an open question (§17).

---

## 5. Inode Allocation

Inodes come from the MDB's `entries.ino` identity sequence, as today, and
are globally unique by construction. No encoding of set ids into inode
numbers is needed, and a file keeps its inode when it moves between sets.

Prerequisite: fix the existing bug where the root row is inserted with an
explicit `ino = 1` without advancing the sequence, so the first `create`
on a fresh database fails with a duplicate key.

Optional optimization (Lustre pre-allocation): a client can reserve a
range of inode numbers in one round trip (`SELECT nextval(...) FROM
generate_series(1, 64)`) and insert them with `OVERRIDING SYSTEM VALUE`,
cutting a round trip from each `create`.

---

## 6. Read Path

### 6.1 Metadata

`lookup`, `getattr` and `readdir` read the MDB, routed to its standby
when fresh, exactly as today's replica logic. Rename, `rmdir` and other
read-before-write checks read the MDB primary (`WritesStayOnPrimary`).

Batch attributes with directory listings (READDIRPLUS): one query
returns names and attributes, so `ls -l` and `git status` do not issue a
`getattr` per entry. (GPFS reports ~10× from inode prefetch on `ls -l`.)

### 6.2 File data

1. Resolve the file's `set_id`, `data_ver` and `data_lsn` from the cached
   entry (refreshed per §11).
2. Read blocks from the set primary by `(ino, block_no)`.
3. Phase 4: read from a set standby instead when
   `pg_last_wal_replay_lsn() >= data_lsn` on that standby, choosing the
   member with the lowest recent latency (River's graduated declustering).
   This generalizes today's per-database freshness check to per-file
   freshness.

---

## 7. Write Path: Ordered Writes

There is no distributed transaction between the MDB and a data set.
Writes are made safe by ordering, the same rule ext4 uses in
`data=ordered` mode: **data reaches durable storage before the metadata
that makes it visible.**

On flush of a handle's dirty blocks (prerequisite: today's buffered-write
bug that zeroes untouched bytes of a partially written block must be
fixed first — partial blocks need read-modify-write):

1. **Data transaction** on the set primary: upsert the dirty
   `(ino, block_no)` rows in one transaction with
   `synchronous_commit = on` (or `remote_apply`), so the commit returns
   only after a synchronous standby has the WAL. Record
   `pg_current_wal_lsn()` after commit.
2. **Metadata transaction** on the MDB:

   ```sql
   UPDATE entries
      SET size     = GREATEST(size, $new_end),   -- extending writes merge (GPFS metanode rule)
          mtime    = now(),
          data_ver = data_ver + 1,
          data_lsn = $lsn,
          set_id   = COALESCE(set_id, $chosen_set)
    WHERE ino = $ino;
   ```

Failure cases:

| Fails at | Result |
|---|---|
| Step 1 | `EIO`; nothing changed anywhere. |
| Between 1 and 2 (client crash) | In-place overwrites within the old size are visible; bytes beyond the old size are invisible. Identical to a local file system crashing after writeback but before the inode update. |
| Step 2 | Retry step 2; if it cannot succeed, `EIO`. Blocks beyond `size` are reclaimed by GC (§8). |

**Durability setting.** pgfs currently forces `synchronous_commit = off`
for speed. Ordered writes require the data commit to be durable before
the metadata commit, so sharded mode uses `on`/`remote_apply` on data
sets and `on` on the MDB. Single-node mode keeps today's setting.

**Concurrent writers.** All writes to a set are ordered by its single
primary, so replicas never diverge (unlike v0.1). Two clients writing the
same block get last-commit-wins at the block level; `size` takes the
maximum. `truncate`/`utimes`, which can decrease size or move mtime
backwards, set the value explicitly (GPFS takes an exclusive lock for
exactly these).

---

## 8. Delete, Truncate & Garbage Collection

Metadata first, data later — the reverse of writes, for the same reason:
a reader must never reach data through metadata that has been removed.

- `unlink`/`rmdir` delete the `entries` row in one MDB transaction. The
  file's blocks become garbage.
- `truncate` lowers `size` and bumps `data_ver`; blocks past the new end
  become garbage (the partial last block is rewritten in the data
  transaction first).
- A **garbage collector** (GFS §4.4) runs per data set at low priority:
  delete `blocks` rows whose `ino` has no `entries` row with that
  `set_id`, or whose `block_no` lies beyond the entry's size.
  Safe because inode numbers are never reused and an entry always exists
  before its first block write (§4).
- GC also cleans up after a client crash between §7 steps 1 and 2 and
  after an aborted move (§10).

---

## 9. Failure Handling

### 9.1 Slow hosts

Each client keeps a moving average of latency per set and per MDB host.
Placement (§4) avoids slow sets for new files; Phase 4 standby reads
avoid slow members. A host that is slow but alive is the common fault
(River) and must not be modelled only as up or down.

### 9.2 Data-set primary failure

- **Detection and promotion are delegated** to established Postgres
  failover tooling with an independent witness — for example
  pg_auto_failover (its monitor node) or Patroni (its DCS). The witness
  plays the role of Harp's witness: it breaks ties without holding data,
  so `R = 2` copies get three-node failover decisions.
- After promotion, the tool (or a hook) updates the set's row in
  `data_sets`: new `primary_dsn`, `epoch = epoch + 1`, and writes the
  new epoch into the promoted host's `set_identity`.
- **No acknowledged write is lost**: each data commit waited for a
  synchronous standby, so the promoted standby has it.
- **An isolated old primary cannot acknowledge new writes**: with
  `synchronous_standby_names` set and its standby gone, commits on it
  block instead of returning. A client whose commit times out reports
  `EIO`, re-reads `data_sets`, and retries against the new primary.
  (Caveat: a cancelled wait leaves the transaction committed locally on
  the old primary; see §17.)
- **Stale routing is detected**: before using a data connection, and
  after any error, a client checks `set_identity` against the epoch it
  read from `data_sets`. This is the fencing token Frangipani recommends
  for writes from a holder whose lease has expired.
- While a set has no primary, reads and writes of its files return `EIO`
  after a timeout; files on other sets are unaffected.

### 9.3 MDB failure

The MDB is a single point of failure, as GFS's master is. It gets the same
treatment as a data set: a synchronous standby, a witness, promotion by
the same tooling, and clients that reconnect to the new primary.
Prerequisite: pgfs today never reconnects after a database restart; Phase
0 adds reconnection with re-resolution of the target.

### 9.4 Re-replication

A standby that returns resumes streaming. A replaced host is rebuilt with
`pg_basebackup --max-rate` so rebuild traffic does not starve clients
(GFS throttles cloning). Sets with no healthy standby are rebuilt first:
repair time dominates mirrored reliability (RAID's MTTF analysis).

---

## 10. Moving Data Between Sets

Used for rebalancing, draining a set, and replacing hardware.

1. Mark the source set `draining` (no new placements; xFS's obsolete
   stripe groups).
2. For each file: note `data_ver`, copy its blocks to the destination
   set (data transaction, §7 step 1).
3. In one MDB transaction: `UPDATE entries SET set_id = $dst WHERE ino =
   $ino AND data_ver = $seen` — a compare-and-swap. If a write landed
   meanwhile, zero rows update and the file is retried.
4. Source blocks become garbage (§8).

Clients holding the old `set_id` in cache see the bumped state on their
next revalidation (§11). Reads in the window between steps 3 and the
client's revalidation still find the old blocks until GC removes them,
so GC must not reclaim a moved file's source blocks until every live
client lease has been renewed once after the move (§17).

This replaces v0.1's cross-set rename: rename never moves data.

---

## 11. Shared Mounts & Cache Coherence

Every client reads topology and metadata from the MDB, so all clients agree
on placement by construction (v0.1 relied on identical `--shards` lists).

### 11.1 v1: bounded staleness

Kernel attribute and entry caches expire after `--attr-ttl` (default
1 s), as today.

### 11.2 Phase 3: close-to-open (AFS)

On `open`, the client revalidates the entry against the MDB. If
`data_ver` is unchanged, it replies with `FOPEN_KEEP_CACHE` so the kernel
keeps its page cache; otherwise the kernel drops it. A reader therefore
sees every write that was flushed before it opened the file.

### 11.3 Phase 3: pushed invalidation (callbacks)

Clients register in `clients` with a renewable lease and `LISTEN` on a
notification channel. A metadata change `NOTIFY`s the inode and parent;
receiving clients call the FUSE invalidation notifications
(`FUSE_NOTIFY_INVAL_INODE`, `FUSE_NOTIFY_INVAL_ENTRY`) so the kernel drops
cached attributes and names immediately. This lets `--attr-ttl` be raised
safely; AFS, Coda and Frangipani all found validation traffic dominates
without callbacks. A client whose lease lapses drops all cached state.

### 11.4 Phase 5: write leases

Per-file write leases (Frangipani's whole-file locks, GPFS's byte-range
tokens) for workloads where several clients write one file. Not needed
for the target workloads (source trees, documents, build outputs), where
write sharing is rare (xFS measured 96 % of overwrites by the previous
writer).

---

## 12. CLI & Configuration

```text
pgfs [OPTIONS] MOUNTPOINT

--conn <CONN>        MDB connection string (unchanged)
--replica <CONN>     MDB standby for metadata reads (unchanged)
--attr-ttl <secs>    Kernel attr/entry cache TTL (unchanged, default 1.0)
--unlogged           Unlogged tables (single-node mode only)
```

Data sets are **not** listed on the command line. They are registered in
the MDB's `data_sets` table, which every client reads at mount and after
errors, so topology changes need no client restarts:

```text
pgfs-admin set add    --primary <DSN> [--standby <DSN> ...]
pgfs-admin set drain  <set_id>
pgfs-admin set list
```

(`pgfs-admin` can start as SQL scripts.) v0.1's `--shards` and
`--replication-factor` are dropped; `R` is however many standbys a set
has.

---

## 13. Observability & Metrics

Extends the existing metrics export (`spec/observe.py`):

- Per set: read/write counts, latency histogram, current epoch, state.
- `SET_FAILOVERS` (epoch changes observed), `STALE_EPOCH_RETRIES`.
- `GC_PENDING_BLOCKS`, `GC_RECLAIMED_BLOCKS` per set.
- `MOVE_CAS_RETRIES` (§10).
- Phase 3: `INVALIDATIONS_RECEIVED`, `OPEN_REVALIDATIONS`,
  `KEEP_CACHE_HITS`.
- SIGUSR1 dump: `data_sets` as seen by this client, per-set latency,
  lease expiry.
- Startup log line: `topology: mdb=<host>, sets=<n>, standbys=<per set>`.

---

## 14. Operational Scripts

- `scripts/shard_db.sh up|down|status` — provision the local test
  topology (§2) under `testdata/`: an MDB pair and `S` data sets of
  `R` instances each, with synchronous replication configured, and
  register the sets in `data_sets`. Must remove containers and stale
  volumes on `down` (today's `replica_db.sh` leaves both behind).
- `scripts/test_shard.sh` — integration suite: write across sets; kill a
  set primary, promote, verify no acknowledged write is lost and
  `EIO`-then-success for in-flight writes; drain a set and verify moves;
  two clients with close-to-open semantics.
- `Makefile`: `shard-up`, `shard-down`, `test-shard`.

---

## 15. Consistency Model

| Guarantee | v1 (Phases 1–2) | With Phase 3 |
|---|---|---|
| Namespace operations (create, rename, unlink, `rmdir`) | Atomic, serialized by the MDB | Same |
| Read-your-writes (same client) | Yes | Yes |
| Cross-client visibility of data | Bounded by `--attr-ttl` | Close-to-open; invalidation pushed |
| Concurrent writes to the same block | Last commit wins; replicas never diverge | Same |
| File size under concurrent extension | Maximum of writers' ends | Same |
| Acknowledged write after set-primary failure | Durable (synchronous standby) | Same |
| Crash between data and metadata commit | Old size; overwrites within it visible | Same |

---

## 16. Phased Implementation Roadmap

Each phase lands green and is useful without the next.

**Phase 0 — single-node prerequisites** (fix known bugs, reshape storage)
- Fix partial-block buffered writes (read-modify-write), the `LIKE`
  wildcard in directory rename, the root-row sequence bug, and add
  database reconnection.
- Re-key `blocks` by `(ino, block_no)`; rename becomes metadata-only.
- Add `entries.data_ver`; READDIRPLUS-style batched attributes.
- Exit: `make test-integration` plus regression tests for each bug.

**Phase 1 — data sets without replication**
- `data_sets`, `entries.set_id`; route block I/O; placement on first
  write; ordered writes (§7); GC (§8).
- Exit: `test_shard.sh` write/read/delete across two single-instance
  sets; crash-between-commits test shows the §7 table.

**Phase 2 — replication and failover**
- Synchronous standby per set and for the MDB; epochs and
  `set_identity`; reconnect-and-reroute; failover tooling integration.
- Exit: kill-primary tests lose no acknowledged write.

**Phase 3 — shared-mount coherence**
- Close-to-open via `data_ver` and `FOPEN_KEEP_CACHE`; client leases;
  `LISTEN/NOTIFY` invalidation.
- Exit: two mounts pass a visibility test without relying on TTL expiry.

**Phase 4 — operations**
- Draining and moving (§10); standby reads by LSN; latency-aware
  placement.

**Phase 5 — future**
- Write leases / byte-range tokens; striping large files across sets;
  erasure coding; partitioned metadata.

---

## 17. Open Questions

1. **Defaults for `S` and `R`.** `R = 2` plus a witness (Harp) or
   `R = 3` with quorum commit (`synchronous_standby_names = 'ANY 1 (...)'`)?
2. **Inline threshold.** Below what size do file contents stay in the MDB
   (§4)? Small files are most files in source trees.
3. **Cancelled synchronous waits.** If pgfs times out a commit waiting
   for a standby, Postgres has committed it locally on the primary. Is
   reporting `EIO` and re-reading after reroute sufficient, or should
   such files be marked for verification?
4. **GC safety after moves.** Is "every live lease renewed once" the
   right bound before reclaiming a moved file's source blocks (§10)?
5. **Metadata scale-out.** When one MDB is not enough, partition the
   namespace (by directory, as IndexFS/HopsFS-style systems do) or move
   metadata to a distributed SQL store? See the recent-papers survey.
6. **Large files.** At what size does per-file placement need striping
   (Phase 5)?
